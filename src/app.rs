use crate::{
    anm::{load_anm, AnmFile},
    audio_player::AudioPlayer,
    bik_preview::{
        extract_bik_audio_wav, load_bik_preview, spawn_bik_decoder, BikPreview, BikWorkerEvent,
    },
    bnk::{format_name, load_bnk, BnkFile},
    dds_preview::{load_dds_preview, DdsPreview},
    fs_tree::{category_name, classify_path, scan_game_data, AssetCategory, FileNode, NodeKind},
    geo::{load_geo, GeoAssetType, GeoFile},
    geo_viewer::{
        draw_geo_viewer, draw_scene_viewer, focus_scene_viewer_on_point, reset_geo_viewer,
        reset_scene_viewer, GeoViewerState, SceneGeoModel,
    },
    scn::{load_scn, ScnFile},
};
use eframe::egui;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
struct ModelTextureRef {
    name: String,
    resolved_path: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct AnmGeoCandidate {
    path: PathBuf,
    score: i32,
    bone_count: Option<usize>,
}

#[derive(Clone, Debug, Default)]
struct AssetLinks {
    model_to_textures: std::collections::BTreeMap<PathBuf, Vec<ModelTextureRef>>,
    texture_to_models: std::collections::BTreeMap<PathBuf, Vec<PathBuf>>,
}

pub struct ModToolApp {
    game_root: Option<PathBuf>,
    tree: Vec<FileNode>,
    selected_file: Option<PathBuf>,
    status: String,
    dark_mode: bool,

    dds_preview: Option<DdsPreview>,
    dds_preview_path: Option<PathBuf>,
    dds_error: Option<String>,
    texture_zoom: f32,
    dds_view_height: f32,

    bnk_file: Option<BnkFile>,
    bnk_loaded_path: Option<PathBuf>,
    bnk_error: Option<String>,
    selected_bnk_entry: Option<usize>,

    audio_player: Option<AudioPlayer>,
    audio_error: Option<String>,

    anm_file: Option<AnmFile>,
    anm_loaded_path: Option<PathBuf>,
    anm_error: Option<String>,
    anm_geo_overrides: std::collections::BTreeMap<PathBuf, PathBuf>,

    active_geo_animation: Option<PathBuf>,
    active_geo_animation_file: Option<AnmFile>,
    active_geo_animation_loaded_path: Option<PathBuf>,
    active_geo_animation_error: Option<String>,
    active_geo_animation_playing: bool,
    active_geo_animation_loop: bool,
    active_geo_animation_time: f32,
    active_geo_animation_speed: f32,

    geo_file: Option<GeoFile>,
    geo_loaded_path: Option<PathBuf>,
    geo_error: Option<String>,

    scn_file: Option<ScnFile>,
    scn_loaded_path: Option<PathBuf>,
    scn_error: Option<String>,

    scn_scene_models: Vec<SceneGeoModel>,
    scn_scene_models_path: Option<PathBuf>,
    scn_scene_unresolved: Vec<String>,
    scn_scene_error: Option<String>,
    selected_scn_node: Option<usize>,
    scn_viewer: GeoViewerState,
    scn_view_height: f32,
    scn_embedded_texture_previews: std::collections::HashMap<String, DdsPreview>,

    geo_material_previews: Vec<Option<DdsPreview>>,
    geo_materials_loaded_path: Option<PathBuf>,
    geo_material_error: Option<String>,

    asset_links: AssetLinks,

    geo_viewer: GeoViewerState,
    geo_viewer_path: Option<PathBuf>,
    geo_view_height: f32,

    bik_preview: Option<BikPreview>,
    bik_preview_path: Option<PathBuf>,
    bik_texture: Option<egui::TextureHandle>,
    bik_error: Option<String>,
    bik_audio_error: Option<String>,
    bik_audio_wav: Option<Vec<u8>>,
    bik_audio_path: Option<PathBuf>,
    bik_audio_active: bool,
    bik_zoom: f32,
    bik_view_height: f32,
    bik_current_frame: usize,
    bik_current_time_seconds: f32,
    bik_is_playing: bool,
    bik_loop: bool,
    bik_decoder_rx: Option<std::sync::mpsc::Receiver<BikWorkerEvent>>,
    bik_frame_queue: VecDeque<(usize, f32, egui::ColorImage)>,
    bik_clock_started_at: Option<Instant>,
    bik_clock_start_secs: f32,
    bik_decoder_finished: bool,
}

impl ModToolApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        Self {
            game_root: None,
            tree: Vec::new(),
            selected_file: None,
            status: "Choose your Little Britain install folder.".to_owned(),
            dark_mode: true,

            dds_preview: None,
            dds_preview_path: None,
            dds_error: None,
            texture_zoom: 1.0,
            dds_view_height: 420.0,

            bnk_file: None,
            bnk_loaded_path: None,
            bnk_error: None,
            selected_bnk_entry: None,

            audio_player: None,
            audio_error: None,

            anm_file: None,
            anm_loaded_path: None,
            anm_error: None,
            anm_geo_overrides: std::collections::BTreeMap::new(),

            active_geo_animation: None,
            active_geo_animation_file: None,
            active_geo_animation_loaded_path: None,
            active_geo_animation_error: None,
            active_geo_animation_playing: false,
            active_geo_animation_loop: true,
            active_geo_animation_time: 0.0,
            active_geo_animation_speed: 1.0,

            geo_file: None,
            geo_loaded_path: None,
            geo_error: None,

            scn_file: None,
            scn_loaded_path: None,
            scn_error: None,

            geo_material_previews: Vec::new(),
            geo_materials_loaded_path: None,
            geo_material_error: None,

            asset_links: AssetLinks::default(),

            geo_viewer: GeoViewerState::default(),
            geo_viewer_path: None,
            geo_view_height: 520.0,

            scn_scene_models: Vec::new(),
            scn_scene_models_path: None,
            scn_scene_unresolved: Vec::new(),
            scn_scene_error: None,
            selected_scn_node: None,
            scn_viewer: GeoViewerState::default(),
            scn_view_height: 520.0,
            scn_embedded_texture_previews: std::collections::HashMap::new(),

            bik_preview: None,
            bik_preview_path: None,
            bik_texture: None,
            bik_error: None,
            bik_audio_error: None,
            bik_audio_wav: None,
            bik_audio_path: None,
            bik_audio_active: false,
            bik_zoom: 1.0,
            bik_view_height: 420.0,
            bik_current_frame: 0,
            bik_current_time_seconds: 0.0,
            bik_is_playing: false,
            bik_loop: true,
            bik_decoder_rx: None,
            bik_frame_queue: VecDeque::new(),
            bik_clock_started_at: None,
            bik_clock_start_secs: 0.0,
            bik_decoder_finished: false,
        }
    }

    fn open_game_folder(&mut self) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            match scan_game_data(&folder) {
                Ok(tree) => {
                    self.game_root = Some(folder);
                    self.tree = tree;
                    self.asset_links = self.build_asset_links();
                    self.selected_file = None;
                    self.dds_preview = None;
                    self.dds_preview_path = None;
                    self.dds_error = None;
                    self.dds_view_height = 420.0;
                    self.bnk_file = None;
                    self.bnk_loaded_path = None;
                    self.bnk_error = None;
                    self.selected_bnk_entry = None;
                    self.anm_file = None;
                    self.anm_loaded_path = None;
                    self.anm_error = None;
                    self.anm_geo_overrides.clear();
                    self.active_geo_animation = None;
                    self.geo_file = None;
                    self.geo_loaded_path = None;
                    self.geo_error = None;
                    self.geo_viewer = GeoViewerState::default();
                    self.geo_view_height = 520.0;
                    self.geo_viewer_path = None;
                    self.geo_material_previews.clear();
                    self.geo_materials_loaded_path = None;
                    self.geo_material_error = None;
                    self.active_geo_animation_file = None;
                    self.active_geo_animation_loaded_path = None;
                    self.active_geo_animation_error = None;
                    self.active_geo_animation_playing = false;
                    self.active_geo_animation_time = 0.0;
                    self.scn_file = None;
                    self.scn_loaded_path = None;
                    self.scn_error = None;   
                    self.scn_scene_models.clear();
                    self.scn_scene_models_path = None;
                    self.scn_scene_unresolved.clear();
                    self.scn_scene_error = None;
                    self.selected_scn_node = None;
                    self.scn_viewer = GeoViewerState::default();
                    self.scn_view_height = 520.0;  
                    self.scn_embedded_texture_previews.clear();               
                    self.reset_bik_state();
                    self.status = "Loaded Data folder.".to_owned();
                }
                Err(err) => {
                    self.status = err.to_string();
                }
            }
        }
    }

    fn rescan(&mut self) {
        if let Some(root) = self.game_root.clone() {
            match scan_game_data(&root) {
                Ok(tree) => {
                    self.tree = tree;
                    
                    self.asset_links = self.build_asset_links();

                    self.dds_preview = None;
                    self.dds_preview_path = None;
                    self.dds_error = None;
                    self.texture_zoom = 1.0;
                    self.dds_view_height = 420.0;

                    self.bnk_file = None;
                    self.bnk_loaded_path = None;
                    self.bnk_error = None;
                    self.selected_bnk_entry = None;

                    self.anm_file = None;
                    self.anm_loaded_path = None;
                    self.anm_error = None;
                    self.anm_geo_overrides.clear();
                    self.active_geo_animation = None;

                    self.geo_file = None;

                    self.geo_viewer = GeoViewerState::default();
                    self.geo_viewer_path = None;
                    self.geo_view_height = 520.0;

                    self.geo_material_previews.clear();
                    self.geo_materials_loaded_path = None;
                    self.geo_material_error = None;

                    self.active_geo_animation_file = None;
                    self.active_geo_animation_loaded_path = None;
                    self.active_geo_animation_error = None;
                    self.active_geo_animation_playing = false;
                    self.active_geo_animation_time = 0.0;

                    self.scn_file = None;
                    self.scn_loaded_path = None;
                    self.scn_error = None;

                    self.scn_scene_models.clear();
                    self.scn_scene_models_path = None;
                    self.scn_scene_unresolved.clear();
                    self.scn_scene_error = None;
                    self.scn_viewer = GeoViewerState::default();
                    self.scn_view_height = 520.0;
                    self.scn_embedded_texture_previews.clear();                    
                    
                    self.reset_bik_state();

                    self.status = "Rescanned Data folder.".to_owned();
                }
                Err(err) => {
                    self.status = err.to_string();
                }
            }
        }
    }

    fn ui_file_row(ui: &mut egui::Ui, node: &FileNode, selected_file: &mut Option<PathBuf>) {
        let is_selected = selected_file.as_ref() == Some(&node.path);
        if ui.selectable_label(is_selected, &node.name).clicked() {
            *selected_file = Some(node.path.clone());
        }
    }

    fn ui_category_group(
        ui: &mut egui::Ui,
        title: &str,
        files: &[&FileNode],
        selected_file: &mut Option<PathBuf>,
    ) {
        if files.is_empty() {
            return;
        }

        egui::CollapsingHeader::new(title)
            .default_open(false)
            .show(ui, |ui| {
                for file in files {
                    Self::ui_file_row(ui, file, selected_file);
                }
            });
    }

    fn ui_node(ui: &mut egui::Ui, node: &FileNode, selected_file: &mut Option<PathBuf>) {
        match node.kind {
            NodeKind::File => {
                Self::ui_file_row(ui, node, selected_file);
            }
            NodeKind::Folder => {
                egui::CollapsingHeader::new(&node.name)
                    .default_open(false)
                    .show(ui, |ui| {
                        let mut real_folders: Vec<&FileNode> = Vec::new();
                        let mut animations: Vec<&FileNode> = Vec::new();
                        let mut models: Vec<&FileNode> = Vec::new();
                        let mut textures: Vec<&FileNode> = Vec::new();
                        let mut particles: Vec<&FileNode> = Vec::new();
                        let mut audio: Vec<&FileNode> = Vec::new();
                        let mut audio_banks: Vec<&FileNode> = Vec::new();
                        let mut lighting: Vec<&FileNode> = Vec::new();
                        let mut scenes: Vec<&FileNode> = Vec::new();
                        let mut logs: Vec<&FileNode> = Vec::new();
                        let mut videos: Vec<&FileNode> = Vec::new();
                        let mut other: Vec<&FileNode> = Vec::new();

                        for child in &node.children {
                            match child.kind {
                                NodeKind::Folder => real_folders.push(child),
                                NodeKind::File => match child.category.unwrap_or(AssetCategory::Unknown) {
                                    AssetCategory::Animation => animations.push(child),
                                    AssetCategory::Model => models.push(child),
                                    AssetCategory::Texture => textures.push(child),
                                    AssetCategory::Particle => particles.push(child),
                                    AssetCategory::AudioStream => audio.push(child),
                                    AssetCategory::AudioBank => audio_banks.push(child),
                                    AssetCategory::Lighting => lighting.push(child),
                                    AssetCategory::Scene => scenes.push(child),
                                    AssetCategory::Log => logs.push(child),
                                    AssetCategory::Video => videos.push(child),
                                    AssetCategory::Unknown => other.push(child),
                                },
                            }
                        }

                        for folder in real_folders {
                            Self::ui_node(ui, folder, selected_file);
                        }

                        Self::ui_category_group(ui, "Animations", &animations, selected_file);
                        Self::ui_category_group(ui, "Models", &models, selected_file);
                        Self::ui_category_group(ui, "Textures", &textures, selected_file);
                        Self::ui_category_group(ui, "Particles", &particles, selected_file);
                        Self::ui_category_group(ui, "Audio", &audio, selected_file);
                        Self::ui_category_group(ui, "Audio Banks", &audio_banks, selected_file);
                        Self::ui_category_group(ui, "Lighting", &lighting, selected_file);
                        Self::ui_category_group(ui, "Levels", &scenes, selected_file);
                        Self::ui_category_group(ui, "Logs", &logs, selected_file);
                        Self::ui_category_group(ui, "Videos", &videos, selected_file);
                        Self::ui_category_group(ui, "Other", &other, selected_file);
                    });
            }
        }
    }

    fn stop_bik_audio(&mut self) {
        if self.bik_audio_active {
            if let Some(player) = self.audio_player.as_ref() {
                player.stop();
            }
        }

        self.bik_audio_active = false;
    }

    fn ensure_bik_audio_loaded(&mut self) -> bool {
        let Some(preview) = self.bik_preview.as_ref() else {
            return false;
        };

        if !preview.has_audio {
            return false;
        }

        let path = preview.path.clone();

        if self.bik_audio_path.as_ref() == Some(&path) {
            return self.bik_audio_wav.is_some();
        }

        self.bik_audio_path = Some(path.clone());
        self.bik_audio_wav = None;
        self.bik_audio_error = None;

        match extract_bik_audio_wav(&path) {
            Ok(Some(wav_bytes)) => {
                self.bik_audio_wav = Some(wav_bytes);
                true
            }
            Ok(None) => false,
            Err(err) => {
                self.bik_audio_error = Some(err.to_string());
                false
            }
        }
    }

    fn start_bik_audio(&mut self, start_seconds: f32) {
        if !self.ensure_bik_audio_loaded() {
            self.bik_audio_active = false;
            return;
        }

        if !self.ensure_audio_player() {
            self.bik_audio_active = false;
            self.bik_audio_error = self.audio_error.clone();
            return;
        }

        let Some(wav_bytes) = self.bik_audio_wav.clone() else {
            self.bik_audio_active = false;
            return;
        };

        let label = self
            .bik_preview
            .as_ref()
            .map(|preview| {
                preview
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| preview.path.display().to_string())
            })
            .unwrap_or_else(|| "BIK audio".to_owned());

        if let Some(player) = self.audio_player.as_mut() {
            match player.play_data(label.clone(), wav_bytes) {
                Ok(()) => {
                    player.seek(Duration::from_secs_f32(start_seconds.max(0.0)));
                    self.bik_audio_error = None;
                    self.bik_audio_active = true;
                    self.status = format!("Playing {label}");
                }
                Err(err) => {
                    self.bik_audio_error = Some(err.to_string());
                    self.bik_audio_active = false;
                }
            }
        }
    }  

    fn reset_bik_state(&mut self) {
        self.stop_bik_audio();
        self.bik_preview = None;
        self.bik_preview_path = None;
        self.bik_texture = None;
        self.bik_error = None;
        self.bik_audio_error = None;
        self.bik_audio_wav = None;
        self.bik_audio_path = None;
        self.bik_zoom = 1.0;
        self.bik_view_height = 420.0;
        self.bik_current_frame = 0;
        self.bik_current_time_seconds = 0.0;
        self.bik_is_playing = false;
        self.bik_loop = true;
        self.bik_decoder_rx = None;
        self.bik_frame_queue.clear();
        self.bik_clock_started_at = None;
        self.bik_clock_start_secs = 0.0;
        self.bik_decoder_finished = false;
    }

    fn set_bik_texture_from_image(&mut self, ctx: &egui::Context, image: egui::ColorImage) {
        if let Some(texture) = self.bik_texture.as_mut() {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            let name = self
                .bik_preview
                .as_ref()
                .map(|p| format!("bik_video:{}", p.path.display()))
                .unwrap_or_else(|| "bik_video".to_owned());

            self.bik_texture =
                Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR));
        }
    }

    fn ensure_bik_preview_loaded(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_file.clone() else {
            self.reset_bik_state();
            return;
        };

        let is_bik = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("bik"))
            .unwrap_or(false);

        if !is_bik {
            self.reset_bik_state();
            return;
        }

        if self.bik_preview_path.as_ref() == Some(&path) {
            return;
        }

        self.reset_bik_state();
        self.bik_preview_path = Some(path.clone());

        match load_bik_preview(&path) {
            Ok(preview) => {
                let first_frame = preview.first_frame.clone();
                self.bik_preview = Some(preview);
                self.set_bik_texture_from_image(ctx, first_frame);
                self.bik_current_frame = 0;
                self.bik_current_time_seconds = 0.0;
                self.bik_error = None;
                self.bik_audio_error = None;
                ctx.request_repaint();
            }
            Err(err) => {
                self.bik_error = Some(err.to_string());
            }
        }
    }

    fn start_bik_playback(&mut self, ctx: &egui::Context) {
        let Some((preview_path, estimated_total, first_frame)) = self
            .bik_preview
            .as_ref()
            .map(|preview| {
                (
                    preview.path.clone(),
                    preview.estimated_frame_count(),
                    preview.first_frame.clone(),
                )
            })
        else {
            return;
        };

        let mut start_frame = self.bik_current_frame;

        if estimated_total > 0 && start_frame >= estimated_total {
            start_frame = 0;
            self.bik_current_frame = 0;
            self.bik_current_time_seconds = 0.0;
            self.set_bik_texture_from_image(ctx, first_frame);
        }

        self.bik_decoder_rx = Some(spawn_bik_decoder(preview_path, start_frame));
        self.bik_frame_queue.clear();
        self.bik_clock_started_at = Some(Instant::now());
        self.bik_clock_start_secs = self.bik_current_time_seconds;
        self.bik_decoder_finished = false;
        self.bik_is_playing = true;
        self.bik_error = None;
        self.start_bik_audio(self.bik_current_time_seconds);
        ctx.request_repaint();
    }

    fn pause_bik_playback(&mut self) {
        self.bik_is_playing = false;
        self.bik_decoder_rx = None;
        self.bik_frame_queue.clear();
        self.bik_clock_started_at = None;
        self.bik_decoder_finished = false;

        if self.bik_audio_active {
            if let Some(player) = self.audio_player.as_ref() {
                player.pause();
            }
        }
    }

    fn stop_bik_playback(&mut self, ctx: &egui::Context) {
        self.bik_is_playing = false;
        self.bik_decoder_rx = None;
        self.bik_frame_queue.clear();
        self.bik_clock_started_at = None;
        self.bik_clock_start_secs = 0.0;
        self.bik_current_frame = 0;
        self.bik_current_time_seconds = 0.0;
        self.bik_decoder_finished = false;
        self.stop_bik_audio();

        let first_frame = self
            .bik_preview
            .as_ref()
            .map(|preview| preview.first_frame.clone());

        if let Some(first_frame) = first_frame {
            self.set_bik_texture_from_image(ctx, first_frame);
        }
    }

    fn seek_bik_to_time(&mut self, seconds: f32, ctx: &egui::Context) {
        let was_playing = self.bik_is_playing;

        let Some((total, fps, first_frame)) = self
            .bik_preview
            .as_ref()
            .map(|preview| {
                (
                    preview.total_duration_seconds(),
                    preview.fps.max(0.001),
                    preview.first_frame.clone(),
                )
            })
        else {
            return;
        };

        let target = seconds.clamp(0.0, total.max(0.0));

        self.bik_is_playing = false;
        self.bik_decoder_rx = None;
        self.bik_frame_queue.clear();
        self.bik_clock_started_at = None;
        self.bik_clock_start_secs = target;
        self.bik_decoder_finished = false;

        if self.bik_audio_active {
            if let Some(player) = self.audio_player.as_ref() {
                player.pause();
                player.seek(Duration::from_secs_f32(target));
            }
        }

        self.bik_current_time_seconds = target;
        self.bik_current_frame = (target * fps).floor() as usize;

        if target <= 0.0 {
            self.set_bik_texture_from_image(ctx, first_frame);
        }

        if was_playing {
            self.start_bik_playback(ctx);
        }
    }

    fn poll_bik_decoder(&mut self, _ctx: &egui::Context) {
        let Some(rx) = self.bik_decoder_rx.as_ref() else {
            return;
        };

        let mut finished = false;
        let mut error: Option<String> = None;

        for event in rx.try_iter() {
            match event {
                BikWorkerEvent::Frame {
                    frame_index,
                    time_seconds,
                    image,
                } => {
                    self.bik_frame_queue
                        .push_back((frame_index, time_seconds, image));
                }
                BikWorkerEvent::Finished => {
                    finished = true;
                }
                BikWorkerEvent::Error(err) => {
                    error = Some(err);
                }
            }
        }

        if let Some(err) = error {
            self.bik_is_playing = false;
            self.bik_decoder_rx = None;
            self.bik_frame_queue.clear();
            self.bik_decoder_finished = false;
            self.stop_bik_audio();
            self.bik_error = Some(err);
            return;
        }

        if finished {
            self.bik_decoder_rx = None;
            self.bik_decoder_finished = true;
        }
    }

    fn update_bik_playback_clock(&mut self, ctx: &egui::Context) {
        if !self.bik_is_playing {
            return;
        }

        let Some(started_at) = self.bik_clock_started_at else {
            self.bik_clock_started_at = Some(Instant::now());
            ctx.request_repaint_after(Duration::from_secs_f32(1.0 / 60.0));
            return;
        };

        let target_time = if self.bik_audio_active {
            self.audio_player
                .as_ref()
                .filter(|player| !player.is_empty())
                .map(|player| player.position().as_secs_f32())
                .unwrap_or_else(|| {
                    self.bik_clock_start_secs
                        + Instant::now().duration_since(started_at).as_secs_f32()
                })
        } else {
            self.bik_clock_start_secs
                + Instant::now().duration_since(started_at).as_secs_f32()
        };

        while let Some((_, time_seconds, _)) = self.bik_frame_queue.front() {
            if *time_seconds <= target_time {
                let (frame_index, time_seconds, image) =
                    self.bik_frame_queue.pop_front().unwrap();
                self.bik_current_frame = frame_index;
                self.bik_current_time_seconds = time_seconds;
                self.set_bik_texture_from_image(ctx, image);
            } else {
                break;
            }
        }

        if self.bik_decoder_finished && self.bik_frame_queue.is_empty() {
            self.bik_decoder_finished = false;

            if self.bik_loop {
                self.stop_bik_playback(ctx);
                self.start_bik_playback(ctx);
            } else {
                self.bik_is_playing = false;
                self.stop_bik_audio();
            }

            return;
        }

        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / 60.0));
    }

    fn ensure_dds_preview_loaded(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_file.clone() else {
            self.dds_preview = None;
            self.dds_preview_path = None;
            self.dds_error = None;
            return;
        };

        if self.dds_preview_path.as_ref() == Some(&path) {
            return;
        }

        self.dds_preview = None;
        self.dds_error = None;
        self.dds_preview_path = Some(path.clone());
        self.texture_zoom = 1.0;

        let is_dds = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("dds"))
            .unwrap_or(false);

        if !is_dds {
            return;
        }

        match load_dds_preview(ctx, &path) {
            Ok(preview) => {
                self.dds_preview = Some(preview);
            }
            Err(err) => {
                self.dds_error = Some(err.to_string());
            }
        }
    }    

    fn ensure_anm_loaded(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.anm_file = None;
            self.anm_loaded_path = None;
            self.anm_error = None;
            return;
        };

        if self.anm_loaded_path.as_ref() == Some(&path) {
            return;
        }

        self.anm_file = None;
        self.anm_error = None;
        self.anm_loaded_path = Some(path.clone());

        let is_anm = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("anm"))
            .unwrap_or(false);

        if !is_anm {
            return;
        }

        match load_anm(&path) {
            Ok(anm) => {
                self.anm_file = Some(anm);
            }
            Err(err) => {
                self.anm_error = Some(err.to_string());
            }
        }
    }

    fn asset_name_tokens(path: &Path) -> Vec<String> {
        path.file_stem()
            .map(|stem| {
                stem.to_string_lossy()
                    .to_ascii_lowercase()
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|part| !part.is_empty())
                    .map(|part| part.to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn common_prefix_len(a: &str, b: &str) -> usize {
        a.chars()
            .zip(b.chars())
            .take_while(|(left, right)| left == right)
            .count()
    }

    fn geo_bone_count_for_path(path: &Path) -> Option<usize> {
        load_geo(path)
            .ok()
            .and_then(|geo| geo.skeleton.map(|s| s.bone_count))
    }

    fn animation_geo_candidates(
        &self,
        anm_path: &Path,
        rig_bone_count: Option<usize>,
    ) -> Vec<AnmGeoCandidate> {
        let Some(folder) = anm_path.parent() else {
            return Vec::new();
        };

        let animation_stem = anm_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let animation_tokens = Self::asset_name_tokens(anm_path);

        let mut model_nodes = Vec::new();
        Self::collect_files_by_category(&self.tree, AssetCategory::Model, &mut model_nodes);

        let mut out = Vec::new();

        for node in model_nodes {
            if node.path.parent() != Some(folder) {
                continue;
            }

            let geo_stem = node
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let geo_tokens = Self::asset_name_tokens(&node.path);
            let bone_count = Self::geo_bone_count_for_path(&node.path);

            let mut score = 0i32;

            if animation_stem == geo_stem {
                score += 100;
            }
            if animation_stem.starts_with(&geo_stem) {
                score += 60;
            }
            if geo_stem.starts_with(&animation_stem) {
                score += 40;
            }

            let shared_tokens = animation_tokens
                .iter()
                .filter(|token| geo_tokens.iter().any(|geo| geo == *token))
                .count() as i32;
            score += shared_tokens * 20;
            score += Self::common_prefix_len(&animation_stem, &geo_stem).min(24) as i32;

            if let (Some(anim_bones), Some(geo_bones)) = (rig_bone_count, bone_count) {
                if anim_bones == geo_bones {
                    score += 30;
                }
            }

            if score <= 0 && rig_bone_count.is_some() && bone_count == rig_bone_count {
                score = 30;
            }

            out.push(AnmGeoCandidate {
                path: node.path.clone(),
                score,
                bone_count,
            });
        }

        out.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
        });
        out
    }

    fn selected_animation_geo_path(&self) -> Option<PathBuf> {
        let anm_path = self.selected_file.as_ref()?;
        if !anm_path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("anm"))
            .unwrap_or(false)
        {
            return None;
        }

        if let Some(path) = self.anm_geo_overrides.get(anm_path) {
            return Some(path.clone());
        }

        let rig_bone_count = self.anm_file.as_ref().map(|anm| anm.rig_bone_count);
        self.animation_geo_candidates(anm_path, rig_bone_count)
            .into_iter()
            .next()
            .map(|candidate| candidate.path)
    }

    fn selected_geo_target_path(&self) -> Option<PathBuf> {
        match self.selected_extension().as_deref() {
            Some("geo") => self.selected_file.clone(),
            Some("anm") => self.selected_animation_geo_path(),
            _ => None,
        }
    }

    fn asset_stem_lower(path: &Path) -> String {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default()
    }

    fn animation_prefix_name(path: &Path) -> String {
        let stem = Self::asset_stem_lower(path);
        stem.split('_').next().unwrap_or(&stem).to_owned()
    }

    fn animations_for_geo_grouped(&self, geo_path: &Path) -> Vec<(String, Vec<PathBuf>)> {
        let Some(folder) = geo_path.parent() else {
            return Vec::new();
        };

        let geo_stem = Self::asset_stem_lower(geo_path);
        let mut animation_nodes = Vec::new();
        Self::collect_files_by_category(&self.tree, AssetCategory::Animation, &mut animation_nodes);

        let mut groups = std::collections::BTreeMap::<String, Vec<PathBuf>>::new();
        for node in animation_nodes {
            if node.path.parent() != Some(folder) {
                continue;
            }
            let prefix = Self::animation_prefix_name(&node.path);
            groups.entry(prefix).or_default().push(node.path.clone());
        }

        let mut out: Vec<(String, Vec<PathBuf>)> = groups.into_iter().collect();
        for (_, paths) in &mut out {
            paths.sort_by_key(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default()
            });
        }

        out.sort_by(|left, right| {
            let left_is_exact = left.0 == geo_stem;
            let right_is_exact = right.0 == geo_stem;
            right_is_exact
                .cmp(&left_is_exact)
                .then_with(|| left.0.cmp(&right.0))
        });

        out
    }

    fn ensure_active_geo_animation_loaded(&mut self) {
        let Some(path) = self.active_geo_animation.clone() else {
            self.active_geo_animation_file = None;
            self.active_geo_animation_loaded_path = None;
            self.active_geo_animation_error = None;
            self.active_geo_animation_playing = false;
            self.active_geo_animation_time = 0.0;
            return;
        };

        if self.active_geo_animation_loaded_path.as_ref() == Some(&path) {
            return;
        }

        self.active_geo_animation_file = None;
        self.active_geo_animation_loaded_path = Some(path.clone());
        self.active_geo_animation_error = None;
        self.active_geo_animation_time = 0.0;

        match load_anm(&path) {
            Ok(anm) => {
                self.active_geo_animation_playing = anm.rigid_clip.is_some();
                self.active_geo_animation_file = Some(anm);
            }
            Err(err) => {
                self.active_geo_animation_playing = false;
                self.active_geo_animation_error = Some(err.to_string());
            }
        }
    }

    fn update_active_geo_animation_clock(&mut self, ctx: &egui::Context) {
        if !self.active_geo_animation_playing {
            return;
        }

        let Some(anm) = self.active_geo_animation_file.as_ref() else {
            return;
        };

        let Some(clip) = anm.rigid_clip.as_ref() else {
            return;
        };

        let dt = ctx.input(|i| i.unstable_dt).max(1.0 / 240.0);
        self.active_geo_animation_time += dt * self.active_geo_animation_speed.max(0.05);

        let duration = clip
            .duration_seconds
            .max(1.0 / clip.sample_rate.max(1.0));

        if self.active_geo_animation_time > duration {
            if self.active_geo_animation_loop {
                self.active_geo_animation_time %= duration;
            } else {
                self.active_geo_animation_time = duration;
                self.active_geo_animation_playing = false;
            }
        }

        ctx.request_repaint();
    }

    fn ensure_geo_loaded(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.geo_file = None;
            self.geo_loaded_path = None;
            self.geo_error = None;
            self.geo_viewer_path = None;
            self.active_geo_animation = None;
            return;
        };

        let is_geo = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("geo"))
            .unwrap_or(false);

        if !is_geo {
            return;
        }

        if self.geo_loaded_path.as_ref() == Some(&path) {
            return;
        }

        self.geo_file = None;
        self.geo_error = None;
        self.geo_loaded_path = Some(path.clone());
        self.active_geo_animation = None;
        self.active_geo_animation_file = None;
        self.active_geo_animation_loaded_path = None;
        self.active_geo_animation_error = None;
        self.active_geo_animation_playing = false;
        self.active_geo_animation_time = 0.0;

        match load_geo(&path) {
            Ok(geo) => {
                reset_geo_viewer(&mut self.geo_viewer, &geo);
                self.geo_viewer_path = Some(path.clone());
                self.geo_file = Some(geo);
            }
            Err(err) => {
                self.geo_error = Some(err.to_string());
                self.geo_viewer_path = None;
            }
        }
    }

    fn ensure_scn_loaded(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.scn_file = None;
            self.scn_loaded_path = None;
            self.scn_error = None;
            self.selected_scn_node = None;
            return;
        };

        if self.scn_loaded_path.as_ref() == Some(&path) {
            return;
        }

        self.scn_file = None;
        self.scn_error = None;
        self.scn_loaded_path = Some(path.clone());

        let is_scn = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("scn"))
            .unwrap_or(false);

        if !is_scn {
            self.selected_scn_node = None;
            return;
        }
        
        self.selected_scn_node = None;
        
        match load_scn(&path) {
            Ok(scn) => {
                self.scn_file = Some(scn);
            }
            Err(err) => {
                self.scn_error = Some(err.to_string());
            }
        }
    }

    fn reset_scn_scene_state(&mut self) {
        self.scn_scene_models.clear();
        self.scn_scene_models_path = None;
        self.scn_scene_unresolved.clear();
        self.scn_scene_error = None;
        self.scn_viewer = GeoViewerState::default();
        self.scn_view_height = 520.0;
        self.scn_embedded_texture_previews.clear();
    }

    fn ensure_scn_scene_loaded(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_file.clone() else {
            self.reset_scn_scene_state();
            return;
        };

        let is_scn = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("scn"))
            .unwrap_or(false);

        if !is_scn {
            self.reset_scn_scene_state();
            return;
        }

        if self.scn_scene_models_path.as_ref() == Some(&path) {
            return;
        }

        self.reset_scn_scene_state();
        self.scn_scene_models_path = Some(path);

        let Some(scn) = self.scn_file.clone() else {
            return;
        };

        let mut texture_nodes = Vec::new();
        let mut model_nodes = Vec::new();

        Self::collect_files_by_category(&self.tree, AssetCategory::Texture, &mut texture_nodes);
        Self::collect_files_by_category(&self.tree, AssetCategory::Model, &mut model_nodes);

        let mut textures_by_name = std::collections::HashMap::<String, PathBuf>::new();
        for node in texture_nodes {
            if let Some(name) = node.path.file_name().and_then(|s| s.to_str()) {
                textures_by_name
                    .entry(name.to_ascii_lowercase())
                    .or_insert_with(|| node.path.clone());
            }
        }

        let mut geo_by_stem = std::collections::HashMap::<String, PathBuf>::new();
        for node in model_nodes {
            if let Some(stem) = node.path.file_stem().and_then(|s| s.to_str()) {
                geo_by_stem
                    .entry(stem.to_ascii_lowercase())
                    .or_insert_with(|| node.path.clone());
            }
        }

        let mut archetypes = std::collections::BTreeSet::<String>::new();
        for node in &scn.nodes {
            let archetype = node.archetype.trim();
            if !archetype.is_empty() {
                archetypes.insert(archetype.to_ascii_lowercase());
            }
        }

        let mut loaded = Vec::new();
        let mut unresolved = Vec::new();
        let mut failed = Vec::new();

        for archetype in archetypes {
            match geo_by_stem.get(&archetype) {
                Some(model_path) => match load_geo(model_path) {
                    Ok(geo) => {
                        let mut textures = Vec::new();

                        for texture_name in &geo.texture_names {
                            let resolved = Self::guess_geo_texture_path(model_path, texture_name)
                                .or_else(|| {
                                    textures_by_name
                                        .get(&texture_name.to_ascii_lowercase())
                                        .cloned()
                                });

                            let preview = match resolved {
                                Some(tex_path) => match load_dds_preview(ctx, &tex_path) {
                                    Ok(preview) => Some(preview),
                                    Err(err) => {
                                        failed.push(format!(
                                            "{archetype} texture {}: {}",
                                            tex_path.display(),
                                            err
                                        ));
                                        None
                                    }
                                },
                                None => None,
                            };

                            textures.push(preview);
                        }

                        loaded.push(SceneGeoModel {
                            archetype: archetype.clone(),
                            path: model_path.clone(),
                            geo,
                            textures,
                        });
                    }
                    Err(err) => {
                        failed.push(format!("{archetype}: {err}"));
                    }
                },
                None => unresolved.push(archetype),
            }
        }

        let mut embedded_texture_previews = std::collections::HashMap::new();

        for chunk in &scn.mesh_chunks {
            for name in &chunk.texture_names {
                let key = name.to_ascii_lowercase();
                if embedded_texture_previews.contains_key(&key) {
                    continue;
                }

                let Some(tex_path) = textures_by_name.get(&key).cloned() else {
                    continue;
                };

                match load_dds_preview(ctx, &tex_path) {
                    Ok(preview) => {
                        embedded_texture_previews.insert(key, preview);
                    }
                    Err(err) => {
                        failed.push(format!(
                            "SCN texture {}: {}",
                            tex_path.display(),
                            err
                        ));
                    }
                }
            }
        }

        self.scn_embedded_texture_previews = embedded_texture_previews;

        loaded.sort_by_key(|m| m.archetype.clone());

        self.scn_scene_models = loaded;
        self.scn_scene_unresolved = unresolved;

        if !failed.is_empty() {
            self.scn_scene_error = Some(format!(
                "Failed to load {} GEO files: {}",
                failed.len(),
                failed.join(" | ")
            ));
        }

        reset_scene_viewer(&mut self.scn_viewer, &scn);
    }    

    fn find_file_case_insensitive(folder: &Path, filename: &str) -> Option<PathBuf> {
        let direct = folder.join(filename);
        if direct.exists() {
            return Some(direct);
        }

        let target = filename.to_ascii_lowercase();
        let entries = std::fs::read_dir(folder).ok()?;

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name()?.to_string_lossy().to_string();
            if name.to_ascii_lowercase() == target {
                return Some(path);
            }
        }

        None
    }

    fn guess_geo_texture_path(geo_path: &Path, texture_name: &str) -> Option<PathBuf> {
        let folder = geo_path.parent()?;
        Self::find_file_case_insensitive(folder, texture_name)
    }

    fn ensure_geo_materials_loaded(&mut self, ctx: &egui::Context) {
        let Some(geo_path) = self.geo_loaded_path.clone() else {
            self.geo_material_previews.clear();
            self.geo_materials_loaded_path = None;
            self.geo_material_error = None;
            return;
        };

        if self.geo_materials_loaded_path.as_ref() == Some(&geo_path) {
            return;
        }

        self.geo_material_previews.clear();
        self.geo_material_error = None;
        self.geo_materials_loaded_path = Some(geo_path.clone());

        let Some(geo) = self.geo_file.as_ref() else {
            return;
        };

        for texture_name in &geo.texture_names {
            let preview = if let Some(tex_path) = Self::guess_geo_texture_path(&geo_path, texture_name) {
                match load_dds_preview(ctx, &tex_path) {
                    Ok(preview) => Some(preview),
                    Err(err) => {
                        self.geo_material_error = Some(format!(
                            "Could not load GEO texture {}: {}",
                            tex_path.display(),
                            err
                        ));
                        None
                    }
                }
            } else {
                None
            };

            self.geo_material_previews.push(preview);
        }
    }

    fn ensure_bnk_loaded(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.bnk_file = None;
            self.bnk_loaded_path = None;
            self.bnk_error = None;
            self.selected_bnk_entry = None;
            return;
        };

        if self.bnk_loaded_path.as_ref() == Some(&path) {
            return;
        }

        self.bnk_file = None;
        self.bnk_error = None;
        self.selected_bnk_entry = None;
        self.bnk_loaded_path = Some(path.clone());

        let is_bnk = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("bnk"))
            .unwrap_or(false);

        if !is_bnk {
            return;
        }

        match load_bnk(&path) {
            Ok(bnk) => {
                if !bnk.entries.is_empty() {
                    self.selected_bnk_entry = Some(0);
                }
                self.bnk_file = Some(bnk);
            }
            Err(err) => {
                self.bnk_error = Some(err.to_string());
            }
        }
    }    

    fn selected_category(&self) -> Option<AssetCategory> {
        self.selected_file.as_ref().map(|path| classify_path(path))
    }

    fn selected_extension(&self) -> Option<String> {
        self.selected_file
            .as_ref()
            .and_then(|p| p.extension())
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
    }

    fn ensure_audio_player(&mut self) -> bool {
        if self.audio_player.is_some() {
            return true;
        }

        match AudioPlayer::new() {
            Ok(player) => {
                self.audio_player = Some(player);
                self.audio_error = None;
                true
            }
            Err(err) => {
                self.audio_error = Some(err.to_string());
                false
            }
        }
    }

    fn play_selected_audio(&mut self) {
        match self.selected_extension().as_deref() {
            Some("wav") | Some("ogg") => self.play_selected_audio_file(),
            Some("bnk") => self.play_selected_bnk_entry(),
            _ => {}
        }
    }

    fn play_selected_audio_file(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            return;
        };

        if !self.ensure_audio_player() {
            return;
        }

        if let Some(player) = self.audio_player.as_mut() {
            match player.play_file(&path) {
                Ok(()) => {
                    self.audio_error = None;
                    self.bik_audio_active = false;
                    self.status = format!("Playing {}", path.display());
                }
                Err(err) => {
                    self.audio_error = Some(err.to_string());
                }
            }
        }
    }

    fn play_selected_bnk_entry(&mut self) {
        let Some(index) = self.selected_bnk_entry else {
            self.audio_error = Some("Select a BNK entry first.".to_owned());
            return;
        };

        let (label, wav_bytes) = {
            let Some(bnk) = self.bnk_file.as_ref() else {
                self.audio_error = Some("No BNK is loaded.".to_owned());
                return;
            };

            let file_name = bnk
                .path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("bank.bnk");

            let label = format!("{file_name} [entry {index:03}]");

            match bnk.entry_wav_bytes(index) {
                Ok(bytes) => (label, bytes),
                Err(err) => {
                    self.audio_error = Some(err.to_string());
                    return;
                }
            }
        };

        if !self.ensure_audio_player() {
            return;
        }

        if let Some(player) = self.audio_player.as_mut() {
            match player.play_data(label.clone(), wav_bytes) {
                Ok(()) => {
                    self.audio_error = None;
                    self.bik_audio_active = false;
                    self.status = format!("Playing {label}");
                }
                Err(err) => {
                    self.audio_error = Some(err.to_string());
                }
            }
        }
    }

    fn pause_or_resume_audio(&mut self) {
        if let Some(player) = self.audio_player.as_ref() {
            if player.is_paused() {
                player.resume();
            } else {
                player.pause();
            }
        }
    }

    fn stop_audio(&mut self) {
        if let Some(player) = self.audio_player.as_ref() {
            player.stop();
            self.bik_audio_active = false;
            self.status = "Stopped audio.".to_owned();
        }
    }

    fn seek_audio(&mut self, seconds: f32) {
        if let Some(player) = self.audio_player.as_ref() {
            player.seek(Duration::from_secs_f32(seconds.max(0.0)));
        }
    }

    fn format_time(seconds: f32) -> String {
        let total = seconds.max(0.0).floor() as u64;
        let minutes = total / 60;
        let secs = total % 60;
        format!("{minutes:02}:{secs:02}")
    }

    fn draw_bottom_audio_player(&mut self, ui: &mut egui::Ui) {
        let (is_paused, is_empty, volume, position_secs, duration_secs, now_playing) =
            if let Some(player) = self.audio_player.as_ref() {
                (
                    player.is_paused(),
                    player.is_empty(),
                    player.volume(),
                    player.position().as_secs_f32(),
                    player.duration().map(|d| d.as_secs_f32()),
                    player.current_path().map(|s| s.to_owned()),
                )
            } else {
                (false, true, 1.0, 0.0, None, None)
            };

        if let Some(path) = now_playing {
            ui.small(format!("Now playing: {path}"));
        }

        if is_empty {
            ui.small("State: idle");
        } else if is_paused {
            ui.small("State: paused");
        } else {
            ui.small("State: playing");
        }

        ui.horizontal(|ui| {
            if ui.button("Play").clicked() {
                self.play_selected_audio();
            }

            let pause_label = if is_paused { "Resume" } else { "Pause" };
            if ui.button(pause_label).clicked() {
                self.pause_or_resume_audio();
            }

            if ui.button("Stop").clicked() {
                self.stop_audio();
            }

            ui.separator();

            let mut new_volume = volume;
            if ui
                .add(egui::Slider::new(&mut new_volume, 0.0..=2.0).text("Volume"))
                .changed()
            {
                if let Some(player) = self.audio_player.as_ref() {
                    player.set_volume(new_volume);
                }
            }
        });

        let max_secs = duration_secs.unwrap_or(position_secs.max(1.0));
        let mut timeline_secs = position_secs.min(max_secs);

        let response = ui.add_sized(
            [ui.available_width(), 18.0],
            egui::Slider::new(&mut timeline_secs, 0.0..=max_secs)
                .show_value(false),
        );

        if response.changed() {
            self.seek_audio(timeline_secs);
        }

        ui.horizontal(|ui| {
            ui.label(Self::format_time(timeline_secs));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    duration_secs
                        .map(Self::format_time)
                        .unwrap_or_else(|| "--:--".to_owned()),
                );
            });
        });

        if let Some(err) = &self.audio_error {
            ui.colored_label(egui::Color32::RED, format!("Audio error: {}", err));
        }
    }

    fn jump_to_file(&mut self, path: PathBuf) {
        self.selected_file = Some(path);
    }

    fn collect_files_by_category<'a>(
        nodes: &'a [FileNode],
        category: AssetCategory,
        out: &mut Vec<&'a FileNode>,
    ) {
        for node in nodes {
            match node.kind {
                NodeKind::Folder => Self::collect_files_by_category(&node.children, category, out),
                NodeKind::File => {
                    if node.category == Some(category) {
                        out.push(node);
                    }
                }
            }
        }
    }

    fn scn_node_group_name(name: &str) -> String {
        let trimmed = name.trim();

        if trimmed.is_empty() {
            return "(unnamed)".to_owned();
        }

        let without_digits = trimmed.trim_end_matches(|c: char| c.is_ascii_digit());
        let without_separators =
            without_digits.trim_end_matches(|c: char| c == '_' || c == '-' || c == ' ');

        if without_separators.is_empty() {
            trimmed.to_owned()
        } else {
            without_separators.to_owned()
        }
    }    

    fn build_asset_links(&self) -> AssetLinks {
        let mut links = AssetLinks::default();

        let mut texture_nodes = Vec::new();
        let mut model_nodes = Vec::new();

        Self::collect_files_by_category(&self.tree, AssetCategory::Texture, &mut texture_nodes);
        Self::collect_files_by_category(&self.tree, AssetCategory::Model, &mut model_nodes);

        let mut textures_by_name: std::collections::HashMap<String, PathBuf> =
            std::collections::HashMap::new();

        for tex in texture_nodes {
            textures_by_name
                .entry(tex.name.to_ascii_lowercase())
                .or_insert_with(|| tex.path.clone());
        }

        for model in model_nodes {
            let Ok(geo) = load_geo(&model.path) else {
                continue;
            };

            let mut model_refs: Vec<ModelTextureRef> = Vec::new();
            let mut seen_model_entries = std::collections::HashSet::new();

            for tex_name in &geo.texture_names {
                let resolved = Self::guess_geo_texture_path(&model.path, tex_name).or_else(|| {
                    textures_by_name
                        .get(&tex_name.to_ascii_lowercase())
                        .cloned()
                });

                let dedupe_key = (
                    tex_name.to_ascii_lowercase(),
                    resolved
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default(),
                );

                if !seen_model_entries.insert(dedupe_key) {
                    continue;
                }

                model_refs.push(ModelTextureRef {
                    name: tex_name.clone(),
                    resolved_path: resolved.clone(),
                });

                if let Some(tex_path) = resolved {
                    links
                        .texture_to_models
                        .entry(tex_path)
                        .or_default()
                        .push(model.path.clone());
                }
            }

            model_refs.sort_by_key(|r| r.name.to_ascii_lowercase());
            links
                .model_to_textures
                .insert(model.path.clone(), model_refs);
        }

        for models in links.texture_to_models.values_mut() {
            models.sort_by_key(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default()
            });
            models.dedup();
        }

        links
    }    
}

impl eframe::App for ModToolApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.dark_mode {
            ui.ctx().set_visuals(egui::Visuals::dark());
        } else {
            ui.ctx().set_visuals(egui::Visuals::light());
        }

        self.ensure_bnk_loaded();
        self.ensure_dds_preview_loaded(ui.ctx());
        self.ensure_bik_preview_loaded(ui.ctx());
        self.poll_bik_decoder(ui.ctx());
        self.update_bik_playback_clock(ui.ctx());
        self.ensure_anm_loaded();
        self.ensure_active_geo_animation_loaded();
        self.update_active_geo_animation_clock(ui.ctx());
        self.ensure_geo_loaded();
        self.ensure_scn_loaded();
        self.ensure_scn_scene_loaded(ui.ctx());
        self.ensure_geo_materials_loaded(ui.ctx());

        let mut pending_jump: Option<PathBuf> = None;
        if let Some(player) = self.audio_player.as_ref() {
            if !player.is_empty() {
                ui.ctx().request_repaint();
            }
        }

        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open game folder").clicked() {
                    self.open_game_folder();
                }

                if ui
                    .add_enabled(self.game_root.is_some(), egui::Button::new("Rescan"))
                    .clicked()
                {
                    self.rescan();
                }

                let theme_label = if self.dark_mode { "Light mode" } else { "Dark mode" };

                if ui.button(theme_label).clicked() {
                    self.dark_mode = !self.dark_mode;
                }

                ui.separator();
                ui.label(&self.status);
            });
        });

        egui::Panel::left("file_tree")
            .resizable(true)
            .default_size(320.0)
            .show_inside(ui, |ui| {
                ui.heading("Data");
                ui.separator();

                if self.tree.is_empty() {
                    ui.label("No game folder loaded.");
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for node in &self.tree {
                            Self::ui_node(ui, node, &mut self.selected_file);
                        }
                    });
                }
            });

        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(340.0)
            .show_inside(ui, |ui| {
                ui.heading("Inspector");
                ui.separator();

                if let Some(path) = &self.selected_file {
                    let ext = self.selected_extension().unwrap_or_else(|| "(none)".into());
                    let size = std::fs::metadata(path).ok().map(|m| m.len()).unwrap_or(0);

                    ui.label(format!("Path: {}", path.display()));
                    ui.label(format!("Extension: {}", ext));

                    if let Some(category) = self.selected_category() {
                        ui.label(format!("Category: {}", category_name(category)));
                    }

                    ui.label(format!("Size: {} bytes", size));

                    if ext == "dds" {
                        if let Some(preview) = &self.dds_preview {
                            ui.label(format!("Width: {}", preview.width));
                            ui.label(format!("Height: {}", preview.height));
                            ui.label(format!("Mipmaps: {}", preview.mipmaps));
                        }

                        if let Some(err) = &self.dds_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("DDS preview error: {}", err),
                            );
                        }
                    }

                    if ext == "bnk" {
                        if let Some(bnk) = &self.bnk_file {
                            ui.label(format!("Entries: {}", bnk.entry_count));
                            ui.label(format!("Bank size: {} bytes", bnk.file_size));

                            if let Some(selected_index) = self.selected_bnk_entry {
                                if let Some(entry) = bnk.entries.get(selected_index) {
                                    ui.separator();
                                    ui.label(format!("Selected entry: #{}", entry.index));
                                    ui.label(format!("Offset: 0x{:08X}", entry.data_offset));
                                    ui.label(format!("End: 0x{:08X}", entry.data_end()));
                                    ui.label(format!(
                                        "Format: 0x{:08X} ({})",
                                        entry.format_word,
                                        format_name(entry.format_word)
                                    ));
                                    ui.label(format!("Sample rate: {} Hz", entry.sample_rate));
                                    ui.label(format!("Clip bytes: {}", entry.byte_len));
                                    ui.label(format!("Reserved: 0x{:08X}", entry.reserved));

                                    if let Some(seconds) = entry.estimated_duration_seconds() {
                                        ui.label(format!("Estimated duration: {:.2} sec", seconds));
                                    } else {
                                        ui.label("Estimated duration: unknown");
                                    }
                                }
                            }
                        }

                        if let Some(err) = &self.bnk_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("BNK read error: {}", err),
                            );
                        }
                    }
                    if ext == "bik" {
                        if let Some(preview) = &self.bik_preview {
                            ui.label(format!("Width: {}", preview.width));
                            ui.label(format!("Height: {}", preview.height));
                            ui.label(format!("FPS: {:.3}", preview.fps));
                            ui.label(format!(
                                "Estimated frames: {}",
                                preview.estimated_frame_count()
                            ));
                            ui.label(format!(
                                "Current frame: {}",
                                self.bik_current_frame.saturating_add(1)
                            ));
                            ui.label(format!(
                                "Current time: {:.2} sec",
                                self.bik_current_time_seconds
                            ));

                            if let Some(duration) = preview.duration_seconds {
                                ui.label(format!("Duration: {:.2} sec", duration));
                            } else {
                                ui.label("Duration: unknown");
                            }
                        }

                        if let Some(err) = &self.bik_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("BIK playback error: {}", err),
                            );
                        }
                    }

                    if ext == "anm" {
                        if let Some(anm) = self.anm_file.clone() {
                            ui.label(format!("Version: {}.{}", anm.version_major, anm.version_minor));
                            ui.label(format!("Payload size: {} bytes", anm.payload_size));
                            ui.label(format!("Rig bones: {}", anm.rig_bone_count));
                            ui.label(format!("Duration hint: {:.3} sec", anm.duration_hint_seconds));
                            ui.label(format!("Section count hint: {}", anm.section_count_hint));
                            ui.label(format!("Section table: 0x{:08X}", anm.section_table_offset));
                            ui.label(format!("Timing table: 0x{:08X}", anm.timing_table_offset));
                            ui.label(format!("Key blocks hint: {}", anm.key_block_count_hint));
                            ui.label(format!("Key count hint: {}", anm.key_count_hint));

                            ui.separator();
                            ui.small("ANM files do not auto-load a GEO anymore.");
                            ui.small("Open a rigged or skinned GEO and use its Animations panel at the bottom.");

                            if !anm.timing_samples.is_empty() {
                                ui.separator();
                                ui.heading("Timing samples");
                                ui.separator();
                                for value in anm.timing_samples.iter().take(16) {
                                    ui.label(format!("{:.4} sec", value));
                                }
                            }

                            if !anm.timing_offsets.is_empty() {
                                ui.separator();
                                ui.heading("Timing offsets");
                                ui.separator();
                                for value in anm.timing_offsets.iter().take(16) {
                                    ui.label(format!("0x{:08X}", value));
                                }
                            }

                            if anm.embedded_strings.is_empty() {
                                ui.separator();
                                ui.small(
                                    "No readable embedded GEO or asset-path strings were found in this ANM.",
                                );
                            } else {
                                ui.separator();
                                ui.heading("Embedded strings");
                                ui.separator();
                                for value in &anm.embedded_strings {
                                    ui.monospace(value);
                                }
                            }
                        }

                        if let Some(err) = &self.anm_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("ANM read error: {}", err),
                            );
                        }
                    }

                    if ext == "scn" {
                        if let Some(scn) = &self.scn_file {
                            ui.label(format!("Nodes: {}", scn.nodes.len()));
                            ui.label(format!("Renderable nodes: {}", scn.renderable_count()));
                            ui.label(format!("Marker nodes: {}", scn.marker_count()));
                            ui.label(format!(
                                "Embedded chunks: {}",
                                scn.embedded_mesh_chunk_count()
                            ));
                            ui.label(format!(
                                "Embedded tris: {}",
                                scn.embedded_triangle_count()
                            ));

                            ui.label(format!(
                                "Embedded textures: {}",
                                scn.embedded_texture_name_count()
                            ));

                            ui.label(format!(
                                "Texture spans: {}",
                                scn.texture_span_count()
                            ));
                            
                            ui.label(format!(
                                "Secondary transforms: {}",
                                scn.secondary_transforms.len()
                            ));
                            ui.label(format!("Remap pairs: {}", scn.remap_pairs.len()));
                            ui.label(format!("Resolved GEOs: {}", self.scn_scene_models.len()));
                            ui.label(format!(
                                "Missing archetypes: {}",
                                self.scn_scene_unresolved.len()
                            ));

                            ui.separator();
                            ui.heading("Header");
                            ui.separator();

                            ui.label(format!("version: {}", scn.header.version));
                            ui.label(format!("unk_04: {}", scn.header.unk_04));
                            ui.label(format!("remap_count: {}", scn.header.remap_count));
                            ui.label(format!(
                                "record_table_off: 0x{:08X}",
                                scn.header.record_table_offset
                            ));
                            ui.label(format!("node_count: {}", scn.header.node_count));
                            ui.label(format!(
                                "names_off: 0x{:08X}",
                                scn.header.names_offset
                            ));
                            ui.label(format!(
                                "xforms_off: 0x{:08X}",
                                scn.header.transforms_offset
                            ));
                            ui.label(format!(
                                "archetypes_off: 0x{:08X}",
                                scn.header.archetypes_offset
                            ));
                            ui.label(format!(
                                "flags_off: 0x{:08X}",
                                scn.header.flags_offset
                            ));
                            ui.label(format!(
                                "secondary_xforms_off: 0x{:08X}",
                                scn.header.secondary_transform_offset
                            ));

                            ui.separator();
                            ui.heading("Span modes");
                            ui.separator();

                            for (mode, count) in scn.texture_span_mode_counts() {
                                ui.label(format!("mode {}: {}", mode, count));
                            }

                            ui.separator();
                            ui.heading("Top archetypes");
                            ui.separator();

                            for (name, count) in scn.top_archetypes(24) {
                                ui.label(format!("{name}: {count}"));
                            }
                        }

                        if let Some(err) = &self.scn_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("SCN read error: {}", err),
                            );
                        }
                    }                  

                    if ext == "geo" {
                        if let Some(geo) = &self.geo_file {
                            ui.label(format!("Vertices: {}", geo.vertex_count));
                            ui.label(format!("Indices: {}", geo.index_count));
                            ui.label(format!("Faces: {}", geo.faces.len()));
                            ui.label(format!("Textures: {}", geo.texture_names.len()));
                            ui.label(format!("Subsets: {}", geo.subsets.len()));
                            ui.label(format!("Asset type: {}", geo.asset_type.as_str()));

                            if let Some(skeleton) = &geo.skeleton {
                                ui.separator();
                                ui.label(format!("Bones: {}", skeleton.bone_count));
                                ui.label(format!("Skeleton ptr: 0x{:08X}", skeleton.skeleton_ptr));
                                ui.label(format!("aux_a_off: 0x{:08X}", skeleton.aux_a_off));
                                ui.label(format!("aux_b_off: 0x{:08X}", skeleton.aux_b_off));
                                ui.label(format!("name_table_off: 0x{:08X}", skeleton.name_table_off));
                            }

                        if geo.weight_profile.has_weights {
                            ui.separator();
                            ui.label(format!(
                                "Weighted vertices: {}",
                                geo.weight_profile.weighted_vertex_count
                            ));
                            ui.label(format!(
                                "Single influence: {}",
                                geo.weight_profile.single_influence_vertices
                            ));
                            ui.label(format!(
                                "Multi influence: {}",
                                geo.weight_profile.multi_influence_vertices
                            ));
                            ui.label(format!(
                                "Max influences/vertex: {}",
                                geo.weight_profile.max_influences_per_vertex
                            ));
                            ui.label(format!(
                                "Single-bone faces: {}",
                                geo.weight_profile.single_bone_faces
                            ));
                            ui.label(format!(
                                "Mixed-bone faces: {}",
                                geo.weight_profile.mixed_bone_faces
                            ));
                            ui.label(format!(
                                "Rigid single-influence: {}",
                                geo.weight_profile.rigid_single_influence
                            ));
                            ui.label(format!(
                                "Rigid face partition: {}",
                                geo.weight_profile.rigid_face_partition
                            ));
                        }
                    }

                    if let Some(err) = &self.geo_error {
                        ui.colored_label(
                            egui::Color32::RED,
                            format!("GEO read error: {}", err),
                        );
                    }
                }                    

                    ui.separator();
                    ui.label("Viewer:");

                    match ext.as_str() {
                        "dds" => {
                            ui.label("Texture viewer loaded.");
                        }
                        "bnk" => {
                            ui.label("BNK reader loaded.");
                        }
                        "wav" | "ogg" => {
                            ui.label("Audio controls are shown in the preview panel.");
                        }
                        "geo" => {
                            ui.label("GEO reader loaded.");
                        }
                        "bik" => {
                            ui.label("BIK player loaded.");
                        }
                        "scn" => {
                            ui.label("SCN inspector loaded.");
                        }
                        _ => {
                            ui.label("No viewer yet. Raw/hex viewer later.");
                        }
                    }
                } else {
                    ui.label("Select a file.");
                }
            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.heading("Preview");
            ui.separator();

                if let Some(path) = &self.selected_file {
                ui.label(format!("Selected: {}", path.display()));

                match self.selected_extension().as_deref() {
                    Some("dds") => {
                        if let Some(preview) = &self.dds_preview {
                            ui.horizontal(|ui| {
                                ui.label("Zoom");
                                ui.add(
                                    egui::Slider::new(&mut self.texture_zoom, 0.25..=8.0)
                                        .logarithmic(true)
                                        .text("x"),
                                );

                                if ui.button("Reset").clicked() {
                                    self.texture_zoom = 1.0;
                                }
                            });

                            ui.separator();

                            let preview_height = self.dds_view_height.clamp(180.0, 900.0);

                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), preview_height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    let preview_hovered = ui.rect_contains_pointer(ui.max_rect());

                                    if preview_hovered {
                                        let scroll_y = ui.ctx().input(|i| i.smooth_scroll_delta.y);

                                        if scroll_y.abs() > 0.0 {
                                            let zoom_factor = (1.0 + scroll_y * 0.001).clamp(0.5, 1.5);
                                            self.texture_zoom = (self.texture_zoom * zoom_factor).clamp(0.25, 8.0);
                                            ui.ctx().request_repaint();
                                        }
                                    }

                                    let tex_size = preview.texture.size_vec2();
                                    let available = ui.available_size();

                                    let fit_scale =
                                        (available.x / tex_size.x).min(available.y / tex_size.y).min(1.0);

                                    let desired_size = tex_size * fit_scale.max(0.1) * self.texture_zoom;

                                    egui::ScrollArea::both()
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            ui.image((preview.texture.id(), desired_size));
                                        });
                                },
                            );

                            let (resize_rect, resize_response) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 12.0),
                                egui::Sense::drag(),
                            );

                            let resize_response =
                                resize_response.on_hover_cursor(egui::CursorIcon::ResizeVertical);

                            ui.painter().hline(
                                resize_rect.x_range(),
                                resize_rect.center().y,
                                egui::Stroke::new(1.5, egui::Color32::GRAY),
                            );

                            if resize_response.dragged() {
                                let delta = ui.ctx().input(|i| i.pointer.delta()).y;
                                self.dds_view_height = (self.dds_view_height + delta).clamp(180.0, 900.0);
                                ui.ctx().request_repaint();
                            }
                        } else if let Some(err) = &self.dds_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("Could not decode DDS: {}", err),
                            );
                        } else {
                            ui.label("DDS selected, but no preview is loaded.");
                        }

                        ui.separator();
                        ui.heading("Texture");
                        ui.separator();

                        if let Some(dds_path) = &self.dds_preview_path {
                            let label = dds_path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| dds_path.display().to_string());

                            ui.label(label);
                        } else {
                            ui.label("(none)");
                        }

                        ui.separator();
                        ui.heading("Models");
                        ui.separator();

                        if let Some(dds_path) = &self.dds_preview_path {
                            if let Some(models) = self.asset_links.texture_to_models.get(dds_path) {
                                if models.is_empty() {
                                    ui.label("(not used by any scanned GEO)");
                                } else {
                                    egui::ScrollArea::vertical()
                                        .max_height(140.0)
                                        .show(ui, |ui| {
                                            for model_path in models {
                                                let label = model_path
                                                    .file_name()
                                                    .map(|n| n.to_string_lossy().to_string())
                                                    .unwrap_or_else(|| model_path.display().to_string());

                                                if ui.button(label).clicked() {
                                                    pending_jump = Some(model_path.clone());
                                                }
                                            }
                                        });
                                }
                            } else {
                                ui.label("(not used by any scanned GEO)");
                            }
                        }
                    }

                    Some("bik") => {
                        if let Some(preview) = &self.bik_preview {
                            let total_duration = preview.total_duration_seconds();
                            let preview_fps = preview.fps;
                            let estimated_frames = preview.estimated_frame_count();
                            let has_audio = preview.has_audio;
                            let file_label = preview             
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| preview.path.display().to_string());

                            ui.horizontal(|ui| {
                                ui.label("Zoom");
                                ui.add(
                                    egui::Slider::new(&mut self.bik_zoom, 0.25..=8.0)
                                        .logarithmic(true)
                                        .text("x"),
                                );

                                if ui.button("Reset").clicked() {
                                    self.bik_zoom = 1.0;
                                }
                            });

                            ui.horizontal(|ui| {
                                let play_label = if self.bik_is_playing { "Pause" } else { "Play" };

                                if ui.button(play_label).clicked() {
                                    if self.bik_is_playing {
                                        self.pause_bik_playback();
                                    } else {
                                        self.start_bik_playback(ui.ctx());
                                    }
                                }

                                if ui.button("Stop").clicked() {
                                    self.stop_bik_playback(ui.ctx());
                                }

                                if ui.button("Restart").clicked() {
                                    self.stop_bik_playback(ui.ctx());
                                    self.start_bik_playback(ui.ctx());
                                }

                                ui.separator();
                                ui.checkbox(&mut self.bik_loop, "Loop");

                                ui.separator();
                                ui.small(if has_audio {
                                    "Audio track detected"
                                } else {
                                    "No audio track detected"
                                });

                                ui.separator();

                                let mut bik_volume = self
                                    .audio_player
                                    .as_ref()
                                    .map(|player| player.volume())
                                    .unwrap_or(1.0);

                                if ui
                                    .add(egui::Slider::new(&mut bik_volume, 0.0..=2.0).text("Volume"))
                                    .changed()
                                {
                                    if let Some(player) = self.audio_player.as_ref() {
                                        player.set_volume(bik_volume);
                                    }
                                }
                            });

                            if let Some(err) = &self.bik_audio_error {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    format!("BIK audio error: {}", err),
                                );
                            }

                            let mut timeline_secs =
                                self.bik_current_time_seconds.clamp(0.0, total_duration.max(0.0));

                            let timeline_response = ui.add_sized(
                                [ui.available_width(), 18.0],
                                egui::Slider::new(&mut timeline_secs, 0.0..=total_duration.max(0.01))
                                    .show_value(false),
                            );

                            if timeline_response.changed() {
                                self.seek_bik_to_time(timeline_secs, ui.ctx());
                            }

                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "Frame {}",
                                    self.bik_current_frame.saturating_add(1)
                                ));

                                ui.separator();

                                ui.label(format!(
                                    "{:.2} / {:.2} sec",
                                    self.bik_current_time_seconds,
                                    total_duration
                                ));

                                ui.separator();

                                ui.label(format!(
                                    "~{} frames @ {:.3} fps",
                                    estimated_frames,
                                    preview_fps
                                ));
                            });

                            ui.separator();

                            let preview_height = self.bik_view_height.clamp(180.0, 900.0);

                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), preview_height),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    let preview_hovered = ui.rect_contains_pointer(ui.max_rect());

                                    if preview_hovered {
                                        let scroll_y = ui.ctx().input(|i| i.smooth_scroll_delta.y);

                                        if scroll_y.abs() > 0.0 {
                                            let zoom_factor = (1.0 + scroll_y * 0.001).clamp(0.5, 1.5);
                                            self.bik_zoom = (self.bik_zoom * zoom_factor).clamp(0.25, 8.0);
                                            ui.ctx().request_repaint();
                                        }
                                    }

                                    if let Some(texture) = self.bik_texture.as_ref() {
                                        let tex_size = texture.size_vec2();
                                        let available = ui.available_size();

                                        let fit_scale =
                                            (available.x / tex_size.x).min(available.y / tex_size.y).min(1.0);

                                        let desired_size = tex_size * fit_scale.max(0.1) * self.bik_zoom;

                                        egui::ScrollArea::both()
                                            .auto_shrink([false, false])
                                            .show(ui, |ui| {
                                                ui.image((texture.id(), desired_size));
                                            });
                                    } else {
                                        ui.label("No decoded frame available.");
                                    }
                                },
                            );

                            let (resize_rect, resize_response) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 12.0),
                                egui::Sense::drag(),
                            );

                            let resize_response =
                                resize_response.on_hover_cursor(egui::CursorIcon::ResizeVertical);

                            ui.painter().hline(
                                resize_rect.x_range(),
                                resize_rect.center().y,
                                egui::Stroke::new(1.5, egui::Color32::GRAY),
                            );

                            if resize_response.dragged() {
                                let delta = ui.ctx().input(|i| i.pointer.delta()).y;
                                self.bik_view_height =
                                    (self.bik_view_height + delta).clamp(180.0, 900.0);
                                ui.ctx().request_repaint();
                            }

                            ui.separator();
                            ui.heading("Video");
                            ui.separator();
                            ui.label(file_label);
                        } else if let Some(err) = &self.bik_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("Could not decode BIK: {}", err),
                            );
                        } else {
                            ui.label("BIK selected, but preview is still loading.");
                        }
                    }

                    Some("bnk") => {
                        egui::Panel::bottom("preview_bnk_audio_player")
                            .resizable(false)
                            .default_size(120.0)
                            .show_inside(ui, |ui| {
                                self.draw_bottom_audio_player(ui);
                            });

                        if let Some(bnk) = &self.bnk_file {
                            let entry_count = bnk.entry_count;
                            let entries = bnk.entries.clone();

                            ui.heading(format!("Entries ({})", entry_count));
                            ui.separator();

                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for entry in &entries {
                                    let duration_text = entry
                                        .estimated_duration_seconds()
                                        .map(|seconds| format!("{seconds:.2}s"))
                                        .unwrap_or_else(|| "?".to_owned());

                                    let label = format!(
                                        "#{:03}   {} Hz   {} bytes   {}",
                                        entry.index,
                                        entry.sample_rate,
                                        entry.byte_len,
                                        duration_text
                                    );

                                    let is_selected = self.selected_bnk_entry == Some(entry.index);

                                    if ui.selectable_label(is_selected, label).clicked() {
                                        self.selected_bnk_entry = Some(entry.index);
                                    }
                                }
                            });

                        } else if let Some(err) = &self.bnk_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("Could not read BNK: {}", err),
                            );
                        } else {
                            ui.label("BNK selected, but no bank info is loaded.");
                        }
                    }
                     
                    Some("scn") => {
                        if let Some(scn) = &self.scn_file {
                            draw_scene_viewer(
                                ui,
                                scn,
                                &self.scn_scene_models,
                                &self.scn_embedded_texture_previews,
                                &mut self.scn_viewer,
                                self.scn_view_height,
                            );
                            let (resize_rect, resize_response) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 12.0),
                                egui::Sense::drag(),
                            );

                            let resize_response =
                                resize_response.on_hover_cursor(egui::CursorIcon::ResizeVertical);

                            ui.painter().hline(
                                resize_rect.x_range(),
                                resize_rect.center().y,
                                egui::Stroke::new(1.5, egui::Color32::GRAY),
                            );

                            if resize_response.dragged() {
                                let delta = ui.ctx().input(|i| i.pointer.delta()).y;
                                self.scn_view_height =
                                    (self.scn_view_height + delta).clamp(260.0, 900.0);
                                ui.ctx().request_repaint();
                            }

                            ui.separator();

                            ui.horizontal_wrapped(|ui| {
                                ui.label(format!("Nodes: {}", scn.nodes.len()));
                                ui.separator();
                                ui.label(format!("Renderable: {}", scn.renderable_count()));
                                ui.separator();
                                ui.label(format!("Markers: {}", scn.marker_count()));
                                ui.separator();
                                ui.label(format!(
                                    "Embedded chunks: {}",
                                    scn.embedded_mesh_chunk_count()
                                ));
                                ui.separator();
                                ui.label(format!(
                                    "Embedded tris: {}",
                                    scn.embedded_triangle_count()
                                ));
                                ui.separator();
                                ui.label(format!("Resolved GEOs: {}", self.scn_scene_models.len()));
                                ui.separator();
                                ui.label(format!(
                                    "Missing archetypes: {}",
                                    self.scn_scene_unresolved.len()
                                ));
                            });

                            ui.small(
                                "This 3D view now draws embedded SCN static mesh plus any placed GEO props resolved from archetype names.",
                            );

                            if let Some(err) = &self.scn_scene_error {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    format!("SCN scene load error: {}", err),
                                );
                            }

                            if !self.scn_scene_unresolved.is_empty() {
                                egui::CollapsingHeader::new("Missing archetypes")
                                    .default_open(false)
                                    .show(ui, |ui| {
                                        for name in &self.scn_scene_unresolved {
                                            ui.monospace(name);
                                        }
                                    });
                            }

                            ui.separator();

                            let mut grouped_scene_nodes: std::collections::BTreeMap<
                                String,
                                Vec<(usize, String, String, [f32; 3], u32, u16)>,
                            > = std::collections::BTreeMap::new();

                            for node in &scn.nodes {
                                let group_name = Self::scn_node_group_name(&node.name);

                                grouped_scene_nodes
                                    .entry(group_name)
                                    .or_default()
                                    .push((
                                        node.index,
                                        node.name.clone(),
                                        node.archetype_label().to_owned(),
                                        node.translation,
                                        node.record_offset,
                                        node.flags,
                                    ));
                            }

                            egui::CollapsingHeader::new("Scene nodes")
                                .id_salt(format!("scene_nodes:{}", scn.path.display()))
                                .default_open(true)
                                .show(ui, |ui| {
                                    let scene_nodes_height = ui.available_height().clamp(180.0, 420.0);

                                    egui::ScrollArea::vertical()
                                        .id_salt(format!("scene_nodes_scroll:{}", scn.path.display()))
                                        .max_height(scene_nodes_height)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            for (group_name, nodes) in &grouped_scene_nodes {
                                                egui::CollapsingHeader::new(format!(
                                                    "{} ({})",
                                                    group_name,
                                                    nodes.len()
                                                ))
                                                .default_open(false)
                                                .show(ui, |ui| {
                                                    for (index, name, archetype, translation, record_offset, flags) in nodes {
                                                        let label = format!(
                                                            "#{:04}  {:<22}  {:<12}  pos=({:>9.2}, {:>9.2}, {:>9.2})  rec=0x{:08X}  flag=0x{:04X}",
                                                            index,
                                                            name,
                                                            archetype,
                                                            translation[0],
                                                            translation[1],
                                                            translation[2],
                                                            record_offset,
                                                            flags,
                                                        );

                                                        let is_selected =
                                                            self.selected_scn_node == Some(*index);

                                                        if ui.selectable_label(is_selected, label).clicked() {
                                                            self.selected_scn_node = Some(*index);
                                                            focus_scene_viewer_on_point(
                                                                &mut self.scn_viewer,
                                                                *translation,
                                                            );
                                                            ui.ctx().request_repaint();
                                                        }
                                                    }
                                                });
                                            }
                                        });
                                });
                        } else if let Some(err) = &self.scn_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("Could not read SCN: {}", err),
                            );
                        } else {
                            ui.label("SCN selected, but no scene info is loaded.");
                        }
                    }                

                    Some("geo") | Some("anm") => {
                        if let Some(geo) = &self.geo_file {
                            let active_rigid_clip = self
                                .active_geo_animation_file
                                .as_ref()
                                .and_then(|anm| anm.rigid_clip.as_ref());

                            let active_rigid_tag = self
                                .active_geo_animation
                                .as_ref()
                                .and_then(|path| path.file_name())
                                .map(|name| name.to_string_lossy().to_string());

                            draw_geo_viewer(
                                ui,
                                geo,
                                &self.geo_material_previews,
                                active_rigid_clip,
                                self.active_geo_animation_time,
                                active_rigid_tag.as_deref(),
                                &mut self.geo_viewer,
                                self.geo_view_height,
                            );

                            let (resize_rect, resize_response) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 12.0),
                                egui::Sense::drag(),
                            );

                            let resize_response =
                                resize_response.on_hover_cursor(egui::CursorIcon::ResizeVertical);

                            ui.painter().hline(
                                resize_rect.x_range(),
                                resize_rect.center().y,
                                egui::Stroke::new(1.5, egui::Color32::GRAY),
                            );

                            if resize_response.dragged() {
                                let delta = ui.ctx().input(|i| i.pointer.delta()).y;
                                self.geo_view_height =
                                    (self.geo_view_height + delta).clamp(260.0, 900.0);
                                ui.ctx().request_repaint();
                            }

                            ui.separator();

                            let active_anm_path = self.active_geo_animation.clone();
                            let show_animations =
                                geo.skeleton.is_some()
                                    && matches!(
                                        geo.asset_type,
                                        GeoAssetType::SkinnedMesh | GeoAssetType::RigidProp
                                    );

                            let loaded_geo_path = self.geo_loaded_path.clone();

                            let model_texture_refs = loaded_geo_path
                                .as_ref()
                                .and_then(|path| self.asset_links.model_to_textures.get(path).cloned());

                            let animation_groups = if show_animations {
                                loaded_geo_path
                                    .as_ref()
                                    .map(|path| self.animations_for_geo_grouped(path))
                                    .unwrap_or_default()
                            } else {
                                Vec::new()
                            };

                            let geo_stem = loaded_geo_path
                                .as_ref()
                                .map(|path| Self::asset_stem_lower(path))
                                .unwrap_or_default();

                            let mut newly_selected_animation: Option<PathBuf> = None;

                            if show_animations {
                                ui.columns(3, |columns| {
                                    let (left_cols, rest) = columns.split_at_mut(1);
                                    let left = &mut left_cols[0];
                                    let (middle_cols, right_cols) = rest.split_at_mut(1);
                                    let middle = &mut middle_cols[0];
                                    let right = &mut right_cols[0];

                                    left.heading("Model");
                                    left.separator();

                                    if let Some(model_path) = &loaded_geo_path {
                                        let label = model_path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_else(|| model_path.display().to_string());

                                        left.label(label);
                                    } else {
                                        left.label("(none)");
                                    }

                                    if let Some(anm_path) = &active_anm_path {
                                        let label = anm_path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_else(|| anm_path.display().to_string());
                                        left.separator();
                                        left.label(format!("Selected ANM: {}", label));
                                    }

                                    left.separator();
                                    left.heading("Textures");
                                    left.separator();

                                    if let Some(texture_refs) = &model_texture_refs {
                                        if texture_refs.is_empty() {
                                            left.label("(none found)");
                                        } else {
                                            for tex in texture_refs {
                                                match &tex.resolved_path {
                                                    Some(path) => {
                                                        if left.button(&tex.name).clicked() {
                                                            pending_jump = Some(path.clone());
                                                        }
                                                    }
                                                    None => {
                                                        left.colored_label(
                                                            egui::Color32::RED,
                                                            format!("{} (missing)", tex.name),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        left.label("(none found)");
                                    }

                                    left.separator();
                                    left.heading("Subsets");
                                    left.separator();

                                    for (i, subset) in geo.subsets.iter().enumerate() {
                                        left.horizontal(|ui| {
                                            ui.label(format!(
                                                "#{:02}  material={}  flags={}  start={}  count={}",
                                                i, subset.material, subset.flags, subset.start, subset.count
                                            ));

                                            if let Some(tex_name) = geo.texture_names.get(subset.material) {
                                                ui.label(" -> ");

                                                if let Some(texture_refs) = &model_texture_refs {
                                                    if let Some(tex_ref) =
                                                        texture_refs.iter().find(|t| t.name == *tex_name)
                                                    {
                                                        match &tex_ref.resolved_path {
                                                            Some(_path) => {
                                                                ui.label(tex_name);
                                                            }
                                                            None => {
                                                                ui.colored_label(
                                                                    egui::Color32::RED,
                                                                    format!("{} (missing)", tex_name),
                                                                );
                                                            }
                                                        }
                                                    } else {
                                                        ui.label(tex_name);
                                                    }
                                                } else {
                                                    ui.label(tex_name);
                                                }
                                            }
                                        });
                                    }

                                    middle.heading("Skeleton / Bones");
                                    middle.separator();

                                    if let Some(skeleton) = &geo.skeleton {
                                        middle.label(format!("Bone count: {}", skeleton.bone_count));
                                        middle.separator();

                                        egui::ScrollArea::vertical()
                                            .id_salt(format!(
                                                "geo_bones_scroll:{}",
                                                loaded_geo_path
                                                    .as_ref()
                                                    .map(|p| p.display().to_string())
                                                    .unwrap_or_default()
                                            ))
                                            .show(middle, |ui| {
                                                for (i, name) in skeleton.names.iter().enumerate() {
                                                    let parent_text =
                                                        match skeleton.parent.get(i).and_then(|p| *p) {
                                                            Some(parent) => parent.to_string(),
                                                            None => "-".to_owned(),
                                                        };

                                                    ui.label(format!(
                                                        "#{:03}  parent={}  {}",
                                                        i, parent_text, name
                                                    ));
                                                }
                                            });
                                    } else {
                                        middle.label("No skeleton detected.");
                                    }

                                    right.heading("Animations");
                                    right.separator();

                                    if let Some(model_path) = &loaded_geo_path {
                                        if animation_groups.is_empty() {
                                            right.label("No ANM files found in this folder.");
                                        } else {
                                    if let Some(anm_path) = &active_anm_path {
                                        let label = anm_path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_else(|| anm_path.display().to_string());

                                        right.small(format!("Selected: {}", label));

                                        if let Some(anm) = &self.active_geo_animation_file {
                                            if let Some(clip) = &anm.rigid_clip {
                                                let frame_count = clip
                                                    .streams
                                                    .iter()
                                                    .map(|stream| stream.rotations_xyzw.len())
                                                    .min()
                                                    .unwrap_or(0);

                                                right.horizontal(|ui| {
                                                    if ui
                                                        .button(if self.active_geo_animation_playing { "Pause" } else { "Play" })
                                                        .clicked()
                                                    {
                                                        self.active_geo_animation_playing = !self.active_geo_animation_playing;
                                                    }

                                                    if ui.button("Stop").clicked() {
                                                        self.active_geo_animation_playing = false;
                                                        self.active_geo_animation_time = 0.0;
                                                    }

                                                    ui.checkbox(&mut self.active_geo_animation_loop, "Loop");
                                                });

                                                let duration = clip.duration_seconds.max(1.0 / clip.sample_rate.max(1.0));

                                                if right
                                                    .add(
                                                        egui::Slider::new(&mut self.active_geo_animation_time, 0.0..=duration)
                                                            .text("Time"),
                                                    )
                                                    .changed()
                                                {
                                                    self.active_geo_animation_playing = false;
                                                }

                                                right.add(
                                                    egui::Slider::new(&mut self.active_geo_animation_speed, 0.1..=3.0)
                                                        .text("Speed"),
                                                );

                                                right.small(format!(
                                                    "Experimental structural playback | {} streams | {} frames | {:.2} sec",
                                                    clip.streams.len(),
                                                    clip.frame_times.len().max(
                                                        clip.streams
                                                            .iter()
                                                            .map(|stream| stream.rotations_xyzw.len())
                                                            .min()
                                                            .unwrap_or(0)
                                                    ),
                                                    duration
                                                ));
                                            } else {
                                                right.colored_label(
                                                    egui::Color32::YELLOW,
                                                    "This ANM did not decode in the experimental rigid-prop player.",
                                                );
                                            }
                                        }

                                        if let Some(err) = &self.active_geo_animation_error {
                                            right.colored_label(
                                                egui::Color32::RED,
                                                format!("Animation load error: {}", err),
                                            );
                                        }

                                        right.separator();
                                    }

                                            egui::ScrollArea::vertical()
                                                .id_salt(format!(
                                                    "geo_anim_scroll:{}",
                                                    model_path.display()
                                                ))
                                                .show(right, |ui| {
                                                    for (group_name, paths) in &animation_groups {
                                                        egui::CollapsingHeader::new(group_name.as_str())
                                                            .id_salt(format!(
                                                                "geo_anim_group:{}:{}",
                                                                model_path.display(),
                                                                group_name
                                                            ))
                                                            .default_open(group_name == &geo_stem)
                                                            .show(ui, |ui| {
                                                                for anm_path in paths {
                                                                    let label = anm_path
                                                                        .file_name()
                                                                        .map(|name| {
                                                                            name.to_string_lossy().to_string()
                                                                        })
                                                                        .unwrap_or_else(|| {
                                                                            anm_path.display().to_string()
                                                                        });

                                                                    let is_selected =
                                                                        active_anm_path.as_ref() == Some(anm_path);

                                                                    if ui.selectable_label(is_selected, label).clicked() {
                                                                        self.active_geo_animation = Some(anm_path.clone());
                                                                        self.active_geo_animation_file = None;
                                                                        self.active_geo_animation_loaded_path = None;
                                                                        self.active_geo_animation_error = None;
                                                                        self.active_geo_animation_time = 0.0;
                                                                        self.active_geo_animation_playing = true;
                                                                    }
                                                                }
                                                            });
                                                    }
                                                });
                                        }
                                    } else {
                                        right.label("No GEO is currently loaded.");
                                    }
                                });
                            } else {
                                ui.columns(2, |columns| {
                                    let (left_cols, right_cols) = columns.split_at_mut(1);
                                    let left = &mut left_cols[0];
                                    let right = &mut right_cols[0];

                                    left.heading("Model");
                                    left.separator();

                                    if let Some(model_path) = &loaded_geo_path {
                                        let label = model_path
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                            .unwrap_or_else(|| model_path.display().to_string());

                                        left.label(label);
                                    } else {
                                        left.label("(none)");
                                    }

                                    left.separator();
                                    left.heading("Textures");
                                    left.separator();

                                    if let Some(texture_refs) = &model_texture_refs {
                                        if texture_refs.is_empty() {
                                            left.label("(none found)");
                                        } else {
                                            for tex in texture_refs {
                                                match &tex.resolved_path {
                                                    Some(path) => {
                                                        if left.button(&tex.name).clicked() {
                                                            pending_jump = Some(path.clone());
                                                        }
                                                    }
                                                    None => {
                                                        left.colored_label(
                                                            egui::Color32::RED,
                                                            format!("{} (missing)", tex.name),
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        left.label("(none found)");
                                    }

                                    left.separator();
                                    left.heading("Subsets");
                                    left.separator();

                                    for (i, subset) in geo.subsets.iter().enumerate() {
                                        left.horizontal(|ui| {
                                            ui.label(format!(
                                                "#{:02}  material={}  flags={}  start={}  count={}",
                                                i, subset.material, subset.flags, subset.start, subset.count
                                            ));

                                            if let Some(tex_name) = geo.texture_names.get(subset.material) {
                                                ui.label(" -> ");

                                                if let Some(texture_refs) = &model_texture_refs {
                                                    if let Some(tex_ref) =
                                                        texture_refs.iter().find(|t| t.name == *tex_name)
                                                    {
                                                        match &tex_ref.resolved_path {
                                                            Some(_path) => {
                                                                ui.label(tex_name);
                                                            }
                                                            None => {
                                                                ui.colored_label(
                                                                    egui::Color32::RED,
                                                                    format!("{} (missing)", tex_name),
                                                                );
                                                            }
                                                        }
                                                    } else {
                                                        ui.label(tex_name);
                                                    }
                                                } else {
                                                    ui.label(tex_name);
                                                }
                                            }
                                        });
                                    }

                                    right.heading("Skeleton / Bones");
                                    right.separator();

                                    if let Some(skeleton) = &geo.skeleton {
                                        right.label(format!("Bone count: {}", skeleton.bone_count));
                                        right.separator();

                                        egui::ScrollArea::vertical()
                                            .id_salt(format!(
                                                "geo_bones_scroll:{}",
                                                loaded_geo_path
                                                    .as_ref()
                                                    .map(|p| p.display().to_string())
                                                    .unwrap_or_default()
                                            ))
                                            .show(right, |ui| {
                                                for (i, name) in skeleton.names.iter().enumerate() {
                                                    let parent_text =
                                                        match skeleton.parent.get(i).and_then(|p| *p) {
                                                            Some(parent) => parent.to_string(),
                                                            None => "-".to_owned(),
                                                        };

                                                    ui.label(format!(
                                                        "#{:03}  parent={}  {}",
                                                        i, parent_text, name
                                                    ));
                                                }
                                            });
                                    } else {
                                        right.label("No skeleton detected.");
                                    }
                                });
                            }

                            if let Some(path) = newly_selected_animation {
                                self.active_geo_animation = Some(path);
                            }
                        } else if let Some(err) = &self.geo_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("Could not read GEO: {}", err),
                            );
                        } else if matches!(self.selected_extension().as_deref(), Some("anm")) {
                            ui.label("ANM selected.");
                            ui.label("Open a rigged or skinned GEO to browse or test animations.");
                        } else {
                            ui.label("GEO selected, but no GEO info is loaded.");
                        }
                    }

                    Some("ogg") | Some("wav") => {
                        egui::Panel::bottom("preview_audio_player")
                            .resizable(false)
                            .default_size(120.0)
                            .show_inside(ui, |ui| {
                                self.draw_bottom_audio_player(ui);
                            });

                        ui.label("Audio file selected.");
                        ui.label("The transport bar is at the bottom of this preview window.");
                    }

                    _ => {
                        ui.label("Placeholder preview panel for now.");
                    }
                }
            }else {
                ui.label("Open the game folder, then click a file.");
            }
        });

        if let Some(path) = pending_jump {
            self.jump_to_file(path);
        }
    }
}