use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const VERTEX_STRIDE: usize = 36;
pub const SKIN_RECORD_STRIDE: usize = 20;
const EPSILON: f32 = 1.0e-6;

#[derive(Clone, Debug)]
pub struct GeoSubset {
    pub material: usize,
    pub flags: u32,
    pub start: u32,
    pub count: u32,
}

#[derive(Clone, Debug)]
pub struct GeoInfluence {
    pub bone_index: usize,
    pub weight: f32,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct GeoSkeleton {
    pub skeleton_ptr: u32,
    pub bone_count: usize,
    pub names: Vec<String>,
    pub first_child: Vec<u32>,
    pub next_sibling: Vec<u32>,
    pub parent: Vec<Option<usize>>,
    pub bind_matrices: Vec<[f32; 16]>,
    pub inverse_bind_matrices: Vec<[f32; 16]>,
    pub aux_a_off: u32,
    pub aux_b_off: u32,
    pub name_table_off: u32,
    pub weights: Option<Vec<Vec<GeoInfluence>>>,
}

#[derive(Clone, Debug, Default)]
pub struct GeoWeightProfile {
    pub has_weights: bool,
    pub weighted_vertex_count: usize,
    pub single_influence_vertices: usize,
    pub multi_influence_vertices: usize,
    pub max_influences_per_vertex: usize,
    pub rigid_single_influence: bool,
    pub single_bone_faces: usize,
    pub mixed_bone_faces: usize,
    pub rigid_face_partition: bool,
    pub dominant_bone_vertex_counts: Vec<(String, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeoAssetType {
    MeshOnly,
    RigidProp,
    SkinnedMesh,
}

impl GeoAssetType {
    pub fn as_str(&self) -> &'static str {
        match self {
            GeoAssetType::MeshOnly => "Mesh Only",
            GeoAssetType::RigidProp => "Rigid Prop",
            GeoAssetType::SkinnedMesh => "Skinned Mesh",
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct GeoFile {
    pub path: PathBuf,
    pub vertex_offset: u32,
    pub index_offset: u32,
    pub vertex_count: usize,
    pub index_count: usize,
    pub verts: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 4]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u16>,
    pub faces: Vec<[u16; 3]>,
    pub texture_names: Vec<String>,
    pub subsets: Vec<GeoSubset>,
    pub skeleton: Option<GeoSkeleton>,
    pub asset_type: GeoAssetType,
    pub weight_profile: GeoWeightProfile,
}

pub fn load_geo(path: &Path) -> Result<GeoFile> {
    let data = fs::read(path)
        .with_context(|| format!("Failed to read GEO file {}", path.display()))?;

    let vertex_offset = read_u32(&data, 0x68)?;
    let index_offset = read_u32(&data, 0x6C)?;
    let vertex_count = read_u32(&data, 0x8C)? as usize;
    let index_count = read_u32(&data, 0xB4)? as usize;

    if vertex_offset as usize + vertex_count * VERTEX_STRIDE > data.len() {
        bail!("Vertex block exceeds file size");
    }
    if index_offset as usize + index_count * 2 > data.len() {
        bail!("Index block exceeds file size");
    }
    if index_count % 3 != 0 {
        bail!("Index count is not divisible by 3");
    }

    let mut verts = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut colors = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);

    for i in 0..vertex_count {
        let off = vertex_offset as usize + i * VERTEX_STRIDE;
        let x = read_f32(&data, off)?;
        let y = read_f32(&data, off + 4)?;
        let z = read_f32(&data, off + 8)?;
        let nx = read_f32(&data, off + 12)?;
        let ny = read_f32(&data, off + 16)?;
        let nz = read_f32(&data, off + 20)?;
        let rgba = read_bytes4(&data, off + 24)?;
        let u = read_f32(&data, off + 28)?;
        let v = read_f32(&data, off + 32)?;

        verts.push([x, y, z]);
        normals.push([nx, ny, nz]);
        colors.push(rgba);
        uvs.push([u, v]);
    }

    let mut indices = Vec::with_capacity(index_count);
    for i in 0..index_count {
        indices.push(read_u16(&data, index_offset as usize + i * 2)?);
    }

    let mut faces = Vec::with_capacity(index_count / 3);
    for i in (0..index_count).step_by(3) {
        faces.push([indices[i], indices[i + 1], indices[i + 2]]);
    }

    let texture_names = parse_texture_names(&data)?;
    let texture_count = texture_names.len().max(1);
    let subsets = parse_subsets(&data, texture_count, index_count)?;
    let skeleton = parse_skeleton(&data, vertex_count)?;
    let weight_profile = summarize_weight_profile(skeleton.as_ref(), &faces);
    let asset_type = classify_geo_asset(skeleton.as_ref(), &weight_profile);

    Ok(GeoFile {
        path: path.to_path_buf(),
        vertex_offset,
        index_offset,
        vertex_count,
        index_count,
        verts,
        normals,
        colors,
        uvs,
        indices,
        faces,
        texture_names,
        subsets,
        skeleton,
        asset_type,
        weight_profile,
    })
}

fn parse_texture_names(data: &[u8]) -> Result<Vec<String>> {
    let tex_count = read_u32(data, 0x54)? as usize;
    let tex_ptr = read_u32(data, 0x5C)? as usize;

    let mut names = Vec::new();

    if tex_ptr != 0 && tex_ptr + tex_count * 4 <= data.len() {
        for i in 0..tex_count {
            let sptr = read_u32(data, tex_ptr + i * 4)? as usize;
            if sptr < data.len() {
                let end = data[sptr..]
                    .iter()
                    .position(|&b| b == 0)
                    .map(|p| sptr + p)
                    .unwrap_or(data.len());

                let raw = &data[sptr..end];
                if let Ok(s) = std::str::from_utf8(raw) {
                    if !s.is_empty() {
                        names.push(s.to_string());
                    }
                }
            }
        }
    }

    if names.is_empty() {
        for s in extract_ascii_strings(data, 4) {
            let low = s.to_ascii_lowercase();
            if low.ends_with(".dds")
                || low.ends_with(".png")
                || low.ends_with(".tga")
                || low.ends_with(".bmp")
            {
                names.push(s);
            }
        }
    }

    let mut dedup = Vec::new();

    for name in names {
        let already_seen = dedup
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&name));

        if !already_seen {
            dedup.push(name);
        }
    }

    Ok(dedup)
}

fn parse_subsets(data: &[u8], texture_count: usize, index_count: usize) -> Result<Vec<GeoSubset>> {
    let subset_ptr = read_u32(data, 0x94)? as usize;
    let mut subsets = Vec::new();

    if subset_ptr != 0 && subset_ptr + texture_count * 16 <= data.len() {
        let mut ok = true;
        let mut running_total = 0usize;

        for i in 0..texture_count {
            let base = subset_ptr + i * 16;
            let mut mat_index = read_u32(data, base)? as usize;
            let flags = read_u32(data, base + 4)?;
            let start = read_u32(data, base + 8)?;
            let mut count = read_u32(data, base + 12)?;

            if count == 0 && texture_count == 1 && i == 0 {
                count = index_count as u32;
            }

            if mat_index >= texture_count {
                mat_index = texture_count.saturating_sub(1);
            }

            if start as usize + count as usize > index_count || count % 3 != 0 || start % 3 != 0 {
                ok = false;
                break;
            }

            subsets.push(GeoSubset {
                material: mat_index,
                flags,
                start,
                count,
            });

            running_total += count as usize;
        }

        if ok && running_total == index_count {
            return Ok(subsets);
        }
    }

    Ok(vec![GeoSubset {
        material: 0,
        flags: 0,
        start: 0,
        count: index_count as u32,
    }])
}

fn parse_skeleton(data: &[u8], vertex_count: usize) -> Result<Option<GeoSkeleton>> {
    let skeleton_ptr = read_u32(data, 0x10)? as usize;
    if skeleton_ptr == 0 || skeleton_ptr + 44 > data.len() {
        return Ok(None);
    }

    let bone_count = read_u32(data, skeleton_ptr + 8)? as usize;
    if bone_count == 0 || bone_count > 512 {
        return Ok(None);
    }

    let child_rel = read_u32(data, skeleton_ptr + 16)? as usize;
    let sibling_rel = read_u32(data, skeleton_ptr + 20)? as usize;
    let inv_bind_rel = read_u32(data, skeleton_ptr + 24)? as usize;
    let bind_rel = read_u32(data, skeleton_ptr + 28)? as usize;
    let aux_a_rel = read_u32(data, skeleton_ptr + 32)? as usize;
    let aux_b_rel = read_u32(data, skeleton_ptr + 36)? as usize;
    let name_table_rel = read_u32(data, skeleton_ptr + 40)? as usize;

    if child_rel == 0 || sibling_rel == 0 || inv_bind_rel == 0 || bind_rel == 0 || name_table_rel == 0 {
        return Ok(None);
    }

    let child_off = skeleton_ptr + child_rel;
    let sibling_off = skeleton_ptr + sibling_rel;
    let inv_bind_off = skeleton_ptr + inv_bind_rel;
    let bind_off = skeleton_ptr + bind_rel;
    let aux_a_off = if aux_a_rel != 0 { (skeleton_ptr + aux_a_rel) as u32 } else { 0 };
    let aux_b_off = if aux_b_rel != 0 { (skeleton_ptr + aux_b_rel) as u32 } else { 0 };
    let name_table_off = (skeleton_ptr + name_table_rel) as u32;

    if child_off + bone_count * 4 > data.len()
        || sibling_off + bone_count * 4 > data.len()
        || inv_bind_off + bone_count * 64 > data.len()
        || bind_off + bone_count * 64 > data.len()
        || name_table_off as usize + bone_count * 4 > data.len()
    {
        return Ok(None);
    }

    let mut first_child = Vec::with_capacity(bone_count);
    let mut next_sibling = Vec::with_capacity(bone_count);
    for i in 0..bone_count {
        first_child.push(read_u32(data, child_off + i * 4)?);
        next_sibling.push(read_u32(data, sibling_off + i * 4)?);
    }

    let mut bind_matrices = Vec::with_capacity(bone_count);
    let mut inverse_bind_matrices = Vec::with_capacity(bone_count);
    for i in 0..bone_count {
        bind_matrices.push(read_f32x16(data, bind_off + i * 64)?);
        inverse_bind_matrices.push(read_f32x16(data, inv_bind_off + i * 64)?);
    }

    let mut names = Vec::with_capacity(bone_count);
    for i in 0..bone_count {
        let rel = read_u32(data, name_table_off as usize + i * 4)? as usize;
        let abs_off = skeleton_ptr + rel;

        if abs_off < skeleton_ptr || abs_off >= data.len() {
            names.push(format!("bone_{:03}", i));
            continue;
        }

        let end = data[abs_off..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| abs_off + p)
            .unwrap_or(data.len());

        let raw = &data[abs_off..end];
        let name = std::str::from_utf8(raw).unwrap_or("").trim().to_string();
        if name.is_empty() {
            names.push(format!("bone_{:03}", i));
        } else {
            names.push(name);
        }
    }

    let parent = build_parent_from_child_sibling(&first_child, &next_sibling);

    let skin_ptr = read_u32(data, 0x9C)? as usize;
    let weights = if skin_ptr != 0 && skin_ptr + vertex_count * SKIN_RECORD_STRIDE <= data.len() {
        let mut all = Vec::with_capacity(vertex_count);
        for i in 0..vertex_count {
            let off = skin_ptr + i * SKIN_RECORD_STRIDE;
            let raw_indices = &data[off..off + 4];
            let raw_weights = [
                read_f32(data, off + 4)?,
                read_f32(data, off + 8)?,
                read_f32(data, off + 12)?,
                read_f32(data, off + 16)?,
            ];

            let mut influences = Vec::new();
            for j in 0..4 {
                let raw_idx = raw_indices[j] as usize;
                let weight = raw_weights[j];
                if weight <= EPSILON {
                    continue;
                }
                if raw_idx % 3 != 0 {
                    continue;
                }
                let bone_index = raw_idx / 3;
                if bone_index >= bone_count {
                    continue;
                }
                influences.push(GeoInfluence { bone_index, weight });
            }
            all.push(influences);
        }
        Some(all)
    } else {
        None
    };

    Ok(Some(GeoSkeleton {
        skeleton_ptr: skeleton_ptr as u32,
        bone_count,
        names,
        first_child,
        next_sibling,
        parent,
        bind_matrices,
        inverse_bind_matrices,
        aux_a_off,
        aux_b_off,
        name_table_off,
        weights,
    }))
}

fn build_parent_from_child_sibling(first_child: &[u32], next_sibling: &[u32]) -> Vec<Option<usize>> {
    let count = first_child.len();
    let mut parent = vec![None; count];
    let mut visited = vec![false; count];

    fn visit(
        node: usize,
        parent_index: Option<usize>,
        first_child: &[u32],
        next_sibling: &[u32],
        visited: &mut [bool],
        parent: &mut [Option<usize>],
    ) {
        let mut cur = node;
        loop {
            if cur >= visited.len() || visited[cur] {
                return;
            }

            visited[cur] = true;
            parent[cur] = parent_index;

            let child = first_child[cur] as usize;
            if child != 0 && child < visited.len() {
                visit(child, Some(cur), first_child, next_sibling, visited, parent);
            }

            let sibling = next_sibling[cur] as usize;
            if sibling == 0 || sibling >= visited.len() {
                break;
            }

            cur = sibling;
        }
    }

    if count > 0 {
        visit(0, None, first_child, next_sibling, &mut visited, &mut parent);
    }

    parent
}

fn summarize_weight_profile(skeleton: Option<&GeoSkeleton>, faces: &[[u16; 3]]) -> GeoWeightProfile {
    let Some(skeleton) = skeleton else {
        return GeoWeightProfile::default();
    };

    let Some(weights) = skeleton.weights.as_ref() else {
        return GeoWeightProfile::default();
    };

    let mut summary = GeoWeightProfile {
        has_weights: true,
        weighted_vertex_count: weights.len(),
        ..Default::default()
    };

    let mut dominant_names: Vec<Option<String>> = vec![None; weights.len()];
    let mut dominant_counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    for (vertex_index, influences) in weights.iter().enumerate() {
        let influence_count = influences.len();
        summary.max_influences_per_vertex = summary.max_influences_per_vertex.max(influence_count);

        if influence_count == 0 {
            continue;
        } else if influence_count == 1 {
            summary.single_influence_vertices += 1;
        } else {
            summary.multi_influence_vertices += 1;
        }

        if let Some(best) = influences
            .iter()
            .max_by(|a, b| a.weight.partial_cmp(&b.weight).unwrap_or(std::cmp::Ordering::Equal))
        {
            let name = skeleton
                .names
                .get(best.bone_index)
                .cloned()
                .unwrap_or_else(|| format!("bone_{:03}", best.bone_index));
            dominant_names[vertex_index] = Some(name.clone());
            *dominant_counts.entry(name).or_insert(0) += 1;
        }
    }

    summary.rigid_single_influence =
        summary.weighted_vertex_count > 0 && summary.single_influence_vertices == summary.weighted_vertex_count;

    for face in faces {
        let mut set = std::collections::BTreeSet::new();
        for &vi in face {
            if let Some(Some(name)) = dominant_names.get(vi as usize) {
                set.insert(name.clone());
            }
        }

        if set.len() == 1 {
            summary.single_bone_faces += 1;
        } else if set.len() > 1 {
            summary.mixed_bone_faces += 1;
        }
    }

    summary.rigid_face_partition = !faces.is_empty() && summary.single_bone_faces == faces.len();

    let mut counts: Vec<(String, usize)> = dominant_counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase())));
    summary.dominant_bone_vertex_counts = counts;

    summary
}

fn classify_geo_asset(skeleton: Option<&GeoSkeleton>, profile: &GeoWeightProfile) -> GeoAssetType {
    if skeleton.is_none() {
        GeoAssetType::MeshOnly
    } else if profile.rigid_single_influence && profile.rigid_face_partition {
        GeoAssetType::RigidProp
    } else {
        GeoAssetType::SkinnedMesh
    }
}

fn extract_ascii_strings(data: &[u8], min_len: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = Vec::new();

    for &b in data {
        if (32..127).contains(&b) {
            buf.push(b);
        } else {
            if buf.len() >= min_len {
                if let Ok(s) = std::str::from_utf8(&buf) {
                    out.push(s.to_string());
                }
            }
            buf.clear();
        }
    }

    if buf.len() >= min_len {
        if let Ok(s) = std::str::from_utf8(&buf) {
            out.push(s.to_string());
        }
    }

    out
}

fn read_u16(data: &[u8], off: usize) -> Result<u16> {
    let bytes: [u8; 2] = data
        .get(off..off + 2)
        .context("Unexpected end of GEO file")?
        .try_into()
        .unwrap();
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(data: &[u8], off: usize) -> Result<u32> {
    let bytes: [u8; 4] = data
        .get(off..off + 4)
        .context("Unexpected end of GEO file")?
        .try_into()
        .unwrap();
    Ok(u32::from_le_bytes(bytes))
}

fn read_f32(data: &[u8], off: usize) -> Result<f32> {
    let bytes: [u8; 4] = data
        .get(off..off + 4)
        .context("Unexpected end of GEO file")?
        .try_into()
        .unwrap();
    Ok(f32::from_le_bytes(bytes))
}

fn read_bytes4(data: &[u8], off: usize) -> Result<[u8; 4]> {
    Ok(data
        .get(off..off + 4)
        .context("Unexpected end of GEO file")?
        .try_into()
        .unwrap())
}

fn read_f32x16(data: &[u8], off: usize) -> Result<[f32; 16]> {
    let mut out = [0.0f32; 16];
    for i in 0..16 {
        out[i] = read_f32(data, off + i * 4)?;
    }
    Ok(out)
}