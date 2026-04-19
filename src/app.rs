use crate::{
    audio_player::AudioPlayer,
    bnk::{format_name, load_bnk, BnkFile},
    dds_preview::{load_dds_preview, DdsPreview},
    fs_tree::{category_name, classify_path, scan_game_data, AssetCategory, FileNode, NodeKind},
    geo::{load_geo, GeoFile},
    geo_viewer::{draw_geo_viewer, reset_geo_viewer, GeoViewerState},
};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug)]
struct ModelTextureRef {
    name: String,
    resolved_path: Option<PathBuf>,
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

    geo_file: Option<GeoFile>,
    geo_loaded_path: Option<PathBuf>,
    geo_error: Option<String>,

    geo_material_previews: Vec<Option<DdsPreview>>,
    geo_materials_loaded_path: Option<PathBuf>,
    geo_material_error: Option<String>,

    asset_links: AssetLinks,

    geo_viewer: GeoViewerState,
    geo_viewer_path: Option<PathBuf>,
    geo_view_height: f32,

}

impl ModToolApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            game_root: None,
            tree: Vec::new(),
            selected_file: None,
            status: "Choose your Little Britain install folder.".to_owned(),

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

            geo_file: None,
            geo_loaded_path: None,
            geo_error: None,

            geo_material_previews: Vec::new(),
            geo_materials_loaded_path: None,
            geo_material_error: None,

            asset_links: AssetLinks::default(),

            geo_viewer: GeoViewerState::default(),
            geo_viewer_path: None,
            geo_view_height: 520.0,
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
                    self.geo_file = None;
                    self.geo_loaded_path = None;
                    self.geo_error = None;
                    self.geo_viewer = GeoViewerState::default();
                    self.geo_view_height = 520.0;
                    self.geo_viewer_path = None;
                    self.geo_material_previews.clear();
                    self.geo_materials_loaded_path = None;
                    self.geo_material_error = None;
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

                    self.geo_file = None;
                    self.geo_loaded_path = None;
                    self.geo_error = None;

                    self.geo_viewer = GeoViewerState::default();
                    self.geo_viewer_path = None;
                    self.geo_view_height = 520.0;

                    self.geo_material_previews.clear();
                    self.geo_materials_loaded_path = None;
                    self.geo_material_error = None;

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
                        Self::ui_category_group(ui, "Scenes", &scenes, selected_file);
                        Self::ui_category_group(ui, "Logs", &logs, selected_file);
                        Self::ui_category_group(ui, "Other", &other, selected_file);
                    });
            }
        }
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

    fn ensure_geo_loaded(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.geo_file = None;
            self.geo_loaded_path = None;
            self.geo_error = None;
            self.geo_viewer_path = None;
            return;
        };

        if self.geo_loaded_path.as_ref() == Some(&path) {
            return;
        }

        self.geo_file = None;
        self.geo_error = None;
        self.geo_loaded_path = Some(path.clone());

        let is_geo = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("geo"))
            .unwrap_or(false);

        if !is_geo {
            self.geo_viewer_path = None;
            return;
        }

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
        let Some(geo_path) = self.selected_file.clone() else {
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
        self.ensure_bnk_loaded();
        self.ensure_dds_preview_loaded(ui.ctx());
        self.ensure_geo_loaded();
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

                    Some("geo") => {
                        if let Some(geo) = &self.geo_file {
                            draw_geo_viewer(
                                ui,
                                geo,
                                &self.geo_material_previews,
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
                                self.geo_view_height = (self.geo_view_height + delta).clamp(260.0, 900.0);
                                ui.ctx().request_repaint();
                            }

                            ui.separator();

                            ui.columns(2, |columns| {
                                let (left_cols, right_cols) = columns.split_at_mut(1);
                                let left = &mut left_cols[0];
                                let right = &mut right_cols[0];

                            left.heading("Model");
                            left.separator();

                            if let Some(model_path) = &self.geo_loaded_path {
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

                            if let Some(model_path) = &self.geo_loaded_path {
                                if let Some(texture_refs) = self.asset_links.model_to_textures.get(model_path) {
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

                                        if let Some(model_path) = &self.geo_loaded_path {
                                            if let Some(texture_refs) = self.asset_links.model_to_textures.get(model_path) {
                                                if let Some(tex_ref) = texture_refs.iter().find(|t| t.name == *tex_name) {
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

                                    egui::ScrollArea::vertical().show(right, |ui| {
                                        for (i, name) in skeleton.names.iter().enumerate() {
                                            let parent_text = match skeleton.parent.get(i).and_then(|p| *p) {
                                                Some(parent) => parent.to_string(),
                                                None => "-".to_owned(),
                                            };

                                            ui.label(format!("#{:03}  parent={}  {}", i, parent_text, name));
                                        }
                                    });
                                } else {
                                    right.label("No skeleton detected.");
                                }
                            });
                        } else if let Some(err) = &self.geo_error {
                            ui.colored_label(
                                egui::Color32::RED,
                                format!("Could not read GEO: {}", err),
                            );
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