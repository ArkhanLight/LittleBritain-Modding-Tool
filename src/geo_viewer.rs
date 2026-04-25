use crate::{
    anm::{RigidAnimClip, RigidAnimStream},
    dds_preview::DdsPreview,
    geo::{GeoFile, GeoSkeleton},
    scn::{ScnFile, ScnMeshChunk, ScnNode},
};
use bytemuck::{Pod, Zeroable};
use eframe::{
    egui::{self, Color32, Rect, Sense, Vec2},
    egui_wgpu::{self, wgpu},
};
use glam::{Mat4, Quat, Vec3};
use std::{
    collections::{BTreeSet, HashMap, VecDeque, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
};
use wgpu::util::DeviceExt;

const NEAR_PLANE: f32 = 0.05;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

const MAX_MATERIAL_LAYERS: usize = 8;
const MAX_CACHED_GPU_SCENES: usize = 24;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialParams {
    layer_count: u32,
    record_kind: u32,
    _pad: [u32; 2],
}

pub struct GeoViewerState {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub pan: Vec2,
    pub eye: [f32; 3],
    pub move_speed: f32,
    pub show_faces: bool,
    pub show_textures: bool,
    pub show_wireframe: bool,
    pub show_bones: bool,
    pub show_helpers: bool,
    pub cull_backfaces: bool,
    scene_transform_mode: SceneTransformMode,
    scene_gizmo_drag: Option<SceneGizmoDrag>,
    scene_edit_revision: u64,

    scene_key: Option<String>,
    cpu_scene: Option<Arc<CpuScene>>,
    frame_targets: Arc<Mutex<Option<FrameTargets>>>,
}

impl Default for GeoViewerState {
    fn default() -> Self {
        Self {
            yaw: std::f32::consts::PI + 0.35,
            pitch: -0.35,
            distance: 10.0,
            pan: Vec2::ZERO,
            eye: [0.0, 0.0, 0.0],
            move_speed: 2.0,
            show_faces: true,
            show_textures: true,
            show_wireframe: false,
            show_bones: true,
            show_helpers: true,
            cull_backfaces: false,
            scene_transform_mode: SceneTransformMode::Translate,
            scene_gizmo_drag: None,
            scene_edit_revision: 0,
            scene_key: None,
            cpu_scene: None,
            frame_targets: Arc::new(Mutex::new(None)),
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct SceneGeoModel {
    pub archetype: String,
    pub path: PathBuf,
    pub geo: GeoFile,
    pub textures: Vec<Option<DdsPreview>>,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MeshVertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 4],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LineVertex {
    position: [f32; 3],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    light_dir: [f32; 4],
    render_opts: [f32; 4], // x = textures on/off, y = brightness multiplier
}

#[derive(Clone)]
struct CpuTexture {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PartClass {
    Solid,
    Helper,
}

struct CpuPart {
    indices: Vec<u32>,
    textures: Vec<CpuTexture>,
    class: PartClass,
    record_kind: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SceneTransformMode {
    Translate,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GizmoAxis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug)]
struct SceneGizmoDrag {
    mode: SceneTransformMode,
    axis: GizmoAxis,
    start_pointer: egui::Pos2,
    start_translation: Vec3,
    start_rotation: Quat,
    start_scale: Vec3,
    start_pointer_vec: Vec2,
    axis_world: Vec3,
    axis_screen_dir: Vec2,
    handle_world_len: f32,
    screen_origin: egui::Pos2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneSelection {
    Node(usize),
    MeshChunk(usize),
}

#[derive(Clone, Copy, Debug)]
struct ScenePickTarget {
    selection: SceneSelection,
    center: [f32; 3],
    radius: f32,
}

#[allow(dead_code)]
struct CpuScene {
    key: String,
    vertices: Vec<MeshVertex>,
    parts: Vec<CpuPart>,
    ground_lines: Vec<LineVertex>,
    wire_lines: Vec<LineVertex>,
    helper_wire_lines: Vec<LineVertex>,
    bone_lines: Vec<LineVertex>,
    center: [f32; 3],
    radius: f32,
    pick_targets: Vec<ScenePickTarget>,
}

struct GpuPart {
    index_buffer: wgpu::Buffer,
    index_count: u32,
    bind_group: wgpu::BindGroup,
    class: PartClass,
    _material_buffer: wgpu::Buffer,
    _texture_keepalive: Vec<wgpu::Texture>,
}

struct GpuLines {
    buffer: wgpu::Buffer,
    vertex_count: u32,
}

struct GpuScene {
    vertex_buffer: wgpu::Buffer,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    parts: Vec<GpuPart>,
    ground_lines: Option<GpuLines>,
    wire_lines: Option<GpuLines>,
    helper_wire_lines: Option<GpuLines>,
    bone_lines: Option<GpuLines>,
}

struct FrameTargets {
    width: u32,
    height: u32,
    _color_texture: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    blit_bind_group: wgpu::BindGroup,
}

struct GpuShared {
    face_pipeline_cull: wgpu::RenderPipeline,
    face_pipeline_no_cull: wgpu::RenderPipeline,
    line_pipeline_overlay: wgpu::RenderPipeline,
    bone_pipeline_overlay: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    globals_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    blit_layout: wgpu::BindGroupLayout,
    texture_sampler: wgpu::Sampler,
    blit_sampler: wgpu::Sampler,
    scenes: HashMap<String, Arc<GpuScene>>,
    scene_lru: VecDeque<String>,
    fallback_white: Option<(wgpu::Texture, wgpu::TextureView)>,
}

struct GeoGpuCallback {
    rect: Rect,
    scene: Arc<CpuScene>,
    yaw: f32,
    pitch: f32,
    eye: [f32; 3],
    show_faces: bool,
    show_textures: bool,
    show_wireframe: bool,
    show_bones: bool,
    show_helpers: bool,
    cull_backfaces: bool,
    frame_targets: Arc<Mutex<Option<FrameTargets>>>,
}

pub fn reset_geo_viewer(state: &mut GeoViewerState, geo: &GeoFile) {
    let (center, _radius) = geo_bounds(geo);
    state.yaw = std::f32::consts::PI + 0.35;
    state.pitch = -0.35;
    state.distance = geo_frame_distance(geo);
    state.pan = Vec2::ZERO;
    state.eye = eye_from_target(state.yaw, state.pitch, state.distance, center);
}

pub fn draw_geo_viewer(
    ui: &mut egui::Ui,
    geo: &GeoFile,
    textures: &[Option<DdsPreview>],
    rigid_clip: Option<&RigidAnimClip>,
    rigid_time_seconds: f32,
    rigid_anim_tag: Option<&str>,
    state: &mut GeoViewerState,
    viewer_height: f32,
) {
    ui.horizontal(|ui| {
        if ui.button("Reset view").clicked() {
            reset_geo_viewer(state, geo);
        }

        ui.checkbox(&mut state.show_faces, "Faces");
        ui.checkbox(&mut state.show_textures, "Textures");
        ui.checkbox(&mut state.show_wireframe, "Wireframe");

        if geo.skeleton.is_some() {
            ui.checkbox(&mut state.show_bones, "Bones");
        }

        ui.checkbox(&mut state.show_helpers, "Shadows Blobs / Decals");
        ui.checkbox(&mut state.cull_backfaces, "Cull");

        ui.separator();
        ui.add(
            egui::Slider::new(&mut state.move_speed, 0.1..=20.0)
                .text("Fly speed"),
        );

        ui.separator();
        ui.small("GPU viewport: RMB look | Ctrl+LMB orbit model | MMB pan | wheel zoom | WASD fly | Q/E up/down");
    });

    let desired_height = viewer_height.clamp(260.0, 900.0);
    let desired_size = egui::vec2(ui.available_width().max(200.0), desired_height);
    let (response, painter) = ui.allocate_painter(desired_size, Sense::click_and_drag());
    let rect = response.rect;

    painter.rect_filled(rect, 0.0, Color32::from_rgb(30, 30, 34));

    let (scene_center, scene_radius) = geo_bounds(geo);
    let min_distance = NEAR_PLANE + 0.01;
    let max_distance = (scene_radius * 6.0).max(25.0);
    apply_viewer_input(
        ui,
        &response,
        state,
        scene_radius,
        min_distance,
        max_distance,
        Some(scene_center),
    );

    let texture_state = texture_state_hash(textures);
    let rigid_frame = rigid_clip
        .map(|clip| rigid_frame_index(clip, rigid_time_seconds))
        .unwrap_or(0);
    let scene_key = format!(
        "{}#{}#{}#{}",
        geo.path.display(),
        texture_state,
        rigid_anim_tag.unwrap_or("-"),
        rigid_frame
    );

    if state.scene_key.as_deref() != Some(scene_key.as_str()) {
        state.cpu_scene = Some(build_cpu_scene(
            geo,
            textures,
            scene_key.clone(),
            rigid_clip,
            rigid_time_seconds,
        ));
        state.scene_key = Some(scene_key);
    }

    let Some(scene) = state.cpu_scene.clone() else {
        return;
    };

    let callback = egui_wgpu::Callback::new_paint_callback(
        rect,
        GeoGpuCallback {
            rect,
            scene: scene.clone(),
            yaw: state.yaw,
            pitch: state.pitch,
            eye: state.eye,
            show_faces: state.show_faces,
            show_textures: state.show_textures,
            show_wireframe: state.show_wireframe,
            show_bones: state.show_bones,
            show_helpers: state.show_helpers,
            cull_backfaces: state.cull_backfaces,
            frame_targets: state.frame_targets.clone(),
        },
    );

    ui.painter().add(callback);
}

pub fn reset_scene_viewer(state: &mut GeoViewerState, scn: &ScnFile) {
    let (center, _radius) = scn_bounds(scn);
    state.yaw = std::f32::consts::PI + 0.35;
    state.pitch = -0.55;
    state.distance = scn_frame_distance(scn);
    state.pan = Vec2::ZERO;
    state.eye = eye_from_target(state.yaw, state.pitch, state.distance, center);
    state.show_faces = true;
    state.show_textures = true;
    state.show_wireframe = false;
    state.show_bones = true;
    state.cull_backfaces = false;
    state.scene_gizmo_drag = None;
    state.scene_key = None;
    state.cpu_scene = None;
}

pub fn focus_scene_viewer_on_point(state: &mut GeoViewerState, world_pos: [f32; 3]) {
    let view_pos = to_view_space(world_pos);
    state.distance = state.distance.clamp(8.0, 250.0);
    state.pan = Vec2::ZERO;
    state.eye = eye_from_target(state.yaw, state.pitch, state.distance, view_pos);
}

pub fn draw_scene_viewer(
    ui: &mut egui::Ui,
    scn: &mut ScnFile,
    models: &[SceneGeoModel],
    embedded_textures: &HashMap<String, DdsPreview>,
    state: &mut GeoViewerState,
    viewer_height: f32,
    selected: Option<SceneSelection>,
    hidden_nodes: &BTreeSet<usize>,
    hidden_chunks: &BTreeSet<usize>,
) -> Option<SceneSelection> {
    ui.horizontal(|ui| {
        if ui.button("Reset view").clicked() {
            reset_scene_viewer(state, scn);
        }

        ui.checkbox(&mut state.show_faces, "Faces");
        ui.checkbox(&mut state.show_textures, "Textures");
        ui.checkbox(&mut state.show_wireframe, "Wireframe");
        ui.checkbox(&mut state.show_bones, "Markers");
        ui.checkbox(&mut state.show_helpers, "Shadows Blobs / Decals");
        ui.checkbox(&mut state.cull_backfaces, "Cull");

        ui.separator();
        ui.add(egui::Slider::new(&mut state.move_speed, 0.1..=20.0).text("Fly speed"));

        ui.separator();
        ui.small(
            "SCN 3D: RMB look | Ctrl+LMB look | MMB pan | wheel dolly | WASD fly | Q/E up/down | gizmo via Move/Rotate/Scale",
        );
    });

    let desired_height = viewer_height.clamp(260.0, 900.0);
    let desired_size = egui::vec2(ui.available_width().max(200.0), desired_height);
    let (response, painter) = ui.allocate_painter(desired_size, Sense::click_and_drag());
    let rect = response.rect;

    painter.rect_filled(rect, 0.0, Color32::from_rgb(30, 30, 34));

    if !selected_scene_is_visible(selected, hidden_nodes, hidden_chunks) {
        state.scene_gizmo_drag = None;
    }

    let scene_radius = scn_bounds(scn).1;
    let min_distance = 5.0;
    let max_distance = (scene_radius * 8.0).max(600.0);
    let model_texture_state: usize = models
        .iter()
        .map(|m| m.textures.iter().filter(|t| t.is_some()).count())
        .sum();

    let scene_key = format!(
        "scn:{}#nodes:{}#models:{}#embtex:{}#mdltex:{}#hidden_nodes:{:016x}#hidden_chunks:{:016x}#selection:{}#edit:{}",
        scn.path.display(),
        scn.nodes.len(),
        models.len(),
        embedded_textures.len(),
        model_texture_state,
        visibility_set_hash(hidden_nodes),
        visibility_set_hash(hidden_chunks),
        scene_selection_cache_tag(selected),
        state.scene_edit_revision,
    );

    if state.scene_key.as_deref() != Some(scene_key.as_str()) {
        state.cpu_scene = Some(build_cpu_scene_from_scn(
            scn,
            models,
            embedded_textures,
            hidden_nodes,
            hidden_chunks,
            selected,
            scene_key.clone(),
        ));
        state.scene_key = Some(scene_key);
    }

    let Some(scene) = state.cpu_scene.clone() else {
        return None;
    };

    let mut pick_view_proj = make_view_proj(
        rect.width().round().max(1.0) as u32,
        rect.height().round().max(1.0) as u32,
        state.yaw,
        state.pitch,
        state.eye,
        &scene,
    );

    let toolbar_consumed = interact_scene_gizmo_toolbar(ui, rect, state);
    let gizmo_consumed = handle_scene_gizmo_interaction(
        ui,
        &response,
        rect,
        pick_view_proj,
        scn,
        state,
        selected,
        hidden_nodes,
        hidden_chunks,
        scene_radius,
    );
    let block_viewer_input = toolbar_consumed || gizmo_consumed || state.scene_gizmo_drag.is_some();

    let pointer_pick = if !block_viewer_input
        && !ui.input(|i| i.modifiers.ctrl)
        && response.clicked_by(egui::PointerButton::Primary)
    {
        response.interact_pointer_pos().and_then(|pointer_pos| {
            pick_scene_target(&scene, rect, pick_view_proj, state.eye, pointer_pos)
        })
    } else {
        None
    };

    if !block_viewer_input {
        apply_viewer_input(
            ui,
            &response,
            state,
            scene_radius,
            min_distance,
            max_distance,
            None,
        );

        pick_view_proj = make_view_proj(
            rect.width().round().max(1.0) as u32,
            rect.height().round().max(1.0) as u32,
            state.yaw,
            state.pitch,
            state.eye,
            &scene,
        );
    }

    let callback = egui_wgpu::Callback::new_paint_callback(
        rect,
        GeoGpuCallback {
            rect,
            scene: scene.clone(),
            yaw: state.yaw,
            pitch: state.pitch,
            eye: state.eye,
            show_faces: state.show_faces,
            show_textures: state.show_textures,
            show_wireframe: state.show_wireframe,
            show_bones: state.show_bones,
            show_helpers: state.show_helpers,
            cull_backfaces: state.cull_backfaces,
            frame_targets: state.frame_targets.clone(),
        },
    );

    painter.add(callback);
    paint_scene_gizmo(
        &painter,
        rect,
        pick_view_proj,
        scn,
        state,
        selected,
        hidden_nodes,
        hidden_chunks,
        scene_radius,
    );
    paint_scene_gizmo_toolbar(&painter, rect, state);

    pointer_pick
}

impl egui_wgpu::CallbackTrait for GeoGpuCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let surface_format = callback_resources
            .get::<egui_wgpu::RenderState>()
            .map(|rs| rs.target_format)
            .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);

        if !callback_resources.contains::<GpuShared>() {
            callback_resources.insert(GpuShared::new(device, surface_format));
        }

        let shared = callback_resources
            .get_mut::<GpuShared>()
            .expect("GpuShared should exist");

        let gpu_scene = if let Some(scene) = shared.scenes.get(&self.scene.key) {
            if let Some(index) = shared
                .scene_lru
                .iter()
                .position(|key| key == &self.scene.key)
            {
                shared.scene_lru.remove(index);
            }
            shared.scene_lru.push_back(self.scene.key.clone());
            scene.clone()
        } else {
            while shared.scenes.len() >= MAX_CACHED_GPU_SCENES {
                let Some(evicted) = shared.scene_lru.pop_front() else {
                    break;
                };
                shared.scenes.remove(&evicted);
            }

            let uploaded = Arc::new(upload_scene(device, queue, shared, &self.scene));
            shared
                .scenes
                .insert(self.scene.key.clone(), uploaded.clone());
            shared.scene_lru.push_back(self.scene.key.clone());
            uploaded
        };

        let width = (self.rect.width() * screen_descriptor.pixels_per_point)
            .round()
            .max(1.0) as u32;
        let height = (self.rect.height() * screen_descriptor.pixels_per_point)
            .round()
            .max(1.0) as u32;

        let mut frame_guard = self
            .frame_targets
            .lock()
            .expect("frame target mutex poisoned");
        let recreate = frame_guard
            .as_ref()
            .map(|f| f.width != width || f.height != height)
            .unwrap_or(true);
        if recreate {
            *frame_guard = Some(create_frame_targets(device, shared, width, height));
        }
        let frame = frame_guard.as_ref().expect("frame targets should exist");

        let view_proj = make_view_proj(width, height, self.yaw, self.pitch, self.eye, &self.scene);
        let globals = Globals {
            view_proj: view_proj.to_cols_array_2d(),
            light_dir: normalize4([-0.20, 0.90, -0.35, 0.0]),
            render_opts: [
                if self.show_textures { 1.0 } else { 0.0 },
                2.0, // brightness multiplier
                0.0,
                0.0,
            ],
        };
        queue.write_buffer(&gpu_scene.globals_buffer, 0, bytemuck::bytes_of(&globals));

        {
            let mut pass = egui_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("geo_viewer_offscreen_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 52.0 / 255.0,
                            g: 52.0 / 255.0,
                            b: 58.0 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &frame.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if self.show_faces && !gpu_scene.parts.is_empty() {
                let pipeline = if self.cull_backfaces {
                    &shared.face_pipeline_cull
                } else {
                    &shared.face_pipeline_no_cull
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &gpu_scene.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, gpu_scene.vertex_buffer.slice(..));

                for part in &gpu_scene.parts {
                    if !self.show_helpers && matches!(part.class, PartClass::Helper) {
                        continue;
                    }

                    pass.set_bind_group(1, &part.bind_group, &[]);
                    pass.set_index_buffer(part.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..part.index_count, 0, 0..1);
                }
            }

            if self.show_wireframe {
                if let Some(wire) = &gpu_scene.wire_lines {
                    pass.set_pipeline(&shared.line_pipeline_overlay);
                    pass.set_bind_group(0, &gpu_scene.globals_bind_group, &[]);
                    pass.set_vertex_buffer(0, wire.buffer.slice(..));
                    pass.draw(0..wire.vertex_count, 0..1);
                }
            }

            if self.show_helpers {
                if let Some(wire) = &gpu_scene.helper_wire_lines {
                    pass.set_pipeline(&shared.line_pipeline_overlay);
                    pass.set_bind_group(0, &gpu_scene.globals_bind_group, &[]);
                    pass.set_vertex_buffer(0, wire.buffer.slice(..));
                    pass.draw(0..wire.vertex_count, 0..1);
                }
            }

            if self.show_bones {
                if let Some(bones) = &gpu_scene.bone_lines {
                    pass.set_pipeline(&shared.bone_pipeline_overlay);
                    pass.set_bind_group(0, &gpu_scene.globals_bind_group, &[]);
                    pass.set_vertex_buffer(0, bones.buffer.slice(..));
                    pass.draw(0..bones.vertex_count, 0..1);
                }
            }

            if let Some(ground) = &gpu_scene.ground_lines {
                pass.set_pipeline(&shared.line_pipeline_overlay);
                pass.set_bind_group(0, &gpu_scene.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, ground.buffer.slice(..));
                pass.draw(0..ground.vertex_count, 0..1);
            }
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(shared) = callback_resources.get::<GpuShared>() else {
            return;
        };
        let Ok(frame_guard) = self.frame_targets.lock() else {
            return;
        };
        let Some(frame) = frame_guard.as_ref() else {
            return;
        };

        render_pass.set_pipeline(&shared.blit_pipeline);
        render_pass.set_bind_group(0, &frame.blit_bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

impl GpuShared {
    fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geo_globals_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let mut material_entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ];

        for i in 0..MAX_MATERIAL_LAYERS {
            material_entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 + i as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            });
        }

        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geo_material_layout"),
            entries: &material_entries,
        });

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geo_blit_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
            ],
        });

        let texture_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("geo_texture_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let blit_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("geo_blit_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let face_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geo_face_shader"),
            source: wgpu::ShaderSource::Wgsl(FACE_SHADER.into()),
        });
        let line_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geo_line_shader"),
            source: wgpu::ShaderSource::Wgsl(LINE_SHADER.into()),
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geo_blit_shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });

        let face_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geo_face_pipeline_layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geo_line_pipeline_layout"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geo_blit_pipeline_layout"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });

        let face_pipeline_cull =
            create_face_pipeline(device, &face_layout, &face_shader, Some(wgpu::Face::Back));
        let face_pipeline_no_cull = create_face_pipeline(device, &face_layout, &face_shader, None);
        let line_pipeline_overlay = create_line_pipeline(device, &line_layout, &line_shader);
        let bone_pipeline_overlay = create_bone_pipeline(device, &line_layout, &line_shader);

        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("geo_blit_pipeline"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &blit_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            face_pipeline_cull,
            face_pipeline_no_cull,
            line_pipeline_overlay,
            bone_pipeline_overlay,
            blit_pipeline,
            globals_layout,
            material_layout,
            blit_layout,
            texture_sampler,
            blit_sampler,
            scenes: HashMap::new(),
            scene_lru: VecDeque::new(),
            fallback_white: None,
        }
    }
}

fn create_face_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    cull_mode: Option<wgpu::Face>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("geo_face_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<MeshVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3,
                    1 => Float32x3,
                    2 => Float32x4,
                    3 => Float32x2
                ],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_line_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("geo_line_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LineVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_bone_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("geo_bone_pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<LineVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
            }],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_frame_targets(
    device: &wgpu::Device,
    shared: &GpuShared,
    width: u32,
    height: u32,
) -> FrameTargets {
    let size = wgpu::Extent3d {
        width: width.max(1),
        height: height.max(1),
        depth_or_array_layers: 1,
    };

    let color_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("geo_viewer_offscreen_color"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("geo_viewer_offscreen_depth"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let blit_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("geo_viewer_blit_bind_group"),
        layout: &shared.blit_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&shared.blit_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&color_view),
            },
        ],
    });

    FrameTargets {
        width,
        height,
        _color_texture: color_texture,
        color_view,
        _depth_texture: depth_texture,
        depth_view,
        blit_bind_group,
    }
}

fn upload_cpu_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &CpuTexture,
) -> wgpu::Texture {
    let size = wgpu::Extent3d {
        width: tex.width.max(1),
        height: tex.height.max(1),
        depth_or_array_layers: 1,
    };

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("geo_cpu_texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &tex.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * tex.width.max(1)),
            rows_per_image: Some(tex.height.max(1)),
        },
        size,
    );

    texture
}

fn upload_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shared: &mut GpuShared,
    scene: &CpuScene,
) -> GpuScene {
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("geo_mesh_vertices"),
        contents: bytemuck::cast_slice(&scene.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let globals = Globals {
        view_proj: Mat4::IDENTITY.to_cols_array_2d(),
        light_dir: normalize4([-0.20, 0.90, -0.35, 0.0]),
        render_opts: [1.0, 2.0, 0.0, 0.0],
    };
    let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("geo_globals"),
        contents: bytemuck::bytes_of(&globals),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("geo_globals_bind_group"),
        layout: &shared.globals_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: globals_buffer.as_entire_binding(),
        }],
    });

    let mut parts = Vec::new();
    for part in &scene.parts {
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("geo_mesh_indices"),
            contents: bytemuck::cast_slice(&part.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let material_params = MaterialParams {
            layer_count: part.textures.len().min(MAX_MATERIAL_LAYERS) as u32,
            record_kind: part.record_kind,
            _pad: [0; 2],
        };

        let material_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("geo_material_params"),
            contents: bytemuck::bytes_of(&material_params),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let mut keepalive = Vec::new();
        let mut views = Vec::new();

        for tex in part.textures.iter().take(MAX_MATERIAL_LAYERS) {
            let gpu_tex = upload_cpu_texture(device, queue, tex);
            let view = gpu_tex.create_view(&wgpu::TextureViewDescriptor::default());
            keepalive.push(gpu_tex);
            views.push(view);
        }

        while views.len() < MAX_MATERIAL_LAYERS {
            if shared.fallback_white.is_none() {
                let white = upload_cpu_texture(
                    device,
                    queue,
                    &CpuTexture {
                        width: 1,
                        height: 1,
                        rgba: vec![255, 255, 255, 255],
                    },
                );
                let view = white.create_view(&wgpu::TextureViewDescriptor::default());
                shared.fallback_white = Some((white, view));
            }

            if let Some((_, view)) = shared.fallback_white.as_ref() {
                views.push(view.clone());
            }
        }

        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&shared.texture_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: material_buffer.as_entire_binding(),
            },
        ];

        for (i, view) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("geo_material_bind_group"),
            layout: &shared.material_layout,
            entries: &entries,
        });

        parts.push(GpuPart {
            index_buffer,
            index_count: part.indices.len() as u32,
            bind_group,
            class: part.class,
            _material_buffer: material_buffer,
            _texture_keepalive: keepalive,
        });
    }

    GpuScene {
        vertex_buffer,
        globals_buffer,
        globals_bind_group,
        parts,
        ground_lines: upload_lines(device, &scene.ground_lines, "geo_ground_lines"),
        wire_lines: upload_lines(device, &scene.wire_lines, "geo_wire_lines"),
        helper_wire_lines: upload_lines(device, &scene.helper_wire_lines, "geo_helper_wire_lines"),
        bone_lines: upload_lines(device, &scene.bone_lines, "geo_bone_lines"),
    }
}

fn upload_lines(device: &wgpu::Device, vertices: &[LineVertex], label: &str) -> Option<GpuLines> {
    if vertices.is_empty() {
        return None;
    }

    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });

    Some(GpuLines {
        buffer,
        vertex_count: vertices.len() as u32,
    })
}

fn build_cpu_scene(
    geo: &GeoFile,
    textures: &[Option<DdsPreview>],
    key: String,
    rigid_clip: Option<&RigidAnimClip>,
    rigid_time_seconds: f32,
) -> Arc<CpuScene> {
    let (center, radius) = geo_bounds(geo);

    let sampled_pose = rigid_clip.and_then(|clip| sample_rigid_pose(geo, clip, rigid_time_seconds));
    let (positions, normals) = if let Some(pose) = &sampled_pose {
        build_animated_mesh_vertices(geo, &pose.skin_matrices)
    } else {
        (geo.verts.clone(), geo.normals.clone())
    };

    let vertices: Vec<MeshVertex> = positions
        .iter()
        .enumerate()
        .map(|(i, &p)| MeshVertex {
            position: to_view_space(p),
            normal: normals
                .get(i)
                .copied()
                .map(to_view_space)
                .map(normalize3)
                .unwrap_or([0.0, 1.0, 0.0]),
            color: scene_vertex_color(false),
            uv: geo.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
        })
        .collect();

    let mut parts = Vec::new();
    if geo.subsets.is_empty() {
        let mut indices = Vec::with_capacity(geo.faces.len() * 3);
        for face in &geo.faces {
            indices.push(face[0] as u32);
            indices.push(face[1] as u32);
            indices.push(face[2] as u32);
        }
        parts.push(CpuPart {
            indices,
            textures: Vec::new(),
            class: PartClass::Solid,
            record_kind: 1,
        });
    } else {
        for subset in &geo.subsets {
            let start = (subset.start / 3) as usize;
            let count = (subset.count / 3) as usize;
            let end = (start + count).min(geo.faces.len());

            let mut indices = Vec::with_capacity((end - start) * 3);
            for face in &geo.faces[start..end] {
                indices.push(face[0] as u32);
                indices.push(face[1] as u32);
                indices.push(face[2] as u32);
            }

            let textures: Vec<CpuTexture> = textures
                .get(subset.material)
                .and_then(|t| t.as_ref())
                .map(|dds| CpuTexture {
                    width: dds.width as u32,
                    height: dds.height as u32,
                    rgba: dds.rgba_pixels.clone(),
                })
                .into_iter()
                .collect();

            parts.push(CpuPart {
                indices,
                textures,
                class: PartClass::Solid,
                record_kind: 1,
            });
        }
    }

    let ground_lines = build_ground_lines(geo, center, radius);
    let wire_lines = build_wire_lines_from_positions(&positions, &geo.faces);
    let helper_wire_lines = Vec::new();
    let bone_lines =
        if let (Some(skeleton), Some(pose)) = (geo.skeleton.as_ref(), sampled_pose.as_ref()) {
            build_animated_bone_lines(skeleton, &pose.world_matrices)
        } else {
            geo.skeleton
                .as_ref()
                .map(build_bone_lines)
                .unwrap_or_default()
        };

    Arc::new(CpuScene {
        key,
        vertices,
        parts,
        ground_lines,
        wire_lines,
        helper_wire_lines,
        bone_lines,
        center,
        radius,
        pick_targets: Vec::new(),
    })
}

struct SampledRigidPose {
    world_matrices: Vec<Mat4>,
    skin_matrices: Vec<Mat4>,
}

fn rigid_frame_index(clip: &RigidAnimClip, time_seconds: f32) -> usize {
    if clip.frame_times.is_empty() {
        let frame_count = clip
            .streams
            .iter()
            .map(|stream| stream.rotations_xyzw.len())
            .min()
            .unwrap_or(0);

        if frame_count <= 1 {
            return 0;
        }

        let duration = clip.duration_seconds.max(1.0 / clip.sample_rate.max(1.0));
        let clamped = time_seconds.clamp(0.0, duration);
        return ((clamped * clip.sample_rate).round() as usize).min(frame_count - 1);
    }

    let clamped = time_seconds.clamp(0.0, *clip.frame_times.last().unwrap_or(&0.0));

    let mut best_index = 0usize;
    let mut best_dist = f32::MAX;

    for (i, &frame_time) in clip.frame_times.iter().enumerate() {
        let dist = (frame_time - clamped).abs();
        if dist < best_dist {
            best_dist = dist;
            best_index = i;
        }
    }

    best_index
}

fn sample_rigid_pose(
    geo: &GeoFile,
    clip: &RigidAnimClip,
    time_seconds: f32,
) -> Option<SampledRigidPose> {
    let skeleton = geo.skeleton.as_ref()?;
    if skeleton.bone_count == 0 {
        return None;
    }

    let frame_index = rigid_frame_index(clip, time_seconds);

    let bind_world: Vec<Mat4> = skeleton
        .bind_matrices
        .iter()
        .copied()
        .map(mat4_from_arr)
        .collect();

    let inverse_bind: Vec<Mat4> = skeleton
        .inverse_bind_matrices
        .iter()
        .copied()
        .map(mat4_from_arr)
        .collect();

    let use_non_root_offset = should_offset_stream_indices_by_one(skeleton, clip);

    let mut sampled_rotations: Vec<Option<Quat>> = vec![None; skeleton.bone_count];

    for stream in &clip.streams {
        let Some(target_bone_index) =
            target_bone_index_for_stream(skeleton, stream, use_non_root_offset)
        else {
            continue;
        };

        if target_bone_index == 0 && skeleton.bone_count >= 20 {
            continue;
        }

        let samples =
            normalized_stream_rotations_for_pose(skeleton, clip, stream, target_bone_index);

        if samples.is_empty() {
            continue;
        }

        let q = samples[frame_index.min(samples.len().saturating_sub(1))];
        sampled_rotations[target_bone_index] =
            Some(Quat::from_xyzw(q[0], q[1], q[2], q[3]).normalize());
    }

    let mut local_matrices = vec![Mat4::IDENTITY; skeleton.bone_count];

    for bone_index in 0..skeleton.bone_count {
        let bind_local =
            if let Some(parent_index) = skeleton.parent.get(bone_index).and_then(|p| *p) {
                bind_world[parent_index].inverse() * bind_world[bone_index]
            } else {
                bind_world[bone_index]
            };

        let (scale, bind_rotation, translation) = bind_local.to_scale_rotation_translation();

        let final_rotation = sampled_rotations[bone_index]
            .map(|sampled| bind_rotation * sampled)
            .unwrap_or(bind_rotation);

        local_matrices[bone_index] =
            Mat4::from_scale_rotation_translation(scale, final_rotation, translation);
    }

    let mut world_matrices = vec![Mat4::IDENTITY; skeleton.bone_count];
    for bone_index in 0..skeleton.bone_count {
        if let Some(parent_index) = skeleton.parent.get(bone_index).and_then(|p| *p) {
            world_matrices[bone_index] = world_matrices[parent_index] * local_matrices[bone_index];
        } else {
            world_matrices[bone_index] = local_matrices[bone_index];
        }
    }

    let skin_matrices = world_matrices
        .iter()
        .zip(inverse_bind.iter())
        .map(|(world, inv_bind)| *world * *inv_bind)
        .collect();

    Some(SampledRigidPose {
        world_matrices,
        skin_matrices,
    })
}

fn should_offset_stream_indices_by_one(skeleton: &GeoSkeleton, clip: &RigidAnimClip) -> bool {
    if skeleton.bone_count <= 1 {
        return false;
    }

    if !skeleton
        .names
        .first()
        .map(|name| is_root_like_bone_name(name))
        .unwrap_or(false)
    {
        return false;
    }

    if clip.streams.len() + 1 != skeleton.bone_count {
        return false;
    }

    clip.streams
        .iter()
        .enumerate()
        .all(|(i, stream)| stream.stream_index == i)
}

fn target_bone_index_for_stream(
    skeleton: &GeoSkeleton,
    stream: &RigidAnimStream,
    use_non_root_offset: bool,
) -> Option<usize> {
    let bone_index = if use_non_root_offset {
        stream.stream_index + 1
    } else {
        stream.stream_index
    };

    if bone_index < skeleton.bone_count {
        Some(bone_index)
    } else {
        None
    }
}

fn normalized_stream_rotations_for_pose(
    _skeleton: &GeoSkeleton,
    clip: &RigidAnimClip,
    stream: &RigidAnimStream,
    _target_bone_index: usize,
) -> Vec<[f32; 4]> {
    let target_len = clip.frame_times.len().max(
        clip.streams
            .iter()
            .map(|s| s.rotations_xyzw.len())
            .min()
            .unwrap_or(0),
    );

    if target_len == 0 {
        return Vec::new();
    }

    let samples = collapse_adjacent_duplicate_quats(&stream.rotations_xyzw, 1.0e-6);
    if samples.is_empty() {
        return Vec::new();
    }

    if samples.len() == target_len {
        return samples;
    }

    resample_quat_track_len(&samples, target_len)
}

fn collapse_adjacent_duplicate_quats(samples: &[[f32; 4]], epsilon: f32) -> Vec<[f32; 4]> {
    let mut out = Vec::new();

    for &q in samples {
        let q = canonicalize_quat_sign(q);

        let is_same_as_last = out
            .last()
            .map(|&last| quat_distance_sq(last, q) <= epsilon)
            .unwrap_or(false);

        if !is_same_as_last {
            out.push(q);
        }
    }

    out
}

fn canonicalize_quat_sign(mut q: [f32; 4]) -> [f32; 4] {
    if q[3] < 0.0 {
        q[0] = -q[0];
        q[1] = -q[1];
        q[2] = -q[2];
        q[3] = -q[3];
    }
    q
}

fn quat_distance_sq(a: [f32; 4], b: [f32; 4]) -> f32 {
    let direct = (a[0] - b[0]).powi(2)
        + (a[1] - b[1]).powi(2)
        + (a[2] - b[2]).powi(2)
        + (a[3] - b[3]).powi(2);

    let negated = (a[0] + b[0]).powi(2)
        + (a[1] + b[1]).powi(2)
        + (a[2] + b[2]).powi(2)
        + (a[3] + b[3]).powi(2);

    direct.min(negated)
}

fn resample_quat_track_len(samples: &[[f32; 4]], target_len: usize) -> Vec<[f32; 4]> {
    if target_len == 0 {
        return Vec::new();
    }

    if samples.is_empty() {
        return vec![[0.0, 0.0, 0.0, 1.0]; target_len];
    }

    if samples.len() == 1 {
        return vec![samples[0]; target_len];
    }

    if samples.len() == target_len {
        return samples.to_vec();
    }

    let src_last = (samples.len() - 1) as f32;
    let dst_last = (target_len - 1) as f32;

    let mut out = Vec::with_capacity(target_len);
    for i in 0..target_len {
        let t = if target_len <= 1 {
            0.0
        } else {
            i as f32 / dst_last
        };

        let src_pos = t * src_last;
        let src_index = src_pos.round() as usize;
        let src_index = src_index.min(samples.len() - 1);
        out.push(samples[src_index]);
    }

    out
}

fn build_animated_mesh_vertices(
    geo: &GeoFile,
    skin_matrices: &[Mat4],
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let Some(skeleton) = geo.skeleton.as_ref() else {
        return (geo.verts.clone(), geo.normals.clone());
    };

    let Some(weights) = skeleton.weights.as_ref() else {
        return (geo.verts.clone(), geo.normals.clone());
    };

    let mut positions = Vec::with_capacity(geo.verts.len());
    let mut normals = Vec::with_capacity(geo.normals.len());

    for vertex_index in 0..geo.verts.len() {
        let src_pos = geo
            .verts
            .get(vertex_index)
            .copied()
            .unwrap_or([0.0, 0.0, 0.0]);
        let src_nrm = geo
            .normals
            .get(vertex_index)
            .copied()
            .unwrap_or([0.0, 1.0, 0.0]);

        let src_pos = Vec3::new(src_pos[0], src_pos[1], src_pos[2]);
        let src_nrm = Vec3::new(src_nrm[0], src_nrm[1], src_nrm[2]);

        let Some(influences) = weights.get(vertex_index) else {
            positions.push([src_pos.x, src_pos.y, src_pos.z]);
            normals.push([src_nrm.x, src_nrm.y, src_nrm.z]);
            continue;
        };

        if influences.is_empty() {
            positions.push([src_pos.x, src_pos.y, src_pos.z]);
            normals.push([src_nrm.x, src_nrm.y, src_nrm.z]);
            continue;
        }

        let mut pos_accum = Vec3::ZERO;
        let mut nrm_accum = Vec3::ZERO;

        for influence in influences {
            let Some(skin) = skin_matrices.get(influence.bone_index) else {
                continue;
            };

            pos_accum += skin.transform_point3(src_pos) * influence.weight;
            nrm_accum += skin.transform_vector3(src_nrm) * influence.weight;
        }

        let nrm_accum = if nrm_accum.length_squared() > 1.0e-8 {
            nrm_accum.normalize()
        } else {
            src_nrm
        };

        positions.push([pos_accum.x, pos_accum.y, pos_accum.z]);
        normals.push([nrm_accum.x, nrm_accum.y, nrm_accum.z]);
    }

    (positions, normals)
}

fn build_animated_bone_lines(skeleton: &GeoSkeleton, world_matrices: &[Mat4]) -> Vec<LineVertex> {
    let color = [1.0, 0.92, 0.35, 1.0];
    let points: Vec<[f32; 3]> = world_matrices
        .iter()
        .map(|mat| to_view_space(mat.transform_point3(Vec3::ZERO).into()))
        .collect();

    let mut out = Vec::new();
    for (bone_index, parent) in skeleton.parent.iter().enumerate() {
        let Some(parent_index) = *parent else {
            continue;
        };
        let Some(&a) = points.get(parent_index) else {
            continue;
        };
        let Some(&b) = points.get(bone_index) else {
            continue;
        };
        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });
    }
    out
}

fn build_wire_lines_from_positions(positions: &[[f32; 3]], faces: &[[u16; 3]]) -> Vec<LineVertex> {
    let color = [120.0 / 255.0, 220.0 / 255.0, 1.0, 1.0];
    let mut out = Vec::with_capacity(faces.len() * 6);

    for face in faces {
        let ia = face[0] as usize;
        let ib = face[1] as usize;
        let ic = face[2] as usize;

        let Some(&a_raw) = positions.get(ia) else {
            continue;
        };
        let Some(&b_raw) = positions.get(ib) else {
            continue;
        };
        let Some(&c_raw) = positions.get(ic) else {
            continue;
        };

        let a = to_view_space(a_raw);
        let b = to_view_space(b_raw);
        let c = to_view_space(c_raw);

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });
        out.push(LineVertex { position: b, color });
        out.push(LineVertex { position: c, color });
        out.push(LineVertex { position: c, color });
        out.push(LineVertex { position: a, color });
    }

    out
}

fn mat4_from_arr(m: [f32; 16]) -> Mat4 {
    Mat4::from_cols_array(&m)
}

fn is_root_like_bone_name(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    low == "bip01" || low == "root" || low.starts_with("root")
}

fn cpu_texture_from_dds(dds: &DdsPreview) -> CpuTexture {
    CpuTexture {
        width: dds.width as u32,
        height: dds.height as u32,
        rgba: dds.rgba_pixels.clone(),
    }
}

fn build_cpu_scene_from_scn(
    scn: &ScnFile,
    models: &[SceneGeoModel],
    embedded_textures: &HashMap<String, DdsPreview>,
    hidden_nodes: &BTreeSet<usize>,
    hidden_chunks: &BTreeSet<usize>,
    selected: Option<SceneSelection>,
    key: String,
) -> Arc<CpuScene> {
    let geo_by_archetype: HashMap<String, &SceneGeoModel> = models
        .iter()
        .map(|m| (m.archetype.to_ascii_lowercase(), m))
        .collect();

    let mut vertices = Vec::new();
    let mut parts = Vec::new();
    let mut wire_lines = Vec::new();
    let mut helper_wire_lines = Vec::new();
    let mut marker_lines = Vec::new();
    let mut pick_targets = Vec::new();

    let mut have_bounds = false;
    let mut min = [0.0f32; 3];
    let mut max = [0.0f32; 3];

    let mut update_bounds = |p: [f32; 3]| {
        if !have_bounds {
            min = p;
            max = p;
            have_bounds = true;
        } else {
            min[0] = min[0].min(p[0]);
            min[1] = min[1].min(p[1]);
            min[2] = min[2].min(p[2]);
            max[0] = max[0].max(p[0]);
            max[1] = max[1].max(p[1]);
            max[2] = max[2].max(p[2]);
        }
    };

    for (chunk_index, chunk) in scn.mesh_chunks.iter().enumerate() {
        if hidden_chunks.contains(&chunk_index) {
            continue;
        }

        append_scn_chunk_mesh(
            chunk_index,
            chunk,
            embedded_textures,
            selected == Some(SceneSelection::MeshChunk(chunk_index)),
            &mut vertices,
            &mut parts,
            &mut wire_lines,
            &mut helper_wire_lines,
            &mut pick_targets,
            &mut update_bounds,
        );
    }

    for node in &scn.nodes {
        if hidden_nodes.contains(&node.index) {
            continue;
        }

        let marker_pos = to_view_space(node.translation);
        update_bounds(marker_pos);

        if node.is_marker() {
            let (marker_color, marker_size) = scene_marker_style(&node.name);
            let marker_color = if selected == Some(SceneSelection::Node(node.index)) {
                selected_scene_tint_color()
            } else {
                marker_color
            };
            append_scene_marker_lines(&mut marker_lines, marker_pos, marker_size, marker_color);
            pick_targets.push(ScenePickTarget {
                selection: SceneSelection::Node(node.index),
                center: marker_pos,
                radius: marker_size,
            });
            continue;
        }

        let archetype_key = node.archetype.trim().to_ascii_lowercase();
        let Some(model) = geo_by_archetype.get(&archetype_key) else {
            continue;
        };

        let geo = &model.geo;
        let model_is_helper = is_shadow_like_name(&archetype_key);
        let is_selected = selected == Some(SceneSelection::Node(node.index));

        let base_vertex = vertices.len() as u32;
        let mut instance_positions = Vec::with_capacity(geo.verts.len());

        for (i, &pos) in geo.verts.iter().enumerate() {
            let world_pos = apply_transform_point(&node.transform, pos);
            let view_pos = to_view_space(world_pos);

            let raw_normal = geo.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
            let world_normal = apply_transform_direction(&node.transform, raw_normal);
            let view_normal = normalize3(to_view_space(world_normal));

            let uv = geo.uvs.get(i).copied().unwrap_or([0.0, 0.0]);

            vertices.push(MeshVertex {
                position: view_pos,
                normal: view_normal,
                color: scene_vertex_color(is_selected),
                uv,
            });

            instance_positions.push(view_pos);
            update_bounds(view_pos);
        }

        let (instance_center, instance_radius) = bounds_from_points(&instance_positions)
            .map(|(center, radius)| (center, radius.max(45.0)))
            .unwrap_or((marker_pos, 75.0));

        if geo.subsets.is_empty() {
            let part_class = if model_is_helper
                || geo
                    .texture_names
                    .iter()
                    .any(|name| is_shadow_like_name(name))
            {
                PartClass::Helper
            } else {
                PartClass::Solid
            };

            let line_color = if is_selected {
                selected_scene_tint_color()
            } else {
                [120.0 / 255.0, 220.0 / 255.0, 1.0, 1.0]
            };
            let draw_wire = !matches!(part_class, PartClass::Helper);

            let mut part_indices = Vec::with_capacity(geo.faces.len() * 3);

            for face in &geo.faces {
                part_indices.push(base_vertex + face[0] as u32);
                part_indices.push(base_vertex + face[1] as u32);
                part_indices.push(base_vertex + face[2] as u32);

                let ia = face[0] as usize;
                let ib = face[1] as usize;
                let ic = face[2] as usize;

                let Some(&a) = instance_positions.get(ia) else {
                    continue;
                };
                let Some(&b) = instance_positions.get(ib) else {
                    continue;
                };
                let Some(&c) = instance_positions.get(ic) else {
                    continue;
                };

                if draw_wire {
                    wire_lines.push(LineVertex {
                        position: a,
                        color: line_color,
                    });
                    wire_lines.push(LineVertex {
                        position: b,
                        color: line_color,
                    });
                    wire_lines.push(LineVertex {
                        position: b,
                        color: line_color,
                    });
                    wire_lines.push(LineVertex {
                        position: c,
                        color: line_color,
                    });
                    wire_lines.push(LineVertex {
                        position: c,
                        color: line_color,
                    });
                    wire_lines.push(LineVertex {
                        position: a,
                        color: line_color,
                    });
                }
            }

            parts.push(CpuPart {
                indices: part_indices,
                textures: Vec::new(),
                class: part_class,
                record_kind: 1,
            });
        } else {
            for subset in &geo.subsets {
                let texture_name = geo
                    .texture_names
                    .get(subset.material)
                    .map(|s| s.as_str())
                    .unwrap_or("");

                let part_class = if model_is_helper || is_shadow_like_name(texture_name) {
                    PartClass::Helper
                } else {
                    PartClass::Solid
                };

                let line_color = if is_selected {
                    selected_scene_tint_color()
                } else {
                    [120.0 / 255.0, 220.0 / 255.0, 1.0, 1.0]
                };
                let draw_wire = !matches!(part_class, PartClass::Helper);

                let start = (subset.start / 3) as usize;
                let count = (subset.count / 3) as usize;
                let end = (start + count).min(geo.faces.len());

                let mut part_indices = Vec::with_capacity((end - start) * 3);

                for face in &geo.faces[start..end] {
                    part_indices.push(base_vertex + face[0] as u32);
                    part_indices.push(base_vertex + face[1] as u32);
                    part_indices.push(base_vertex + face[2] as u32);

                    let ia = face[0] as usize;
                    let ib = face[1] as usize;
                    let ic = face[2] as usize;

                    let Some(&a) = instance_positions.get(ia) else {
                        continue;
                    };
                    let Some(&b) = instance_positions.get(ib) else {
                        continue;
                    };
                    let Some(&c) = instance_positions.get(ic) else {
                        continue;
                    };

                    if draw_wire {
                        wire_lines.push(LineVertex {
                            position: a,
                            color: line_color,
                        });
                        wire_lines.push(LineVertex {
                            position: b,
                            color: line_color,
                        });
                        wire_lines.push(LineVertex {
                            position: b,
                            color: line_color,
                        });
                        wire_lines.push(LineVertex {
                            position: c,
                            color: line_color,
                        });
                        wire_lines.push(LineVertex {
                            position: c,
                            color: line_color,
                        });
                        wire_lines.push(LineVertex {
                            position: a,
                            color: line_color,
                        });
                    }
                }

                let textures: Vec<CpuTexture> = model
                    .textures
                    .get(subset.material)
                    .and_then(|t| t.as_ref())
                    .map(cpu_texture_from_dds)
                    .into_iter()
                    .collect();

                parts.push(CpuPart {
                    indices: part_indices,
                    textures,
                    class: part_class,
                    record_kind: 1,
                });
            }
        }

        pick_targets.push(ScenePickTarget {
            selection: SceneSelection::Node(node.index),
            center: instance_center,
            radius: instance_radius,
        });
    }

    if !have_bounds {
        min = [-1.0, -1.0, -1.0];
        max = [1.0, 1.0, 1.0];
    }

    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];

    let mut radius: f32 = 1.0;
    for v in &vertices {
        radius = radius.max(length3(sub3(v.position, center)));
    }
    for chunk in marker_lines.chunks(2) {
        for line in chunk {
            radius = radius.max(length3(sub3(line.position, center)));
        }
    }
    radius = radius.max(100.0);

    let ground_y = min[1] - 0.02;
    let ground_lines = build_ground_lines_for_bounds(center, radius, ground_y);

    Arc::new(CpuScene {
        key,
        vertices,
        parts,
        ground_lines,
        wire_lines,
        helper_wire_lines,
        bone_lines: marker_lines,
        center,
        radius,
        pick_targets,
    })
}

fn append_scn_chunk_mesh<F: FnMut([f32; 3])>(
    chunk_index: usize,
    chunk: &ScnMeshChunk,
    embedded_textures: &HashMap<String, DdsPreview>,
    is_selected: bool,
    vertices: &mut Vec<MeshVertex>,
    parts: &mut Vec<CpuPart>,
    wire_lines: &mut Vec<LineVertex>,
    _helper_wire_lines: &mut Vec<LineVertex>,
    pick_targets: &mut Vec<ScenePickTarget>,
    update_bounds: &mut F,
) {
    let part_class = if chunk
        .texture_names
        .iter()
        .any(|name| is_shadow_like_name(name))
    {
        PartClass::Helper
    } else {
        PartClass::Solid
    };

    let line_color = if is_selected {
        selected_scene_tint_color()
    } else {
        [185.0 / 255.0, 185.0 / 255.0, 195.0 / 255.0, 1.0]
    };
    let draw_wire = !matches!(part_class, PartClass::Helper);

    let base_vertex = vertices.len() as u32;
    let mut chunk_positions = Vec::with_capacity(chunk.vertices.len());

    for vertex in &chunk.vertices {
        let world_pos = if let Some(transform) = &chunk.transform {
            apply_transform_point(transform, vertex.position)
        } else {
            vertex.position
        };

        let world_normal = if let Some(transform) = &chunk.transform {
            apply_transform_direction(transform, vertex.normal)
        } else {
            vertex.normal
        };

        let view_pos = to_view_space(world_pos);
        let view_normal = normalize3(to_view_space(world_normal));

        vertices.push(MeshVertex {
            position: view_pos,
            normal: view_normal,
            color: scene_vertex_color(is_selected),
            uv: vertex.uv,
        });

        chunk_positions.push(view_pos);
        update_bounds(view_pos);
    }

    let mut part_indices = Vec::with_capacity(chunk.indices.len());

    for tri in chunk.indices.chunks_exact(3) {
        let ia = tri[0] as usize;
        let ib = tri[1] as usize;
        let ic = tri[2] as usize;

        part_indices.push(base_vertex + tri[0]);
        part_indices.push(base_vertex + tri[1]);
        part_indices.push(base_vertex + tri[2]);

        let Some(&a) = chunk_positions.get(ia) else {
            continue;
        };
        let Some(&b) = chunk_positions.get(ib) else {
            continue;
        };
        let Some(&c) = chunk_positions.get(ic) else {
            continue;
        };

        if draw_wire {
            wire_lines.push(LineVertex {
                position: a,
                color: line_color,
            });
            wire_lines.push(LineVertex {
                position: b,
                color: line_color,
            });
            wire_lines.push(LineVertex {
                position: b,
                color: line_color,
            });
            wire_lines.push(LineVertex {
                position: c,
                color: line_color,
            });
            wire_lines.push(LineVertex {
                position: c,
                color: line_color,
            });
            wire_lines.push(LineVertex {
                position: a,
                color: line_color,
            });
        }
    }

    if let Some((chunk_center, chunk_radius)) = bounds_from_points(&chunk_positions) {
        pick_targets.push(ScenePickTarget {
            selection: SceneSelection::MeshChunk(chunk_index),
            center: chunk_center,
            radius: chunk_radius.max(35.0),
        });
    }

    if !part_indices.is_empty() {
        for span in &chunk.texture_spans {
            let end = span.index_start + span.index_count;
            if end > chunk.indices.len() {
                continue;
            }

            let texture_name = chunk
                .texture_names
                .get(span.texture_slot)
                .or_else(|| chunk.texture_names.first());

            let textures: Vec<CpuTexture> = texture_name
                .and_then(|name| embedded_textures.get(&name.to_ascii_lowercase()))
                .map(cpu_texture_from_dds)
                .into_iter()
                .collect();

            let span_class = if texture_name
                .map(|name| is_shadow_like_name(name))
                .unwrap_or(false)
            {
                PartClass::Helper
            } else {
                part_class
            };

            parts.push(CpuPart {
                indices: chunk.indices[span.index_start..end]
                    .iter()
                    .map(|idx| base_vertex + *idx)
                    .collect(),
                textures,
                class: span_class,
                record_kind: 1,
            });
        }
    }
}

fn scn_bounds(scn: &ScnFile) -> ([f32; 3], f32) {
    let mut have_bounds = false;
    let mut min = [0.0f32; 3];
    let mut max = [0.0f32; 3];

    let mut include = |p: [f32; 3]| {
        if !have_bounds {
            min = p;
            max = p;
            have_bounds = true;
        } else {
            min[0] = min[0].min(p[0]);
            min[1] = min[1].min(p[1]);
            min[2] = min[2].min(p[2]);
            max[0] = max[0].max(p[0]);
            max[1] = max[1].max(p[1]);
            max[2] = max[2].max(p[2]);
        }
    };

    for node in &scn.nodes {
        include(to_view_space(node.translation));
    }

    for chunk in &scn.mesh_chunks {
        for vertex in &chunk.vertices {
            let world_pos = if let Some(transform) = &chunk.transform {
                apply_transform_point(transform, vertex.position)
            } else {
                vertex.position
            };

            include(to_view_space(world_pos));
        }
    }

    if !have_bounds {
        return ([0.0, 0.0, 0.0], 100.0);
    }

    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];

    let mut radius: f32 = 1.0;

    for node in &scn.nodes {
        let p = to_view_space(node.translation);
        radius = radius.max(length3(sub3(p, center)));
    }

    for chunk in &scn.mesh_chunks {
        for vertex in &chunk.vertices {
            let world_pos = if let Some(transform) = &chunk.transform {
                apply_transform_point(transform, vertex.position)
            } else {
                vertex.position
            };

            let p = to_view_space(world_pos);
            radius = radius.max(length3(sub3(p, center)));
        }
    }

    (center, radius.max(100.0))
}

fn geo_min_max(geo: &GeoFile) -> Option<([f32; 3], [f32; 3])> {
    if geo.verts.is_empty() {
        return None;
    }

    let first = to_view_space(geo.verts[0]);
    let mut min = first;
    let mut max = first;

    for &v in &geo.verts {
        let p = to_view_space(v);
        min[0] = min[0].min(p[0]);
        min[1] = min[1].min(p[1]);
        min[2] = min[2].min(p[2]);
        max[0] = max[0].max(p[0]);
        max[1] = max[1].max(p[1]);
        max[2] = max[2].max(p[2]);
    }

    Some((min, max))
}

fn scn_min_max(scn: &ScnFile) -> Option<([f32; 3], [f32; 3])> {
    let mut have_bounds = false;
    let mut min = [0.0f32; 3];
    let mut max = [0.0f32; 3];

    let mut include = |p: [f32; 3]| {
        if !have_bounds {
            min = p;
            max = p;
            have_bounds = true;
        } else {
            min[0] = min[0].min(p[0]);
            min[1] = min[1].min(p[1]);
            min[2] = min[2].min(p[2]);
            max[0] = max[0].max(p[0]);
            max[1] = max[1].max(p[1]);
            max[2] = max[2].max(p[2]);
        }
    };

    for node in &scn.nodes {
        include(to_view_space(node.translation));
    }

    for chunk in &scn.mesh_chunks {
        for vertex in &chunk.vertices {
            let world_pos = if let Some(transform) = &chunk.transform {
                apply_transform_point(transform, vertex.position)
            } else {
                vertex.position
            };

            include(to_view_space(world_pos));
        }
    }

    if have_bounds { Some((min, max)) } else { None }
}

fn extent_dimensions(min: [f32; 3], max: [f32; 3]) -> [f32; 3] {
    [
        (max[0] - min[0]).abs(),
        (max[1] - min[1]).abs(),
        (max[2] - min[2]).abs(),
    ]
}

fn geo_frame_distance(geo: &GeoFile) -> f32 {
    let Some((min, max)) = geo_min_max(geo) else {
        return 4.0;
    };

    let dims = extent_dimensions(min, max);
    let longest = dims[0].max(dims[1]).max(dims[2]);
    let mid = dims[0] + dims[1] + dims[2] - longest - dims[0].min(dims[1]).min(dims[2]);

    (longest * 1.35 + mid * 0.15).max(3.0)
}

fn scn_frame_distance(scn: &ScnFile) -> f32 {
    let Some((min, max)) = scn_min_max(scn) else {
        return 250.0;
    };

    let dims = extent_dimensions(min, max);
    let mut sorted = dims;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let shortest = sorted[0].max(1.0);
    let middle = sorted[1].max(1.0);
    let longest = sorted[2].max(1.0);

    let thin_ratio = longest / middle.max(1.0);

    if thin_ratio > 6.0 {
        (longest * 0.10 + middle * 0.70 + shortest * 0.20).max(40.0)
    } else {
        (longest * 0.28 + middle * 0.30 + shortest * 0.12).max(60.0)
    }
}

fn scene_marker_style(name: &str) -> ([f32; 4], f32) {
    let lower = name.trim().to_ascii_lowercase();

    if lower == "player_start" {
        ([110.0 / 255.0, 1.0, 140.0 / 255.0, 1.0], 150.0)
    } else if lower == "player_end" {
        ([190.0 / 255.0, 1.0, 110.0 / 255.0, 1.0], 140.0)
    } else if lower.starts_with("checkpoint") {
        ([1.0, 145.0 / 255.0, 70.0 / 255.0, 1.0], 130.0)
    } else if lower.starts_with("path") || lower.starts_with("lane") {
        ([90.0 / 255.0, 210.0 / 255.0, 1.0, 1.0], 110.0)
    } else if lower.starts_with("gay")
        || lower.starts_with("cyclist")
        || lower.starts_with("vicky")
        || lower.starts_with("dafydd")
        || lower.starts_with("myfanwy")
    {
        ([1.0, 120.0 / 255.0, 190.0 / 255.0, 1.0], 120.0)
    } else if lower.starts_with("car_") || lower.starts_with("dec_car_") {
        ([1.0, 95.0 / 255.0, 95.0 / 255.0, 1.0], 100.0)
    } else {
        ([1.0, 0.92, 0.35, 1.0], 75.0)
    }
}

fn append_scene_marker_lines(out: &mut Vec<LineVertex>, pos: [f32; 3], half: f32, color: [f32; 4]) {
    out.push(LineVertex {
        position: [pos[0] - half, pos[1], pos[2]],
        color,
    });
    out.push(LineVertex {
        position: [pos[0] + half, pos[1], pos[2]],
        color,
    });

    out.push(LineVertex {
        position: [pos[0], pos[1] - half, pos[2]],
        color,
    });
    out.push(LineVertex {
        position: [pos[0], pos[1] + half, pos[2]],
        color,
    });

    out.push(LineVertex {
        position: [pos[0], pos[1], pos[2] - half],
        color,
    });
    out.push(LineVertex {
        position: [pos[0], pos[1], pos[2] + half],
        color,
    });
}

fn bounds_from_points(points: &[[f32; 3]]) -> Option<([f32; 3], f32)> {
    let &first = points.first()?;
    let mut min = first;
    let mut max = first;

    for &p in points.iter().skip(1) {
        min[0] = min[0].min(p[0]);
        min[1] = min[1].min(p[1]);
        min[2] = min[2].min(p[2]);
        max[0] = max[0].max(p[0]);
        max[1] = max[1].max(p[1]);
        max[2] = max[2].max(p[2]);
    }

    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];

    let mut radius: f32 = 0.0;
    for &p in points {
        radius = radius.max(length3(sub3(p, center)));
    }

    Some((center, radius.max(1.0)))
}

fn project_scene_point(rect: Rect, view_proj: Mat4, point: [f32; 3]) -> Option<(egui::Pos2, f32)> {
    let clip = view_proj * Vec3::from_array(point).extend(1.0);
    if clip.w <= 1.0e-6 {
        return None;
    }

    let ndc = clip.truncate() / clip.w;
    if ndc.x < -1.05
        || ndc.x > 1.05
        || ndc.y < -1.05
        || ndc.y > 1.05
        || ndc.z < -1.05
        || ndc.z > 1.05
    {
        return None;
    }

    let x = rect.left() + (ndc.x * 0.5 + 0.5) * rect.width();
    let y = rect.top() + (1.0 - (ndc.y * 0.5 + 0.5)) * rect.height();
    Some((egui::pos2(x, y), ndc.z))
}

fn projected_pick_radius(
    rect: Rect,
    distance: f32,
    world_radius: f32,
    selection: SceneSelection,
) -> f32 {
    let fov_y = 55.0_f32.to_radians();
    let pixels_per_world =
        rect.height() / (2.0 * distance.max(NEAR_PLANE + 0.01) * (fov_y * 0.5).tan());
    let base_radius = world_radius * pixels_per_world;

    match selection {
        SceneSelection::Node(_) => base_radius.clamp(12.0, 120.0),
        SceneSelection::MeshChunk(_) => base_radius.clamp(10.0, 72.0),
    }
}

fn pick_scene_target(
    scene: &CpuScene,
    rect: Rect,
    view_proj: Mat4,
    eye: [f32; 3],
    pointer_pos: egui::Pos2,
) -> Option<SceneSelection> {
    let eye = arr_to_vec3(eye);
    let mut best: Option<(u8, f32, f32, SceneSelection)> = None;

    for target in &scene.pick_targets {
        let Some((screen_pos, _depth)) = project_scene_point(rect, view_proj, target.center) else {
            continue;
        };

        let world_distance = (arr_to_vec3(target.center) - eye).length();
        let screen_radius =
            projected_pick_radius(rect, world_distance, target.radius, target.selection);
        let pointer_distance = pointer_pos.distance(screen_pos);

        if pointer_distance > screen_radius {
            continue;
        }

        let priority = match target.selection {
            SceneSelection::Node(_) => 0,
            SceneSelection::MeshChunk(_) => 1,
        };
        let normalized_distance = pointer_distance / screen_radius.max(1.0);

        let should_replace = best
            .map(|(best_priority, best_distance, best_norm, _)| {
                priority < best_priority
                    || (priority == best_priority
                        && (world_distance < best_distance - 0.5
                            || ((world_distance - best_distance).abs() <= 0.5
                                && normalized_distance < best_norm)))
            })
            .unwrap_or(true);

        if should_replace {
            best = Some((
                priority,
                world_distance,
                normalized_distance,
                target.selection,
            ));
        }
    }

    best.map(|(_, _, _, selection)| selection)
}

fn visibility_set_hash(indices: &BTreeSet<usize>) -> u64 {
    let mut hasher = DefaultHasher::new();
    indices.hash(&mut hasher);
    hasher.finish()
}

fn scene_selection_cache_tag(selected: Option<SceneSelection>) -> String {
    match selected {
        Some(SceneSelection::Node(index)) => format!("node:{index}"),
        Some(SceneSelection::MeshChunk(index)) => format!("chunk:{index}"),
        None => "none".to_owned(),
    }
}

fn scene_vertex_color(is_selected: bool) -> [f32; 4] {
    [1.0, 1.0, 1.0, if is_selected { 1.0 } else { 0.0 }]
}

fn selected_scene_tint_color() -> [f32; 4] {
    [90.0 / 255.0, 175.0 / 255.0, 1.0, 1.0]
}

fn selected_scene_is_visible(
    selected: Option<SceneSelection>,
    hidden_nodes: &BTreeSet<usize>,
    hidden_chunks: &BTreeSet<usize>,
) -> bool {
    match selected {
        Some(SceneSelection::Node(index)) => !hidden_nodes.contains(&index),
        Some(SceneSelection::MeshChunk(index)) => !hidden_chunks.contains(&index),
        None => false,
    }
}

fn scene_transform_axis_color(axis: GizmoAxis) -> Color32 {
    match axis {
        GizmoAxis::X => Color32::from_rgb(230, 96, 96),
        GizmoAxis::Y => Color32::from_rgb(102, 214, 114),
        GizmoAxis::Z => Color32::from_rgb(96, 166, 255),
    }
}

fn scene_transform_axis_basis(axis: GizmoAxis) -> Vec3 {
    match axis {
        GizmoAxis::X => Vec3::X,
        GizmoAxis::Y => Vec3::Y,
        GizmoAxis::Z => Vec3::Z,
    }
}

fn scene_node_transform(node: &ScnNode) -> (Vec3, Quat, Vec3) {
    let matrix = mat4_from_arr(node.transform);
    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();

    let sanitized_scale = Vec3::new(
        scale.x.abs().max(0.05),
        scale.y.abs().max(0.05),
        scale.z.abs().max(0.05),
    );
    let sanitized_rotation = if rotation.length_squared() > 1.0e-8 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    };
    let sanitized_translation = if translation.is_finite() {
        translation
    } else {
        Vec3::from_array(node.translation)
    };

    (sanitized_translation, sanitized_rotation, sanitized_scale)
}

fn write_scene_node_transform(node: &mut ScnNode, translation: Vec3, rotation: Quat, scale: Vec3) {
    let scale = Vec3::new(
        scale.x.abs().max(0.05),
        scale.y.abs().max(0.05),
        scale.z.abs().max(0.05),
    );
    let rotation = if rotation.length_squared() > 1.0e-8 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    };
    let matrix = Mat4::from_scale_rotation_translation(scale, rotation, translation);
    node.transform = matrix.to_cols_array();
    node.translation = translation.to_array();
}

fn scene_chunk_center(chunk: &ScnMeshChunk) -> Option<Vec3> {
    if chunk.vertices.is_empty() {
        return None;
    }

    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);

    for vertex in &chunk.vertices {
        let pos = if let Some(transform) = &chunk.transform {
            arr_to_vec3(apply_transform_point(transform, vertex.position))
        } else {
            arr_to_vec3(vertex.position)
        };

        min = min.min(pos);
        max = max.max(pos);
    }

    Some((min + max) * 0.5)
}

fn scene_chunk_transform(chunk: &ScnMeshChunk) -> Option<(Vec3, Quat, Vec3)> {
    let matrix = if let Some(transform) = chunk.transform {
        mat4_from_arr(transform)
    } else {
        Mat4::from_translation(scene_chunk_center(chunk)?)
    };

    let (scale, rotation, translation) = matrix.to_scale_rotation_translation();
    let sanitized_scale = Vec3::new(
        scale.x.abs().max(0.05),
        scale.y.abs().max(0.05),
        scale.z.abs().max(0.05),
    );
    let sanitized_rotation = if rotation.length_squared() > 1.0e-8 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    };

    Some((translation, sanitized_rotation, sanitized_scale))
}

fn ensure_scene_chunk_transform(chunk: &mut ScnMeshChunk) -> Option<()> {
    if chunk.transform.is_some() {
        return Some(());
    }

    let center = scene_chunk_center(chunk)?;
    for vertex in &mut chunk.vertices {
        let pos = arr_to_vec3(vertex.position) - center;
        vertex.position = pos.to_array();
    }

    chunk.transform = Some(Mat4::from_translation(center).to_cols_array());
    Some(())
}

fn write_scene_chunk_transform(
    chunk: &mut ScnMeshChunk,
    translation: Vec3,
    rotation: Quat,
    scale: Vec3,
) {
    let scale = Vec3::new(
        scale.x.abs().max(0.05),
        scale.y.abs().max(0.05),
        scale.z.abs().max(0.05),
    );
    let rotation = if rotation.length_squared() > 1.0e-8 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    };
    let matrix = Mat4::from_scale_rotation_translation(scale, rotation, translation);
    chunk.transform = Some(matrix.to_cols_array());
}

fn selected_scene_transform(
    scn: &ScnFile,
    selected: Option<SceneSelection>,
    hidden_nodes: &BTreeSet<usize>,
    hidden_chunks: &BTreeSet<usize>,
) -> Option<(Vec3, Quat, Vec3)> {
    match selected {
        Some(SceneSelection::Node(index)) if !hidden_nodes.contains(&index) => {
            scn.nodes.get(index).map(scene_node_transform)
        }
        Some(SceneSelection::MeshChunk(index)) if !hidden_chunks.contains(&index) => {
            scn.mesh_chunks.get(index).and_then(scene_chunk_transform)
        }
        _ => None,
    }
}

fn write_selected_scene_transform(
    scn: &mut ScnFile,
    selected: SceneSelection,
    translation: Vec3,
    rotation: Quat,
    scale: Vec3,
) -> bool {
    match selected {
        SceneSelection::Node(index) => {
            let Some(node) = scn.nodes.get_mut(index) else {
                return false;
            };
            write_scene_node_transform(node, translation, rotation, scale);
            true
        }
        SceneSelection::MeshChunk(index) => {
            let Some(chunk) = scn.mesh_chunks.get_mut(index) else {
                return false;
            };
            if ensure_scene_chunk_transform(chunk).is_none() {
                return false;
            }
            write_scene_chunk_transform(chunk, translation, rotation, scale);
            true
        }
    }
}

fn scene_gizmo_handle_world_len(eye: [f32; 3], center_view: [f32; 3], scene_radius: f32) -> f32 {
    let distance = (arr_to_vec3(eye) - arr_to_vec3(center_view)).length();
    (distance * 0.18)
        .clamp(scene_radius.max(10.0) * 0.03, scene_radius.max(10.0) * 0.30)
        .max(8.0)
}

fn project_gizmo_axis(
    rect: Rect,
    view_proj: Mat4,
    translation: Vec3,
    axis_world: Vec3,
    handle_world_len: f32,
) -> Option<(egui::Pos2, egui::Pos2)> {
    let center_view = to_view_space(translation.to_array());
    let end_view = to_view_space((translation + axis_world * handle_world_len).to_array());
    let (start, _) = project_scene_point(rect, view_proj, center_view)?;
    let (end, _) = project_scene_point(rect, view_proj, end_view)?;
    Some((start, end))
}

fn closest_distance_to_segment(point: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let ab = b - a;
    let ab_len_sq = ab.length_sq();
    if ab_len_sq <= 1.0e-6 {
        return point.distance(a);
    }

    let t = ((point - a).dot(ab) / ab_len_sq).clamp(0.0, 1.0);
    let closest = a + ab * t;
    point.distance(closest)
}

fn signed_angle_2d(a: Vec2, b: Vec2) -> f32 {
    let cross = a.x * b.y - a.y * b.x;
    let dot = a.dot(b);
    cross.atan2(dot)
}

fn scene_rotation_ring_basis(rotation: Quat, axis: GizmoAxis) -> (Vec3, Vec3) {
    match axis {
        GizmoAxis::X => (rotation * Vec3::Y, rotation * Vec3::Z),
        GizmoAxis::Y => (rotation * Vec3::Z, rotation * Vec3::X),
        GizmoAxis::Z => (rotation * Vec3::X, rotation * Vec3::Y),
    }
}

fn scene_rotation_ring_hit_distance(
    rect: Rect,
    view_proj: Mat4,
    translation: Vec3,
    rotation: Quat,
    axis: GizmoAxis,
    ring_radius: f32,
    pointer: egui::Pos2,
) -> Option<f32> {
    let (u, v) = scene_rotation_ring_basis(rotation, axis);
    let mut best: Option<f32> = None;
    let mut previous: Option<egui::Pos2> = None;
    const SEGMENTS: usize = 48;

    for step in 0..=SEGMENTS {
        let t = step as f32 / SEGMENTS as f32;
        let angle = t * std::f32::consts::TAU;
        let world = translation + (u * angle.cos() + v * angle.sin()) * ring_radius;
        let projected = project_scene_point(rect, view_proj, to_view_space(world.to_array()))
            .map(|(pos, _)| pos);

        if let (Some(prev), Some(curr)) = (previous, projected) {
            let distance = closest_distance_to_segment(pointer, prev, curr);
            best = Some(best.map(|value| value.min(distance)).unwrap_or(distance));
        }

        previous = projected;
    }

    best
}

fn scene_gizmo_axis_drag(
    mode: SceneTransformMode,
    pointer_pos: egui::Pos2,
    translation: Vec3,
    rotation: Quat,
    handle_world_len: f32,
    center_screen: egui::Pos2,
    rect: Rect,
    view_proj: Mat4,
) -> Option<SceneGizmoDrag> {
    let mut best: Option<(f32, SceneGizmoDrag)> = None;

    for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
        let axis_world = rotation * scene_transform_axis_basis(axis);
        match mode {
            SceneTransformMode::Translate | SceneTransformMode::Scale => {
                let Some((start, end)) =
                    project_gizmo_axis(rect, view_proj, translation, axis_world, handle_world_len)
                else {
                    continue;
                };
                let axis_screen = end - start;
                let distance = match mode {
                    SceneTransformMode::Translate => {
                        closest_distance_to_segment(pointer_pos, start, end)
                    }
                    SceneTransformMode::Scale => pointer_pos.distance(end),
                    SceneTransformMode::Rotate => unreachable!(),
                };
                let threshold = if matches!(mode, SceneTransformMode::Translate) {
                    10.0
                } else {
                    12.0
                };
                if distance <= threshold {
                    let drag = SceneGizmoDrag {
                        mode,
                        axis,
                        start_pointer: pointer_pos,
                        start_translation: translation,
                        start_rotation: rotation,
                        start_scale: Vec3::ONE,
                        start_pointer_vec: Vec2::ZERO,
                        axis_world: if axis_world.length_squared() > 1.0e-8 {
                            axis_world.normalize()
                        } else {
                            axis_world
                        },
                        axis_screen_dir: axis_screen,
                        handle_world_len,
                        screen_origin: center_screen,
                    };
                    let best_distance = best
                        .as_ref()
                        .map(|(best_distance, _)| *best_distance)
                        .unwrap_or(f32::MAX);
                    if distance < best_distance {
                        best = Some((distance, drag));
                    }
                }
            }
            SceneTransformMode::Rotate => {
                let Some(distance) = scene_rotation_ring_hit_distance(
                    rect,
                    view_proj,
                    translation,
                    rotation,
                    axis,
                    handle_world_len * 0.9,
                    pointer_pos,
                ) else {
                    continue;
                };
                if distance <= 10.0 {
                    let start_pointer_vec = pointer_pos - center_screen;
                    if start_pointer_vec.length_sq() <= 1.0 {
                        continue;
                    }
                    let drag = SceneGizmoDrag {
                        mode,
                        axis,
                        start_pointer: pointer_pos,
                        start_translation: translation,
                        start_rotation: rotation,
                        start_scale: Vec3::ONE,
                        start_pointer_vec,
                        axis_world: axis_world.normalize_or_zero(),
                        axis_screen_dir: Vec2::ZERO,
                        handle_world_len,
                        screen_origin: center_screen,
                    };
                    let best_distance = best
                        .as_ref()
                        .map(|(best_distance, _)| *best_distance)
                        .unwrap_or(f32::MAX);
                    if distance < best_distance {
                        best = Some((distance, drag));
                    }
                }
            }
        }
    }

    best.map(|(_, drag)| drag)
}

fn scene_gizmo_toolbar_button_rects(rect: Rect) -> [Rect; 3] {
    let button_size = egui::vec2(46.0, 24.0);
    let gap = 4.0;
    let padding = 8.0;
    let total_width = button_size.x * 3.0 + gap * 2.0;
    let frame_rect = Rect::from_min_size(
        egui::pos2(
            rect.right() - total_width - padding * 2.0,
            rect.top() + padding,
        ),
        egui::vec2(total_width + padding, button_size.y + padding),
    );

    std::array::from_fn(|index| {
        let min = egui::pos2(
            frame_rect.left() + padding * 0.5 + index as f32 * (button_size.x + gap),
            frame_rect.top() + padding * 0.5,
        );
        Rect::from_min_size(min, button_size)
    })
}

fn interact_scene_gizmo_toolbar(ui: &mut egui::Ui, rect: Rect, state: &mut GeoViewerState) -> bool {
    let mut consumed = false;
    for (index, (mode, label)) in [
        (SceneTransformMode::Translate, "Move"),
        (SceneTransformMode::Rotate, "Rotate"),
        (SceneTransformMode::Scale, "Scale"),
    ]
    .into_iter()
    .enumerate()
    {
        let button_rect = scene_gizmo_toolbar_button_rects(rect)[index];
        let response = ui.interact(
            button_rect,
            ui.id().with(("scene_transform_mode", label)),
            Sense::click(),
        );

        if response.clicked() {
            state.scene_transform_mode = mode;
            state.scene_gizmo_drag = None;
            ui.ctx().request_repaint();
            consumed = true;
        }

        if response.hovered() && ui.input(|i| i.pointer.primary_down()) {
            consumed = true;
        }
    }

    consumed
}

fn paint_scene_gizmo_toolbar(painter: &egui::Painter, rect: Rect, state: &GeoViewerState) {
    let button_rects = scene_gizmo_toolbar_button_rects(rect);
    let outer = button_rects
        .iter()
        .copied()
        .reduce(|a, b| a.union(b))
        .unwrap_or(rect);
    let frame_rect = outer.expand2(egui::vec2(4.0, 4.0));
    painter.rect_filled(frame_rect, 6.0, Color32::from_black_alpha(150));

    for (index, (mode, label)) in [
        (SceneTransformMode::Translate, "Move"),
        (SceneTransformMode::Rotate, "Rotate"),
        (SceneTransformMode::Scale, "Scale"),
    ]
    .into_iter()
    .enumerate()
    {
        let button_rect = button_rects[index];
        let fill = if state.scene_transform_mode == mode {
            Color32::from_rgb(52, 96, 148)
        } else {
            Color32::from_gray(42)
        };

        painter.rect_filled(button_rect, 4.0, fill);
        painter.text(
            button_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
    }
}

fn handle_scene_gizmo_interaction(
    ui: &mut egui::Ui,
    response: &egui::Response,
    rect: Rect,
    view_proj: Mat4,
    scn: &mut ScnFile,
    state: &mut GeoViewerState,
    selected: Option<SceneSelection>,
    hidden_nodes: &BTreeSet<usize>,
    hidden_chunks: &BTreeSet<usize>,
    scene_radius: f32,
) -> bool {
    let Some(selected_item) = selected else {
        state.scene_gizmo_drag = None;
        return false;
    };
    if !selected_scene_is_visible(Some(selected_item), hidden_nodes, hidden_chunks) {
        state.scene_gizmo_drag = None;
        return false;
    }

    let Some((translation, rotation, scale)) =
        selected_scene_transform(scn, Some(selected_item), hidden_nodes, hidden_chunks)
    else {
        state.scene_gizmo_drag = None;
        return false;
    };

    let center_view = to_view_space(translation.to_array());
    let Some((center_screen, _)) = project_scene_point(rect, view_proj, center_view) else {
        state.scene_gizmo_drag = None;
        return false;
    };
    let handle_world_len = scene_gizmo_handle_world_len(state.eye, center_view, scene_radius);

    if let Some(drag) = state.scene_gizmo_drag {
        if !ui.input(|i| i.pointer.primary_down()) {
            state.scene_gizmo_drag = None;
            return false;
        }

        let Some(pointer_pos) = ui.input(|i| i.pointer.interact_pos()) else {
            return true;
        };

        let mut new_translation = drag.start_translation;
        let mut new_rotation = drag.start_rotation;
        let mut new_scale = drag.start_scale;

        match drag.mode {
            SceneTransformMode::Translate => {
                let axis_len = drag.axis_screen_dir.length().max(8.0);
                let axis_dir = if drag.axis_screen_dir.length_sq() > 1.0e-6 {
                    drag.axis_screen_dir / axis_len
                } else {
                    Vec2::ZERO
                };
                let delta_pixels = (pointer_pos - drag.start_pointer).dot(axis_dir);
                let world_delta =
                    drag.axis_world * (delta_pixels * drag.handle_world_len / axis_len);
                new_translation = drag.start_translation + world_delta;
            }
            SceneTransformMode::Rotate => {
                let current_vec = pointer_pos - drag.screen_origin;
                if current_vec.length_sq() > 1.0 {
                    let angle = signed_angle_2d(drag.start_pointer_vec, current_vec);
                    new_rotation = (drag.start_rotation
                        * Quat::from_axis_angle(scene_transform_axis_basis(drag.axis), angle))
                    .normalize();
                }
            }
            SceneTransformMode::Scale => {
                let axis_len = drag.axis_screen_dir.length().max(16.0);
                let axis_dir = if drag.axis_screen_dir.length_sq() > 1.0e-6 {
                    drag.axis_screen_dir / axis_len
                } else {
                    Vec2::ZERO
                };
                let delta_pixels = (pointer_pos - drag.start_pointer).dot(axis_dir);
                let scale_factor = (1.0 + delta_pixels / 120.0).max(0.05);
                new_scale = drag.start_scale;
                match drag.axis {
                    GizmoAxis::X => new_scale.x = (drag.start_scale.x * scale_factor).max(0.05),
                    GizmoAxis::Y => new_scale.y = (drag.start_scale.y * scale_factor).max(0.05),
                    GizmoAxis::Z => new_scale.z = (drag.start_scale.z * scale_factor).max(0.05),
                }
            }
        }

        if !write_selected_scene_transform(
            scn,
            selected_item,
            new_translation,
            new_rotation,
            new_scale,
        ) {
            state.scene_gizmo_drag = None;
            return false;
        }

        state.scene_edit_revision = state.scene_edit_revision.wrapping_add(1);
        state.scene_key = None;
        ui.ctx().request_repaint();
        return true;
    }

    let wants_drag = response.drag_started_by(egui::PointerButton::Primary)
        || (response.hovered() && ui.input(|i| i.pointer.primary_pressed()));

    if !wants_drag || ui.input(|i| i.modifiers.ctrl) {
        return false;
    }

    let Some(pointer_pos) = ui
        .input(|i| i.pointer.interact_pos())
        .or_else(|| response.interact_pointer_pos())
    else {
        return false;
    };

    let Some(mut drag) = scene_gizmo_axis_drag(
        state.scene_transform_mode,
        pointer_pos,
        translation,
        rotation,
        handle_world_len,
        center_screen,
        rect,
        view_proj,
    ) else {
        return false;
    };
    drag.start_scale = scale;
    state.scene_gizmo_drag = Some(drag);
    ui.ctx().request_repaint();
    true
}

fn paint_scene_gizmo(
    painter: &egui::Painter,
    rect: Rect,
    view_proj: Mat4,
    scn: &ScnFile,
    state: &GeoViewerState,
    selected: Option<SceneSelection>,
    hidden_nodes: &BTreeSet<usize>,
    hidden_chunks: &BTreeSet<usize>,
    scene_radius: f32,
) {
    if !selected_scene_is_visible(selected, hidden_nodes, hidden_chunks) {
        return;
    };
    let Some((translation, rotation, _)) =
        selected_scene_transform(scn, selected, hidden_nodes, hidden_chunks)
    else {
        return;
    };

    let center_view = to_view_space(translation.to_array());
    let Some((center_screen, _)) = project_scene_point(rect, view_proj, center_view) else {
        return;
    };
    let handle_world_len = scene_gizmo_handle_world_len(state.eye, center_view, scene_radius);
    let active_axis = state.scene_gizmo_drag.map(|drag| drag.axis);

    painter.circle_filled(
        center_screen,
        4.0,
        Color32::from_rgba_unmultiplied(235, 240, 248, 210),
    );

    match state.scene_transform_mode {
        SceneTransformMode::Translate | SceneTransformMode::Scale => {
            for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                let axis_world = rotation * scene_transform_axis_basis(axis);
                let Some((start, end)) =
                    project_gizmo_axis(rect, view_proj, translation, axis_world, handle_world_len)
                else {
                    continue;
                };

                let mut color = scene_transform_axis_color(axis);
                if active_axis == Some(axis) {
                    color = color.gamma_multiply(1.35);
                }
                let stroke =
                    egui::Stroke::new(if active_axis == Some(axis) { 3.5 } else { 2.4 }, color);
                painter.line_segment([start, end], stroke);

                if matches!(state.scene_transform_mode, SceneTransformMode::Scale) {
                    let end_rect = Rect::from_center_size(end, egui::vec2(8.0, 8.0));
                    painter.rect_filled(end_rect, 1.5, color);
                } else {
                    painter.circle_filled(end, 4.0, color);
                }
            }
        }
        SceneTransformMode::Rotate => {
            for axis in [GizmoAxis::X, GizmoAxis::Y, GizmoAxis::Z] {
                let mut color = scene_transform_axis_color(axis);
                if active_axis == Some(axis) {
                    color = color.gamma_multiply(1.35);
                }
                let stroke =
                    egui::Stroke::new(if active_axis == Some(axis) { 3.2 } else { 2.0 }, color);
                let (u, v) = scene_rotation_ring_basis(rotation, axis);
                let mut previous: Option<egui::Pos2> = None;
                const SEGMENTS: usize = 48;

                for step in 0..=SEGMENTS {
                    let t = step as f32 / SEGMENTS as f32;
                    let angle = t * std::f32::consts::TAU;
                    let world = translation
                        + (u * angle.cos() + v * angle.sin()) * (handle_world_len * 0.9);
                    let current =
                        project_scene_point(rect, view_proj, to_view_space(world.to_array()))
                            .map(|(pos, _)| pos);

                    if let (Some(prev), Some(curr)) = (previous, current) {
                        painter.line_segment([prev, curr], stroke);
                    }
                    previous = current;
                }
            }
        }
    }
}

fn build_ground_lines_for_bounds(center: [f32; 3], _radius: f32, ground_y: f32) -> Vec<LineVertex> {
    let mut out = Vec::new();

    const GRID_HALF: f32 = 100.0;
    const GRID_STEP: f32 = 10.0;

    let center_x = center[0];
    let center_z = center[2];

    let major = [0.42, 0.42, 0.42, 0.95];
    let minor = [0.26, 0.26, 0.26, 0.75];

    let mut x = -GRID_HALF;
    while x <= GRID_HALF + 0.5 * GRID_STEP {
        let color = if x.abs() < GRID_STEP * 0.5 {
            major
        } else {
            minor
        };

        let a = [center_x + x, ground_y, center_z - GRID_HALF];
        let b = [center_x + x, ground_y, center_z + GRID_HALF];

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });

        x += GRID_STEP;
    }

    let mut z = -GRID_HALF;
    while z <= GRID_HALF + 0.5 * GRID_STEP {
        let color = if z.abs() < GRID_STEP * 0.5 {
            major
        } else {
            minor
        };

        let a = [center_x - GRID_HALF, ground_y, center_z + z];
        let b = [center_x + GRID_HALF, ground_y, center_z + z];

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });

        z += GRID_STEP;
    }

    out
}

fn apply_transform_point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

fn apply_transform_direction(m: &[f32; 16], v: [f32; 3]) -> [f32; 3] {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2],
    ]
}

fn build_wire_lines(geo: &GeoFile) -> Vec<LineVertex> {
    let color = [120.0 / 255.0, 220.0 / 255.0, 1.0, 1.0];
    let mut out = Vec::with_capacity(geo.faces.len() * 6);
    for face in &geo.faces {
        let ia = face[0] as usize;
        let ib = face[1] as usize;
        let ic = face[2] as usize;
        let Some(&a_raw) = geo.verts.get(ia) else {
            continue;
        };
        let Some(&b_raw) = geo.verts.get(ib) else {
            continue;
        };
        let Some(&c_raw) = geo.verts.get(ic) else {
            continue;
        };
        let a = to_view_space(a_raw);
        let b = to_view_space(b_raw);
        let c = to_view_space(c_raw);

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });
        out.push(LineVertex { position: b, color });
        out.push(LineVertex { position: c, color });
        out.push(LineVertex { position: c, color });
        out.push(LineVertex { position: a, color });
    }
    out
}

fn build_ground_lines(geo: &GeoFile, center: [f32; 3], _radius: f32) -> Vec<LineVertex> {
    let mut out = Vec::new();

    let ground_y = geo_ground_y(geo) - 0.02;

    const GRID_HALF: f32 = 200.0;
    const GRID_STEP: f32 = 20.0;

    let center_x = center[0];
    let center_z = center[2];

    let major = [0.42, 0.42, 0.42, 0.95];
    let minor = [0.26, 0.26, 0.26, 0.75];

    let mut x = -GRID_HALF;
    while x <= GRID_HALF + 0.5 * GRID_STEP {
        let color = if x.abs() < GRID_STEP * 0.5 {
            major
        } else {
            minor
        };

        let a = [center_x + x, ground_y, center_z - GRID_HALF];
        let b = [center_x + x, ground_y, center_z + GRID_HALF];

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });

        x += GRID_STEP;
    }

    let mut z = -GRID_HALF;
    while z <= GRID_HALF + 0.5 * GRID_STEP {
        let color = if z.abs() < GRID_STEP * 0.5 {
            major
        } else {
            minor
        };

        let a = [center_x - GRID_HALF, ground_y, center_z + z];
        let b = [center_x + GRID_HALF, ground_y, center_z + z];

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });

        z += GRID_STEP;
    }

    out
}

fn build_bone_lines(skeleton: &GeoSkeleton) -> Vec<LineVertex> {
    let color = [1.0, 0.92, 0.35, 1.0];
    let points: Vec<[f32; 3]> = skeleton
        .bind_matrices
        .iter()
        .map(|mat| to_view_space([mat[12], mat[13], mat[14]]))
        .collect();

    let mut out = Vec::new();
    for (bone_index, parent) in skeleton.parent.iter().enumerate() {
        let Some(parent_index) = *parent else {
            continue;
        };
        let Some(&a) = points.get(parent_index) else {
            continue;
        };
        let Some(&b) = points.get(bone_index) else {
            continue;
        };
        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });
    }
    out
}

fn camera_axes(yaw: f32, pitch: f32) -> (Vec3, Vec3, Vec3) {
    let pitch = pitch.clamp(-1.10, 1.10);
    let forward = Vec3::new(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        yaw.cos() * pitch.cos(),
    )
    .normalize();
    let right = forward.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();
    (forward, right, up)
}

fn vec3_to_arr(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

fn arr_to_vec3(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

fn eye_from_target(yaw: f32, pitch: f32, distance: f32, target: [f32; 3]) -> [f32; 3] {
    let (forward, _, _) = camera_axes(yaw, pitch);
    vec3_to_arr(arr_to_vec3(target) - forward * distance.max(NEAR_PLANE + 0.01))
}

fn orbit_eye_around_target(state: &mut GeoViewerState, target: [f32; 3], delta: Vec2) {
    let current_distance = (arr_to_vec3(state.eye) - arr_to_vec3(target))
        .length()
        .max(NEAR_PLANE + 0.01);

    state.distance = current_distance;
    state.yaw -= delta.x * 0.01;
    state.pitch = (state.pitch - delta.y * 0.01).clamp(-1.10, 1.10);
    state.eye = eye_from_target(state.yaw, state.pitch, state.distance, target);
}

fn apply_viewer_input(
    ui: &mut egui::Ui,
    response: &egui::Response,
    state: &mut GeoViewerState,
    scene_radius: f32,
    _min_distance: f32,
    _max_distance: f32,
    orbit_center: Option<[f32; 3]>,
) {
    let ctrl_held = ui.input(|i| i.modifiers.ctrl);

    if ctrl_held && response.dragged_by(egui::PointerButton::Primary) {
        let delta = ui.input(|i| i.pointer.delta());

        if let Some(center) = orbit_center {
            orbit_eye_around_target(state, center, delta);
        } else {
            state.yaw -= delta.x * 0.01;
            state.pitch = (state.pitch - delta.y * 0.01).clamp(-1.10, 1.10);
        }

        ui.ctx().request_repaint();
    } else if response.dragged_by(egui::PointerButton::Secondary) {
        let delta = ui.input(|i| i.pointer.delta());
        state.yaw -= delta.x * 0.01;
        state.pitch = (state.pitch - delta.y * 0.01).clamp(-1.10, 1.10);
        ui.ctx().request_repaint();
    }

    if response.dragged_by(egui::PointerButton::Middle) {
        let delta = ui.input(|i| i.pointer.delta());
        let (_, right, up) = camera_axes(state.yaw, state.pitch);

        // Keep pan speed independent from fly speed.
        let pan_scale = (scene_radius * 0.00008).clamp(0.5, 1.0);

        let eye =
            arr_to_vec3(state.eye) + right * (-delta.x * pan_scale) + up * (delta.y * pan_scale);

        state.eye = vec3_to_arr(eye);
        ui.ctx().request_repaint();
    }

    if response.hovered() {
        let (forward, right, _) = camera_axes(state.yaw, state.pitch);

        let fly_base_speed = (state.distance * 1.2)
            .max(scene_radius * 0.015)
            .clamp(1.0, 800.0);

        let zoom_speed = (state.distance * 0.5)
            .max(scene_radius * 0.01)
            .clamp(2.0, 600.0);

        let dt = ui.input(|i| i.unstable_dt).clamp(1.0 / 240.0, 1.0 / 20.0);

        let shift = ui.input(|i| i.modifiers.shift);

        let scroll_y = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if scroll_y.abs() > 0.0 {
            let eye = arr_to_vec3(state.eye) + forward * (scroll_y * zoom_speed * 0.02);
            state.eye = vec3_to_arr(eye);
            ui.ctx().request_repaint();
        }

        let mut move_dir = Vec3::ZERO;

        if ui.input(|i| i.key_down(egui::Key::W)) {
            move_dir += forward;
        }
        if ui.input(|i| i.key_down(egui::Key::S)) {
            move_dir -= forward;
        }
        if ui.input(|i| i.key_down(egui::Key::A)) {
            move_dir -= right;
        }
        if ui.input(|i| i.key_down(egui::Key::D)) {
            move_dir += right;
        }
        if ui.input(|i| i.key_down(egui::Key::Q)) {
            move_dir -= Vec3::Y;
        }
        if ui.input(|i| i.key_down(egui::Key::E)) {
            move_dir += Vec3::Y;
        }

        if move_dir.length_squared() > 0.0 {
            let boost = if shift { 4.0 } else { 1.0 };
            let speed = fly_base_speed * state.move_speed.max(0.1) * boost * dt;

            let eye = arr_to_vec3(state.eye) + move_dir.normalize() * speed;
            state.eye = vec3_to_arr(eye);
            ui.ctx().request_repaint();
        }
    }
}

fn make_view_proj(
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    eye: [f32; 3],
    scene: &CpuScene,
) -> Mat4 {
    let pitch = pitch.clamp(-1.10, 1.10);

    let (forward, _right, up) = camera_axes(yaw, pitch);
    let eye = Vec3::new(eye[0], eye[1], eye[2]);
    let target = eye + forward;

    let aspect = (width as f32 / (height.max(1) as f32)).max(0.01);
    let proj = Mat4::perspective_rh_gl(
        55.0_f32.to_radians(),
        aspect,
        NEAR_PLANE,
        (scene.radius * 20.0).max(100.0),
    );
    let view = Mat4::look_at_rh(eye, target, up);
    proj * view
}

fn texture_state_hash(textures: &[Option<DdsPreview>]) -> u64 {
    let mut hash = 1469598103934665603u64;
    for tex in textures {
        match tex {
            Some(t) => {
                hash ^= 0x9e3779b97f4a7c15u64;
                hash = hash.wrapping_mul(1099511628211);
                hash ^= t.width as u64;
                hash = hash.wrapping_mul(1099511628211);
                hash ^= (t.height as u64) << 1;
                hash = hash.wrapping_mul(1099511628211);
            }
            None => {
                hash ^= 0x517cc1b727220a95u64;
                hash = hash.wrapping_mul(1099511628211);
            }
        }
    }
    hash
}

fn normalize4(v: [f32; 4]) -> [f32; 4] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1.0e-6);
    [v[0] / len, v[1] / len, v[2] / len, v[3]]
}

fn geo_ground_y(geo: &GeoFile) -> f32 {
    if geo.verts.is_empty() {
        return 0.0;
    }

    let mut min_y = to_view_space(geo.verts[0])[1];
    for v in &geo.verts {
        let vv = to_view_space(*v);
        min_y = min_y.min(vv[1]);
    }

    min_y
}

fn geo_bounds(geo: &GeoFile) -> ([f32; 3], f32) {
    if geo.verts.is_empty() {
        return ([0.0, 0.0, 0.0], 1.0);
    }

    let mut min = to_view_space(geo.verts[0]);
    let mut max = to_view_space(geo.verts[0]);

    for v in &geo.verts {
        let vv = to_view_space(*v);
        min[0] = min[0].min(vv[0]);
        min[1] = min[1].min(vv[1]);
        min[2] = min[2].min(vv[2]);
        max[0] = max[0].max(vv[0]);
        max[1] = max[1].max(vv[1]);
        max[2] = max[2].max(vv[2]);
    }

    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];

    let mut radius: f32 = 0.0;
    for v in &geo.verts {
        let vv = to_view_space(*v);
        radius = radius.max(length3(sub3(vv, center)));
    }

    (center, radius.max(1.0))
}

fn to_view_space(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(v: [f32; 3], s: f32) -> [f32; 3] {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn length3(v: [f32; 3]) -> f32 {
    dot3(v, v).sqrt()
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = length3(v);
    if len <= 1.0e-6 {
        [0.0, 1.0, 0.0]
    } else {
        scale3(v, 1.0 / len)
    }
}

fn is_shadow_like_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();

    n.contains("shadow") || n.contains("blob") || n.contains("decal")
}

const FACE_SHADER: &str = r#"
struct Globals {
    view_proj : mat4x4<f32>,
    light_dir : vec4<f32>,
    render_opts : vec4<f32>, // x = textures on/off, y = brightness multiplier
};

struct MaterialParams {
    layer_count : u32,
    record_kind : u32,
    _pad0 : u32,
    _pad1 : u32,
};

@group(0) @binding(0)
var<uniform> globals : Globals;

@group(1) @binding(0)
var tex_sampler : sampler;

@group(1) @binding(1)
var<uniform> material : MaterialParams;

@group(1) @binding(2)
var tex0 : texture_2d<f32>;
@group(1) @binding(3)
var tex1 : texture_2d<f32>;
@group(1) @binding(4)
var tex2 : texture_2d<f32>;
@group(1) @binding(5)
var tex3 : texture_2d<f32>;
@group(1) @binding(6)
var tex4 : texture_2d<f32>;
@group(1) @binding(7)
var tex5 : texture_2d<f32>;
@group(1) @binding(8)
var tex6 : texture_2d<f32>;
@group(1) @binding(9)
var tex7 : texture_2d<f32>;

struct VsIn {
    @location(0) position : vec3<f32>,
    @location(1) normal   : vec3<f32>,
    @location(2) color    : vec4<f32>,
    @location(3) uv       : vec2<f32>,
};

struct VsOut {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) normal : vec3<f32>,
    @location(1) uv     : vec2<f32>,
    @location(2) color  : vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out : VsOut;
    out.clip_pos = globals.view_proj * vec4<f32>(v.position, 1.0);
    out.normal = v.normal;
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

fn sample_layer(i: u32, uv: vec2<f32>) -> vec4<f32> {
    switch i {
        case 0u: { return textureSample(tex0, tex_sampler, uv); }
        case 1u: { return textureSample(tex1, tex_sampler, uv); }
        case 2u: { return textureSample(tex2, tex_sampler, uv); }
        case 3u: { return textureSample(tex3, tex_sampler, uv); }
        case 4u: { return textureSample(tex4, tex_sampler, uv); }
        case 5u: { return textureSample(tex5, tex_sampler, uv); }
        case 6u: { return textureSample(tex6, tex_sampler, uv); }
        case 7u: { return textureSample(tex7, tex_sampler, uv); }
        default: { return vec4<f32>(1.0, 1.0, 1.0, 1.0); }
    }
}

fn sample_scn_layers(uv: vec2<f32>, vcolor: vec4<f32>) -> vec4<f32> {
    let count = material.layer_count;

    if (count <= 1u) {
        return sample_layer(0u, uv);
    }

    // Generic, not name-based:
    // use vertex RGB as blend weights for layers 1..3
    // and the remaining weight for layer 0.
    let w1 = clamp(vcolor.r, 0.0, 1.0);
    let w2 = clamp(vcolor.g, 0.0, 1.0);
    let w3 = clamp(vcolor.b, 0.0, 1.0);
    let w0 = max(0.0, 1.0 - (w1 + w2 + w3));

    var accum = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var total = 0.0;

    accum += sample_layer(0u, uv) * w0;
    total += w0;

    if (count > 1u) {
        accum += sample_layer(1u, uv) * w1;
        total += w1;
    }
    if (count > 2u) {
        accum += sample_layer(2u, uv) * w2;
        total += w2;
    }
    if (count > 3u) {
        accum += sample_layer(3u, uv) * w3;
        total += w3;
    }

    // For extra layers beyond 4, mix in their average softly.
    if (count > 4u) {
        var extra = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        var extra_count = 0.0;

        for (var i = 4u; i < count && i < 8u; i = i + 1u) {
            extra += sample_layer(i, uv);
            extra_count += 1.0;
        }

        if (extra_count > 0.0) {
            let extra_avg = extra / extra_count;
            accum = mix(accum, extra_avg, 0.35);
        }
    }

    if (total > 0.0001) {
        return accum / total;
    }

    return sample_layer(0u, uv);
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    let textured = globals.render_opts.x > 0.5;
    let selection_mix = clamp(v.color.a, 0.0, 1.0) * 0.62;

    var base_rgb : vec3<f32>;
    var out_alpha : f32;

    if (textured) {
        let texel = sample_scn_layers(v.uv, v.color);

        if (texel.a < 0.1) {
            discard;
        }

        base_rgb = texel.rgb;
        out_alpha = texel.a;
    } else {
        base_rgb = vec3<f32>(0.50, 0.50, 0.50);
        out_alpha = 1.0;
    }

    let selected_rgb = min(
        base_rgb * vec3<f32>(0.55, 0.86, 1.30) + vec3<f32>(0.08, 0.11, 0.18),
        vec3<f32>(1.0, 1.0, 1.0)
    );
    let display_rgb = mix(base_rgb, selected_rgb, selection_mix);
    let ndotl = abs(dot(normalize(v.normal), normalize(globals.light_dir.xyz)));
    let shade = (0.45 + 0.55 * ndotl) * globals.render_opts.y;
    let lit_rgb = min(display_rgb * shade, vec3<f32>(1.0, 1.0, 1.0));

    return vec4<f32>(lit_rgb, out_alpha);
}
"#;

const LINE_SHADER: &str = r#"
struct Globals {
    view_proj : mat4x4<f32>,
    light_dir : vec4<f32>,
    render_opts : vec4<f32>,
};

@group(0) @binding(0)
var<uniform> globals : Globals;

struct VsIn {
    @location(0) position : vec3<f32>,
    @location(1) color    : vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) color : vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out : VsOut;
    out.clip_pos = globals.view_proj * vec4<f32>(v.position, 1.0);
    out.color = v.color;
    return out;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    return v.color;
}
"#;

const BLIT_SHADER: &str = r#"
struct VsOut {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) uv : vec2<f32>,
};

@group(0) @binding(0)
var blit_sampler : sampler;

@group(0) @binding(1)
var blit_tex : texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index : u32) -> VsOut {
    var out : VsOut;
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0,  1.0),
        vec2<f32>( 3.0,  1.0)
    );
    let p = pos[vertex_index];
    out.clip_pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = 0.5 * vec2<f32>(p.x + 1.0, 1.0 - p.y);
    return out;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    return textureSample(blit_tex, blit_sampler, v.uv);
}
"#;
