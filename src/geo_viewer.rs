use crate::{
    dds_preview::DdsPreview,
    geo::{GeoFile, GeoSkeleton},
};
use bytemuck::{Pod, Zeroable};
use eframe::{
    egui::{self, Color32, Rect, Sense, Vec2},
    egui_wgpu::{self, wgpu},
};
use glam::{Mat4, Vec3};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use wgpu::util::DeviceExt;

const NEAR_PLANE: f32 = 0.05;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;
const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct GeoViewerState {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub pan: Vec2,
    pub show_faces: bool,
    pub show_textures: bool,
    pub show_wireframe: bool,
    pub show_bones: bool,
    pub cull_backfaces: bool,

    scene_key: Option<String>,
    cpu_scene: Option<Arc<CpuScene>>,
}

impl Default for GeoViewerState {
    fn default() -> Self {
        Self {
            yaw: std::f32::consts::PI + 0.35,
            pitch: -0.35,
            distance: 10.0,
            pan: Vec2::ZERO,
            show_faces: true,
            show_textures: true,
            show_wireframe: false,
            show_bones: true,
            cull_backfaces: false,
            scene_key: None,
            cpu_scene: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MeshVertex {
    position: [f32; 3],
    normal: [f32; 3],
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

struct CpuPart {
    indices: Vec<u32>,
    texture: Option<CpuTexture>,
}

struct CpuScene {
    key: String,
    vertices: Vec<MeshVertex>,
    parts: Vec<CpuPart>,
    ground_lines: Vec<LineVertex>,
    wire_lines: Vec<LineVertex>,
    bone_lines: Vec<LineVertex>,
    center: [f32; 3],
    radius: f32,
}

struct GpuPart {
    index_buffer: wgpu::Buffer,
    index_count: u32,
    bind_group: wgpu::BindGroup,
    _texture_keepalive: wgpu::Texture,
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
}

struct GeoGpuCallback {
    rect: Rect,
    scene: Arc<CpuScene>,
    yaw: f32,
    pitch: f32,
    distance: f32,
    pan: Vec2,
    show_faces: bool,
    show_textures: bool,
    show_wireframe: bool,
    show_bones: bool,
    cull_backfaces: bool,
    frame_targets: Arc<Mutex<Option<FrameTargets>>>,
}

pub fn reset_geo_viewer(state: &mut GeoViewerState, geo: &GeoFile) {
    let (_center, radius) = geo_bounds(geo);
    state.yaw = std::f32::consts::PI + 0.35;
    state.pitch = -0.35;
    state.distance = (radius * 3.0).max(4.0);
    state.pan = Vec2::ZERO;
}

pub fn draw_geo_viewer(
    ui: &mut egui::Ui,
    geo: &GeoFile,
    textures: &[Option<DdsPreview>],
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

            ui.checkbox(&mut state.cull_backfaces, "Cull");

            ui.separator();
            ui.small("GPU viewport: LMB orbit | RMB drag pan | wheel zoom");
        });

    let desired_height = viewer_height.clamp(260.0, 900.0);
    let desired_size = egui::vec2(ui.available_width().max(200.0), desired_height);
    let (response, painter) = ui.allocate_painter(desired_size, Sense::click_and_drag());
    let rect = response.rect;

    painter.rect_filled(rect, 0.0, Color32::from_rgb(30, 30, 34));

    if response.dragged_by(egui::PointerButton::Primary) {
        let delta = ui.input(|i| i.pointer.delta());
        state.yaw -= delta.x * 0.01;
        state.pitch = (state.pitch - delta.y * 0.01).clamp(-1.10, 1.10);
        ui.ctx().request_repaint();
    }

    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        let delta = ui.input(|i| i.pointer.delta());
        state.pan += delta;
        ui.ctx().request_repaint();
    }

    let min_distance = NEAR_PLANE + 0.01;
    let max_distance = (geo_bounds(geo).1 * 6.0).max(25.0);

    if response.hovered() {
        let scroll_y = ui.ctx().input(|i| i.smooth_scroll_delta.y);
        if scroll_y.abs() > 0.0 {
            let zoom_factor = 0.9985_f32.powf(scroll_y);
            state.distance = (state.distance * zoom_factor).clamp(min_distance, max_distance);
            ui.ctx().request_repaint();
        }
    }

    state.distance = state.distance.clamp(min_distance, max_distance);

    let texture_state = texture_state_hash(textures);
    let scene_key = format!("{}#{}", geo.path.display(), texture_state);
    if state.scene_key.as_deref() != Some(scene_key.as_str()) {
        state.cpu_scene = Some(build_cpu_scene(geo, textures, scene_key.clone()));
        state.scene_key = Some(scene_key);
    }

    let Some(scene) = state.cpu_scene.clone() else {
        return;
    };

    let callback = egui_wgpu::Callback::new_paint_callback(
        rect,
        GeoGpuCallback {
            rect,
            scene,
            yaw: state.yaw,
            pitch: state.pitch,
            distance: state.distance,
            pan: state.pan,
            show_faces: state.show_faces,
            show_textures: state.show_textures,
            show_wireframe: state.show_wireframe,
            show_bones: state.show_bones,
            cull_backfaces: state.cull_backfaces,
            frame_targets: Arc::new(Mutex::new(None)),
        },
    );

    painter.add(callback);
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
            scene.clone()
        } else {
            let uploaded = Arc::new(upload_scene(device, queue, shared, &self.scene));
            shared.scenes.insert(self.scene.key.clone(), uploaded.clone());
            uploaded
        };

        let width = (self.rect.width() * screen_descriptor.pixels_per_point)
            .round()
            .max(1.0) as u32;
        let height = (self.rect.height() * screen_descriptor.pixels_per_point)
            .round()
            .max(1.0) as u32;

        let mut frame_guard = self.frame_targets.lock().expect("frame target mutex poisoned");
        let recreate = frame_guard
            .as_ref()
            .map(|f| f.width != width || f.height != height)
            .unwrap_or(true);
        if recreate {
            *frame_guard = Some(create_frame_targets(device, shared, width, height));
        }
        let frame = frame_guard.as_ref().expect("frame targets should exist");

        let view_proj = make_view_proj(
            width,
            height,
            self.yaw,
            self.pitch,
            self.distance,
            self.pan,
            &self.scene,
        );
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

            if self.show_faces {
                let pipeline = if self.cull_backfaces {
                    &shared.face_pipeline_cull
                } else {
                    &shared.face_pipeline_no_cull
                };
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &gpu_scene.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, gpu_scene.vertex_buffer.slice(..));
                for part in &gpu_scene.parts {
                    pass.set_bind_group(1, &part.bind_group, &[]);
                    pass.set_index_buffer(part.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..part.index_count, 0, 0..1);
                }
            }

            if let Some(ground) = &gpu_scene.ground_lines {
                pass.set_pipeline(&shared.line_pipeline_overlay);
                pass.set_bind_group(0, &gpu_scene.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, ground.buffer.slice(..));
                pass.draw(0..ground.vertex_count, 0..1);
            }

            if self.show_wireframe {
                if let Some(wire) = &gpu_scene.wire_lines {
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

        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geo_material_layout"),
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
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
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

fn upload_scene(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    shared: &GpuShared,
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
            label: Some("geo_part_indices"),
            contents: bytemuck::cast_slice(&part.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (texture, view) = if let Some(tex) = &part.texture {
            let size = wgpu::Extent3d {
                width: tex.width.max(1),
                height: tex.height.max(1),
                depth_or_array_layers: 1,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("geo_material_texture"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OFFSCREEN_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &tex.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * tex.width.max(1)),
                    rows_per_image: Some(tex.height.max(1)),
                },
                size,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        } else {
            let white = [255u8, 255, 255, 255];
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("geo_white_texture"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: OFFSCREEN_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &white,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        };

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("geo_material_bind_group"),
            layout: &shared.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&shared.texture_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });

        parts.push(GpuPart {
            index_buffer,
            index_count: part.indices.len() as u32,
            bind_group,
            _texture_keepalive: texture,
        });
    }

    GpuScene {
        vertex_buffer,
        globals_buffer,
        globals_bind_group,
        parts,
        ground_lines: upload_lines(device, &scene.ground_lines, "geo_ground_lines"),
        wire_lines: upload_lines(device, &scene.wire_lines, "geo_wire_lines"),
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

fn build_cpu_scene(geo: &GeoFile, textures: &[Option<DdsPreview>], key: String) -> Arc<CpuScene> {
    let (center, radius) = geo_bounds(geo);

    let vertices: Vec<MeshVertex> = geo
        .verts
        .iter()
        .enumerate()
        .map(|(i, &p)| MeshVertex {
            position: to_view_space(p),
            normal: geo
                .normals
                .get(i)
                .copied()
                .map(to_view_space)
                .map(normalize3)
                .unwrap_or([0.0, 1.0, 0.0]),
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
            texture: None,
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

            let texture = textures
                .get(subset.material)
                .and_then(|t| t.as_ref())
                .map(|dds| CpuTexture {
                    width: dds.width as u32,
                    height: dds.height as u32,
                    rgba: dds.rgba_pixels.clone(),
                });

            parts.push(CpuPart { indices, texture });
        }
    }

    let ground_lines = build_ground_lines(geo, center, radius);
    let wire_lines = build_wire_lines(geo);
    let bone_lines = geo
        .skeleton
        .as_ref()
        .map(build_bone_lines)
        .unwrap_or_default();

    Arc::new(CpuScene {
        key,
        vertices,
        parts,
        ground_lines,
        wire_lines,
        bone_lines,
        center,
        radius,
    })
}

fn build_wire_lines(geo: &GeoFile) -> Vec<LineVertex> {
    let color = [120.0 / 255.0, 220.0 / 255.0, 1.0, 1.0];
    let mut out = Vec::with_capacity(geo.faces.len() * 6);
    for face in &geo.faces {
        let ia = face[0] as usize;
        let ib = face[1] as usize;
        let ic = face[2] as usize;
        let Some(&a_raw) = geo.verts.get(ia) else { continue; };
        let Some(&b_raw) = geo.verts.get(ib) else { continue; };
        let Some(&c_raw) = geo.verts.get(ic) else { continue; };
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

fn build_ground_lines(geo: &GeoFile, center: [f32; 3], radius: f32) -> Vec<LineVertex> {
    let mut out = Vec::new();

    let ground_y = geo_ground_y(geo) - 0.02;
    let half = (radius * 2.5).max(6.0);
    let step = (half / 16.0).max(0.25);

    let center_x = center[0];
    let center_z = center[2];

    let major = [0.42, 0.42, 0.42, 0.95];
    let minor = [0.26, 0.26, 0.26, 0.75];

    let mut x = -half;
    while x <= half + 0.5 * step {
        let color = if x.abs() < step * 0.5 { major } else { minor };

        let a = [center_x + x, ground_y, center_z - half];
        let b = [center_x + x, ground_y, center_z + half];

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });

        x += step;
    }

    let mut z = -half;
    while z <= half + 0.5 * step {
        let color = if z.abs() < step * 0.5 { major } else { minor };

        let a = [center_x - half, ground_y, center_z + z];
        let b = [center_x + half, ground_y, center_z + z];

        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });

        z += step;
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
        let Some(parent_index) = *parent else { continue; };
        let Some(&a) = points.get(parent_index) else { continue; };
        let Some(&b) = points.get(bone_index) else { continue; };
        out.push(LineVertex { position: a, color });
        out.push(LineVertex { position: b, color });
    }
    out
}

fn make_view_proj(
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    distance: f32,
    pan: Vec2,
    scene: &CpuScene,
) -> Mat4 {
    let center = Vec3::new(scene.center[0], scene.center[1], scene.center[2]);
    let pitch = pitch.clamp(-1.10, 1.10);
    let distance = distance.max(NEAR_PLANE + 0.01);

    let dir = Vec3::new(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos()).normalize();
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(dir).normalize_or_zero();

    let pan_scale = (scene.radius * 0.0025).max(0.002);
    let target = center + right * (-pan.x * pan_scale) + up * (pan.y * pan_scale);
    let eye = target - dir * distance;

    let aspect = (width as f32 / (height.max(1) as f32)).max(0.01);
    let proj = Mat4::perspective_rh_gl(
        55.0_f32.to_radians(),
        aspect,
        NEAR_PLANE,
        (scene.radius * 20.0).max(100.0),
    );
    let view = Mat4::look_at_rh(eye, target, Vec3::Y);
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

const FACE_SHADER: &str = r#"
struct Globals {
    view_proj : mat4x4<f32>,
    light_dir : vec4<f32>,
    render_opts : vec4<f32>, // x = textures on/off, y = brightness multiplier
};

@group(0) @binding(0)
var<uniform> globals : Globals;

@group(1) @binding(0)
var tex_sampler : sampler;

@group(1) @binding(1)
var tex_color : texture_2d<f32>;

struct VsIn {
    @location(0) position : vec3<f32>,
    @location(1) normal   : vec3<f32>,
    @location(2) uv       : vec2<f32>,
};

struct VsOut {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) normal : vec3<f32>,
    @location(1) uv     : vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out : VsOut;
    out.clip_pos = globals.view_proj * vec4<f32>(v.position, 1.0);
    out.normal = normalize(v.normal);
    out.uv = v.uv;
    return out;
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(tex_color, tex_sampler, v.uv);

    if texel.a < 0.1 {
        discard;
    }

    let ndotl = abs(dot(normalize(v.normal), normalize(globals.light_dir.xyz)));
    let shade = (0.45 + 0.55 * ndotl) * globals.render_opts.y;

    let textured = globals.render_opts.x;
    let flat_gray = vec3<f32>(0.50, 0.50, 0.50);
    let base_rgb = mix(flat_gray, texel.rgb, textured);
    let lit_rgb = min(base_rgb * shade, vec3<f32>(1.0, 1.0, 1.0));

    return vec4<f32>(lit_rgb, texel.a);
}
"#;

const LINE_SHADER: &str = r#"
struct Globals {
    view_proj : mat4x4<f32>,
    light_dir : vec4<f32>,
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
