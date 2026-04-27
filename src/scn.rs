use anyhow::{anyhow, bail, Context, Result};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

const MATRIX_STRIDE: usize = 64;
const SCN_VERTEX_STRIDE: usize = 36;
const SCN_RECORD_STRIDE: usize = 0x68;
const TRAILER_STRIDE: usize = 12;

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ScnHeader {
    pub version: u32,
    pub unk_04: u32,
    pub remap_count: usize,
    pub unk_0c: u32,
    pub record_table_offset: u32,
    pub node_count: usize,
    pub names_offset: u32,
    pub transforms_offset: u32,
    pub archetypes_offset: u32,
    pub flags_offset: u32,
    pub unk_28: u32,
    pub file_size_minus_trailer: u32,
    pub secondary_transform_count: usize,
    pub secondary_transform_offset: u32,
}

#[derive(Clone, Debug)]
pub struct ScnNode {
    pub index: usize,
    pub record_offset: u32,
    pub name: String,
    pub archetype: String,
    pub flags: u16,
    pub transform: [f32; 16],
    pub translation: [f32; 3],
}

impl ScnNode {
    pub fn is_marker(&self) -> bool {
        self.archetype.trim().is_empty()
    }

    pub fn archetype_label(&self) -> &str {
        let trimmed = self.archetype.trim();
        if trimmed.is_empty() {
            "(marker)"
        } else {
            trimmed
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScnMeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
    pub uv: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct ScnTextureSpan {
    pub texture_slot: usize,
    pub mode: u32,
    pub index_start: usize,
    pub index_count: usize,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ScnMeshChunk {
    pub entry_index: usize,
    pub entry_offset: u32,
    pub record_kind: u32,
    pub vertex_offset: u32,
    pub index_offset: u32,
    pub vertex_count: usize,
    pub index_count: usize,
    pub transform_index: Option<usize>,
    pub transform: Option<[f32; 16]>,
    pub texture_names: Vec<String>,
    pub texture_spans: Vec<ScnTextureSpan>,
    pub vertices: Vec<ScnMeshVertex>,
    pub indices: Vec<u32>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct ScnFile {
    pub path: PathBuf,
    pub file_size: usize,
    pub header: ScnHeader,
    pub record_offsets: Vec<u32>,
    pub nodes: Vec<ScnNode>,
    pub secondary_transforms: Vec<[f32; 16]>,
    pub remap_pairs: Vec<(u32, u32)>,
    pub mesh_chunks: Vec<ScnMeshChunk>,
}

impl ScnFile {
    pub fn marker_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_marker()).count()
    }

    pub fn renderable_count(&self) -> usize {
        self.nodes.len().saturating_sub(self.marker_count())
    }

    pub fn embedded_mesh_chunk_count(&self) -> usize {
        self.mesh_chunks.len()
    }

    pub fn embedded_triangle_count(&self) -> usize {
        self.mesh_chunks
            .iter()
            .map(|chunk| chunk.indices.len() / 3)
            .sum()
    }

    pub fn top_archetypes(&self, limit: usize) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();

        for node in &self.nodes {
            let key = node.archetype_label().to_owned();
            *counts.entry(key).or_default() += 1;
        }

        let mut pairs: Vec<_> = counts.into_iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        pairs.truncate(limit);
        pairs
    }

    pub fn embedded_texture_name_count(&self) -> usize {
        let mut names = std::collections::BTreeSet::new();

        for chunk in &self.mesh_chunks {
            for name in &chunk.texture_names {
                names.insert(name.to_ascii_lowercase());
            }
        }

        names.len()
    }

    pub fn texture_span_mode_counts(&self) -> Vec<(u32, usize)> {
        let mut counts: BTreeMap<u32, usize> = BTreeMap::new();

        for chunk in &self.mesh_chunks {
            for span in &chunk.texture_spans {
                *counts.entry(span.mode).or_default() += 1;
            }
        }

        counts.into_iter().collect()
    }

    pub fn texture_span_count(&self) -> usize {
        self.mesh_chunks
            .iter()
            .map(|chunk| chunk.texture_spans.len())
            .sum()
    }
}

pub fn load_scn(path: &Path) -> Result<ScnFile> {
    let data = fs::read(path)
        .with_context(|| format!("Failed to read SCN file {}", path.display()))?;

    if data.len() < 0x38 + TRAILER_STRIDE {
        bail!("SCN file is too small");
    }

    let version = read_u32(&data, 0x00)?;
    let unk_04 = read_u32(&data, 0x04)?;
    let remap_count = read_u32(&data, 0x08)? as usize;
    let unk_0c = read_u32(&data, 0x0C)?;
    let record_table_offset = read_u32(&data, 0x10)?;
    let raw_node_count = read_u32(&data, 0x14)? as usize;
    let names_offset = read_u32(&data, 0x18)?;
    let transforms_offset = read_u32(&data, 0x1C)?;
    let archetypes_offset = read_u32(&data, 0x20)?;
    let flags_offset = read_u32(&data, 0x24)?;
    let unk_28 = read_u32(&data, 0x28)?;
    let file_size_minus_trailer = read_u32(&data, 0x2C)?;
    let secondary_transform_count = read_u32(&data, 0x30)? as usize;
    let secondary_transform_offset = read_u32(&data, 0x34)?;

    let file_size = data.len();

    if file_size_minus_trailer as usize + TRAILER_STRIDE != file_size {
        bail!(
            "SCN header file_size_minus_trailer does not match actual file size (header says {}, file is {})",
            file_size_minus_trailer,
            file_size
        );
    }

    if !(record_table_offset <= names_offset
        && names_offset <= transforms_offset
        && transforms_offset <= archetypes_offset
        && archetypes_offset <= flags_offset
        && flags_offset <= secondary_transform_offset)
    {
        bail!("SCN header offsets are not in the expected ascending order");
    }

    let record_table_off = record_table_offset as usize;
    let names_off = names_offset as usize;
    let transforms_off = transforms_offset as usize;
    let archetypes_off = archetypes_offset as usize;
    let flags_off = flags_offset as usize;
    let secondary_transform_off = secondary_transform_offset as usize;

    let primary_transform_span = archetypes_off
        .checked_sub(transforms_off)
        .context("SCN primary transform span is invalid")?;

    if primary_transform_span % MATRIX_STRIDE != 0 {
        bail!("SCN primary transform span is not a whole number of matrices");
    }

    let node_count = primary_transform_span / MATRIX_STRIDE;

    let header = ScnHeader {
        version,
        unk_04,
        remap_count,
        unk_0c,
        record_table_offset,
        node_count,
        names_offset,
        transforms_offset,
        archetypes_offset,
        flags_offset,
        unk_28,
        file_size_minus_trailer,
        secondary_transform_count,
        secondary_transform_offset,
    };

    let record_table_end = record_table_off + header.remap_count * 4;
    if record_table_end > file_size {
        bail!("SCN record table exceeds file size");
    }

    let transforms_end = transforms_off + header.node_count * MATRIX_STRIDE;
    if transforms_end > file_size {
        bail!("SCN primary transform table exceeds file size");
    }

    let flags_end = flags_off + header.node_count * 2;
    if flags_end > file_size {
        bail!("SCN flags table exceeds file size");
    }

    let secondary_transforms_end =
        secondary_transform_off + header.secondary_transform_count * MATRIX_STRIDE;
    if secondary_transforms_end > file_size {
        bail!("SCN secondary transform table exceeds file size");
    }

    let remap_pairs_offset = secondary_transforms_end;
    let remap_pairs_end = remap_pairs_offset + header.remap_count * 8;

    if remap_pairs_end + TRAILER_STRIDE != file_size {
        bail!("SCN remap pair table does not line up with the file trailer");
    }

    let trailer_zero = read_u32(&data, file_size - 12)?;
    let trailer_remap_count = read_u32(&data, file_size - 8)? as usize;
    let trailer_remap_offset = read_u32(&data, file_size - 4)? as usize;

    if trailer_zero != 0 {
        bail!("SCN trailer first word is not zero");
    }

    if trailer_remap_count != header.remap_count {
        bail!(
            "SCN trailer remap count ({}) does not match header remap count ({})",
            trailer_remap_count,
            header.remap_count
        );
    }

    if trailer_remap_offset != remap_pairs_offset {
        bail!(
            "SCN trailer remap offset (0x{:08X}) does not match computed remap offset (0x{:08X})",
            trailer_remap_offset,
            remap_pairs_offset
        );
    }

    let mut mesh_entry_offsets = Vec::with_capacity(header.remap_count);
    for i in 0..header.remap_count {
        mesh_entry_offsets.push(read_u32(&data, record_table_off + i * 4)?);
    }

    let names = parse_exact_string_table(&data, names_off, transforms_off, header.node_count)
        .context("Failed to parse SCN instance name table")?;

    let archetypes =
        parse_exact_string_table(&data, archetypes_off, flags_off, header.node_count)
            .context("Failed to parse SCN archetype name table")?;

    let mut flags = Vec::with_capacity(header.node_count);
    for i in 0..header.node_count {
        flags.push(read_u16(&data, flags_off + i * 2)?);
    }

    let mut transforms = Vec::with_capacity(header.node_count);
    for i in 0..header.node_count {
        transforms.push(read_matrix(&data, transforms_off + i * MATRIX_STRIDE)?);
    }

    let mut secondary_transforms = Vec::with_capacity(header.secondary_transform_count);
    for i in 0..header.secondary_transform_count {
        secondary_transforms.push(read_matrix(
            &data,
            secondary_transform_off + i * MATRIX_STRIDE,
        )?);
    }

    let mut remap_pairs = Vec::with_capacity(header.remap_count);
    for i in 0..header.remap_count {
        let off = remap_pairs_offset + i * 8;
        remap_pairs.push((read_u32(&data, off)?, read_u32(&data, off + 4)?));
    }

    let entry_to_transform: HashMap<usize, usize> = remap_pairs
        .iter()
        .map(|(entry_index, transform_index)| (*entry_index as usize, *transform_index as usize))
        .collect();

    let mesh_chunks = parse_scn_mesh_chunks(
        &data,
        names_off,
        &mesh_entry_offsets,
        &entry_to_transform,
        &secondary_transforms,
    )?;

    let mut record_offsets = Vec::with_capacity(header.node_count);
    for i in 0..header.node_count {
        record_offsets.push(mesh_entry_offsets.get(i).copied().unwrap_or(0));
    }

    let mut nodes = Vec::with_capacity(header.node_count);
    for i in 0..header.node_count {
        let transform = transforms[i];
        nodes.push(ScnNode {
            index: i,
            record_offset: record_offsets[i],
            name: names[i].clone(),
            archetype: archetypes[i].clone(),
            flags: flags[i],
            translation: [transform[12], transform[13], transform[14]],
            transform,
        });
    }

    if raw_node_count != header.node_count {
        // Keep going. These files still load correctly from the transform span,
        // which is the more trustworthy count for the node tables.
    }

    Ok(ScnFile {
        path: path.to_path_buf(),
        file_size,
        header,
        record_offsets,
        nodes,
        secondary_transforms,
        remap_pairs,
        mesh_chunks,
    })
}

fn parse_scn_mesh_chunks(
    data: &[u8],
    mesh_region_end: usize,
    entry_offsets: &[u32],
    entry_to_transform: &HashMap<usize, usize>,
    secondary_transforms: &[[f32; 16]],
) -> Result<Vec<ScnMeshChunk>> {
    let mut out = Vec::new();

    for (entry_index, &entry_offset_u32) in entry_offsets.iter().enumerate() {
        let entry_offset = entry_offset_u32 as usize;

        if entry_offset + SCN_RECORD_STRIDE > data.len() {
            continue;
        }

        let record_group = read_u32(data, entry_offset)?;
        let record_kind = read_u32(data, entry_offset + 4)?;

        if record_group != 0 || record_kind == 0 {
            continue;
        }

        let vertex_offset = read_u32(data, entry_offset + 0x18)? as usize;
        let index_offset = read_u32(data, entry_offset + 0x1C)? as usize;
        let vertex_count = read_u32(data, entry_offset + 0x3C)? as usize;
        let index_count = read_u32(data, entry_offset + 0x64)? as usize;
        let material_ptr = read_u32(data, entry_offset + 0x0C)? as usize;
        let texture_names = parse_scn_texture_names(data, material_ptr);
        let texture_span_ptr = read_u32(data, entry_offset + 0x44)? as usize;

        if vertex_count == 0 || index_count < 3 || index_count % 3 != 0 {
            continue;
        }

        if !(entry_offset < vertex_offset
            && vertex_offset < index_offset
            && index_offset < mesh_region_end)
        {
            continue;
        }

        if index_offset - vertex_offset < vertex_count * SCN_VERTEX_STRIDE {
            continue;
        }

        if index_offset + index_count * 2 > mesh_region_end {
            continue;
        }

        let mut vertices = Vec::with_capacity(vertex_count);
        for i in 0..vertex_count {
            let off = vertex_offset + i * SCN_VERTEX_STRIDE;
            vertices.push(ScnMeshVertex {
                position: [
                    read_f32(data, off)?,
                    read_f32(data, off + 4)?,
                    read_f32(data, off + 8)?,
                ],
                normal: [
                    read_f32(data, off + 12)?,
                    read_f32(data, off + 16)?,
                    read_f32(data, off + 20)?,
                ],
                color: [
                    data.get(off + 24).copied().unwrap_or(255),
                    data.get(off + 25).copied().unwrap_or(255),
                    data.get(off + 26).copied().unwrap_or(255),
                    data.get(off + 27).copied().unwrap_or(255),
                ],
                uv: [read_f32(data, off + 28)?, read_f32(data, off + 32)?],
            });
        }

        let mut indices = Vec::with_capacity(index_count);
        let mut in_range = true;

        for i in 0..index_count {
            let idx = read_u16(data, index_offset + i * 2)? as u32;
            if idx as usize >= vertex_count {
                in_range = false;
                break;
            }
            indices.push(idx);
        }

        if !in_range {
            continue;
        }

        let texture_spans = parse_scn_texture_spans(
            data,
            texture_span_ptr,
            record_kind as usize,
            index_count,
        );

        let transform_index = entry_to_transform.get(&entry_index).copied();
        let transform = transform_index.and_then(|i| secondary_transforms.get(i).copied());

        out.push(ScnMeshChunk {
            entry_index,
            entry_offset: entry_offset_u32,
            record_kind,
            vertex_offset: vertex_offset as u32,
            index_offset: index_offset as u32,
            vertex_count,
            index_count,
            transform_index,
            transform,
            texture_names,
            texture_spans,
            vertices,
            indices,
        });
    }

    Ok(out)
}

fn parse_exact_string_table(
    data: &[u8],
    start: usize,
    end: usize,
    count: usize,
) -> Result<Vec<String>> {
    if start > end || end > data.len() {
        bail!("Invalid SCN string table range");
    }

    if count == 0 {
        return Ok(Vec::new());
    }

    let mut out = Vec::with_capacity(count);
    let mut current = Vec::new();

    for &byte in &data[start..end] {
        if byte == 0 {
            out.push(String::from_utf8_lossy(&current).to_string());
            current.clear();

            if out.len() == count {
                return Ok(out);
            }
        } else {
            current.push(byte);
        }
    }

    bail!(
        "SCN string table ended early: expected {} strings, got {}",
        count,
        out.len()
    );
}

fn parse_scn_texture_names(data: &[u8], material_ptr: usize) -> Vec<String> {
    let mut out = Vec::new();

    if material_ptr >= data.len() {
        return out;
    }

    for i in 0..16 {
        let ptr_off = material_ptr + i * 4;
        if ptr_off + 4 > data.len() {
            break;
        }

        let name_ptr = match read_u32(data, ptr_off) {
            Ok(v) => v as usize,
            Err(_) => break,
        };

        if name_ptr == 0 || name_ptr >= data.len() {
            break;
        }

        let Some(name) = read_ascii_cstring(data, name_ptr, 128).ok() else {
            break;
        };

        let trimmed = name.trim();
        if trimmed.is_empty() {
            break;
        }

        // Important:
        // SCN texture spans refer to material *slot indices*.
        // Some slots are not DDS textures, for example "park0_white_3116"
        // or "level1_white_1582". Do not break or compact the list here,
        // otherwise later DDS slots shift and render with the wrong texture.
        if trimmed.to_ascii_lowercase().ends_with(".dds") {
            out.push(trimmed.to_string());
        } else {
            out.push(String::new());
        }
    }

    // Keep internal empty slots, but remove useless trailing placeholders.
    while out.last().map(|name| name.trim().is_empty()).unwrap_or(false) {
        out.pop();
    }

    out
}

fn parse_scn_texture_spans(
    data: &[u8],
    table_ptr: usize,
    span_count: usize,
    total_index_count: usize,
) -> Vec<ScnTextureSpan> {
    let mut out = Vec::new();

    if table_ptr >= data.len() || span_count == 0 {
        out.push(ScnTextureSpan {
            texture_slot: 0,
            mode: 0,
            index_start: 0,
            index_count: total_index_count,
        });
        return out;
    }

    for i in 0..span_count {
        let off = table_ptr + i * 16;
        if off + 16 > data.len() {
            break;
        }

        let texture_slot = match read_u32(data, off) {
            Ok(v) => v as usize,
            Err(_) => break,
        };
        let mode = match read_u32(data, off + 4) {
            Ok(v) => v,
            Err(_) => break,
        };
        let index_start = match read_u32(data, off + 8) {
            Ok(v) => v as usize,
            Err(_) => break,
        };
        let index_count = match read_u32(data, off + 12) {
            Ok(v) => v as usize,
            Err(_) => break,
        };

        if index_count == 0 {
            continue;
        }

        if index_start >= total_index_count {
            continue;
        }

        if index_start + index_count > total_index_count {
            continue;
        }

        out.push(ScnTextureSpan {
            texture_slot,
            mode,
            index_start,
            index_count,
        });
    }

    if out.is_empty() {
        out.push(ScnTextureSpan {
            texture_slot: 0,
            mode: 0,
            index_start: 0,
            index_count: total_index_count,
        });
    }

    out
}

fn read_ascii_cstring(data: &[u8], off: usize, max_len: usize) -> Result<String> {
    if off >= data.len() {
        bail!("String pointer out of range");
    }

    let mut end = off;
    while end < data.len() && end - off < max_len {
        let b = data[end];
        if b == 0 {
            break;
        }
        if !(0x20..=0x7E).contains(&b) {
            bail!("String contains non-ASCII bytes");
        }
        end += 1;
    }

    if end == off {
        bail!("Empty string");
    }

    Ok(String::from_utf8_lossy(&data[off..end]).to_string())
}

fn read_matrix(data: &[u8], off: usize) -> Result<[f32; 16]> {
    if off + MATRIX_STRIDE > data.len() {
        bail!("Matrix read exceeds file size");
    }

    let mut out = [0.0f32; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = read_f32(data, off + i * 4)?;
    }
    Ok(out)
}

fn read_u16(data: &[u8], off: usize) -> Result<u16> {
    let bytes = data
        .get(off..off + 2)
        .ok_or_else(|| anyhow!("u16 read out of range at 0x{:X}", off))?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], off: usize) -> Result<u32> {
    let bytes = data
        .get(off..off + 4)
        .ok_or_else(|| anyhow!("u32 read out of range at 0x{:X}", off))?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_f32(data: &[u8], off: usize) -> Result<f32> {
    let bytes = data
        .get(off..off + 4)
        .ok_or_else(|| anyhow!("f32 read out of range at 0x{:X}", off))?;
Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

impl ScnFile {
    pub fn save_scn(&self, path: &Path) -> Result<()> {
        let source = &self.path;
        
        if !source.exists() {
            bail!("Source SCN file not found: {}", source.display());
        }
        
        let mut data = fs::read(source).with_context(|| format!("Failed to read source SCN"))?;
        
        let transforms_offset = read_u32(&data, 0x1C)? as usize;
        let node_count = read_u32(&data, 0x14)? as usize;

        if self.nodes.len() != node_count {
            bail!("Node count mismatch");
        }

        for (i, node) in self.nodes.iter().enumerate() {
            let off = transforms_offset + i * MATRIX_STRIDE;
            for (j, &val) in node.transform.iter().enumerate() {
                write_f32(&mut data, off + j * 4, val);
            }
        }

        let secondary_offset = read_u32(&data, 0x34)? as usize;
        let secondary_count = read_u32(&data, 0x30)? as usize;

        for chunk in &self.mesh_chunks {
            if let Some(transform) = chunk.transform {
                if let Some(idx) = chunk.transform_index {
                    if idx < secondary_count {
                        let off = secondary_offset + idx * MATRIX_STRIDE;
                        for (j, &val) in transform.iter().enumerate() {
                            write_f32(&mut data, off + j * 4, val);
                        }
                    }
                }
            }
        }

        fs::write(path, data).with_context(|| format!("Failed to write SCN"))?;

        Ok(())
    }

    pub fn update_node_transform(&mut self, index: usize, transform: [f32; 16]) {
        if index < self.nodes.len() {
            self.nodes[index].transform = transform;
            self.nodes[index].translation = [transform[12], transform[13], transform[14]];
        }
    }

    pub fn add_node(&mut self, name: String, archetype: String, transform: [f32; 16], flags: u16) {
        let new_node = ScnNode {
            index: self.nodes.len(),
            record_offset: 0,
            name,
            archetype,
            flags,
            transform,
            translation: [transform[12], transform[13], transform[14]],
        };
        self.nodes.push(new_node);
    }

    pub fn remove_node(&mut self, index: usize) -> Option<ScnNode> {
        if index < self.nodes.len() {
            Some(self.nodes.remove(index))
        } else {
            None
        }
    }
}

fn write_u16(data: &mut Vec<u8>, off: usize, val: u16) {
    let bytes = val.to_le_bytes();
    if off + 2 > data.len() {
        data.resize(off + 2, 0);
    }
    data[off] = bytes[0];
    data[off + 1] = bytes[1];
}

fn write_u32(data: &mut Vec<u8>, off: usize, val: u32) {
    let bytes = val.to_le_bytes();
    if off + 4 > data.len() {
        data.resize(off + 4, 0);
    }
    data[off] = bytes[0];
    data[off + 1] = bytes[1];
    data[off + 2] = bytes[2];
    data[off + 3] = bytes[3];
}

fn write_f32(data: &mut Vec<u8>, off: usize, val: f32) {
    let bytes = val.to_le_bytes();
    if off + 4 > data.len() {
        data.resize(off + 4, 0);
    }
    data[off] = bytes[0];
    data[off + 1] = bytes[1];
    data[off + 2] = bytes[2];
    data[off + 3] = bytes[3];
}