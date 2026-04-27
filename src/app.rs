use crate::{
    anm::{AnmFile, load_anm},
    audio_player::AudioPlayer,
    bik_preview::{
        BikPreview, BikWorkerEvent, extract_bik_audio_wav, load_bik_preview, spawn_bik_decoder,
    },
    bnk::{BnkFile, format_name, load_bnk},
    dds_preview::{DdsPreview, DdsRawPreview, dds_preview_from_raw, load_dds_preview, load_dds_raw_preview},
    fs_tree::{AssetCategory, FileNode, NodeKind, category_name, classify_path, scan_game_data},
    geo::{GeoAssetType, GeoFile, load_geo},
    geo_viewer::{
        GeoViewerState, SceneGeoModel, SceneSelection, draw_geo_viewer, draw_scene_viewer,
        focus_scene_viewer_on_point, reset_geo_viewer, reset_scene_viewer,
    },
    mod_workspace::{
        ModPackage, create_lua_mod, create_lua_script, read_text_file, scan_mods, write_text_file,
    },
    loading::{self, LoadingTask},
    scn::{ScnFile, ScnMeshChunk, load_scn},
};
use eframe::egui;
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
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

#[derive(Clone, Debug)]
struct LoadedDdsPreview {
    path: PathBuf,
    raw: DdsRawPreview,
}

#[derive(Clone, Debug)]
struct LoadedSceneGeoModel {
    archetype: String,
    path: PathBuf,
    geo: GeoFile,
    textures: Vec<Option<LoadedDdsPreview>>,
}

#[derive(Clone, Debug)]
struct LoadedSceneData {
    path: PathBuf,
    scn: ScnFile,
    models: Vec<LoadedSceneGeoModel>,
    embedded_textures: std::collections::HashMap<String, LoadedDdsPreview>,
    unresolved: Vec<String>,
    marker_geo_overrides: std::collections::HashMap<usize, String>,
    failed: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FolderLoadMode {
    Open,
    Rescan,
}

#[derive(Clone, Debug)]
enum LoadingJobKind {
    OpenGameFolder,
    RescanGameFolder,
    OpenScene(PathBuf),
    OpenBikPreview(PathBuf),
    LoadBikAudio(PathBuf),
}

enum LoadingJobResult {
    FolderLoaded {
        mode: FolderLoadMode,
        root: PathBuf,
        tree: Vec<FileNode>,
        game_code_map: GameCodeMap,
        mods: Result<Vec<ModPackage>, String>,
        asset_links: AssetLinks,
    },
    SceneLoaded(LoadedSceneData),
    BikPreviewLoaded {
        path: PathBuf,
        preview: BikPreview,
    },
    BikAudioLoaded {
        path: PathBuf,
        wav_bytes: Option<Vec<u8>>,
    },
    Error(String),
}

enum LoadingJobMessage {
    Progress {
        id: u64,
        detail: String,
        fraction: Option<f32>,
    },
    Finished {
        id: u64,
        result: LoadingJobResult,
    },
}

struct ActiveLoadingJob {
    kind: LoadingJobKind,
    task: LoadingTask,
    rx: mpsc::Receiver<LoadingJobMessage>,

    // When a job finishes after the overlay was already visible,
    // keep the overlay alive briefly at 100% so users actually see completion.
    pending_result: Option<LoadingJobResult>,
    finished_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
struct GameCodeMap {
    exe_path: Option<PathBuf>,
    symbol_dump_path: Option<PathBuf>,
    strings_path: Option<PathBuf>,
    bink_proxy_path: Option<PathBuf>,
    real_bink_path: Option<PathBuf>,
    modloader_path: Option<PathBuf>,
    error: Option<String>,
    rtti_classes: Vec<String>,
    function_names: Vec<String>,
    source_paths: Vec<String>,
    game_modes: Vec<String>,
    frontend_functions: Vec<String>,
    character_names: Vec<String>,
    resource_names: Vec<String>,
    asset_refs: Vec<String>,
    raw_tokens: Vec<String>,
    code_refs: Vec<String>,
    script_tokens: Vec<String>,
    modloader_tokens: Vec<String>,
    injection_notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ScnMarkerSummary {
    actor_like: usize,
    cameras: usize,
    checkpoints: usize,
    gameplay_targets: usize,
    path_nodes: usize,
    player_starts: usize,
    player_ends: usize,
    spawns: usize,
    traffic: usize,
    other: usize,
}

struct TexturePreviewWindow {
    id: u64,
    title: String,
    preview: DdsPreview,
    zoom: f32,
    open: bool,
}

struct AudioPreviewWindow {
    id: u64,
    title: String,
    path: PathBuf,
    audio_player: Option<AudioPlayer>,
    audio_error: Option<String>,
    open: bool,
}

struct BnkPreviewWindow {
    id: u64,
    title: String,
    path: PathBuf,
    bnk_file: Option<BnkFile>,
    error: Option<String>,
    audio_player: Option<AudioPlayer>,
    audio_error: Option<String>,
    selected_entry: Option<usize>,
    open: bool,
}

struct GeoPreviewWindow {
    id: u64,
    title: String,
    path: PathBuf,
    geo_file: Option<GeoFile>,
    error: Option<String>,
    material_previews: Vec<Option<DdsPreview>>,
    material_error: Option<String>,
    texture_refs: Vec<ModelTextureRef>,
    animation_groups: Vec<(String, Vec<PathBuf>)>,
    viewer: GeoViewerState,
    viewer_height: f32,
    active_animation: Option<PathBuf>,
    active_animation_file: Option<AnmFile>,
    active_animation_error: Option<String>,
    active_animation_playing: bool,
    active_animation_loop: bool,
    active_animation_time: f32,
    active_animation_speed: f32,
    open: bool,
}

struct FilePreviewWindow {
    id: u64,
    title: String,
    path: PathBuf,
    open: bool,
    preview_text: Option<String>,
    preview_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioWindowKey {
    Main,
    Audio(u64),
    Bnk(u64),
}

#[derive(Debug)]
enum AudioWindowCommand {
    PlayFile {
        path: PathBuf,
        source: AudioWindowKey,
    },
    PlayData {
        label: String,
        wav_bytes: Vec<u8>,
        source: AudioWindowKey,
    },
    PauseResume(AudioWindowKey),
    Stop(AudioWindowKey),
    Seek {
        seconds: f32,
        source: AudioWindowKey,
    },
    SetVolume(f32),
}

pub struct ModToolApp {
    game_root: Option<PathBuf>,
    tree: Vec<FileNode>,
    selected_file: Option<PathBuf>,
    status: String,
    dark_mode: bool,
    gpu_target_format: Option<eframe::egui_wgpu::wgpu::TextureFormat>,

    dds_preview: Option<DdsPreview>,
    dds_preview_path: Option<PathBuf>,
    dds_error: Option<String>,
    texture_zoom: f32,
    dds_view_height: f32,
    texture_preview_windows: Vec<TexturePreviewWindow>,
    next_texture_preview_window_id: u64,
    audio_preview_windows: Vec<AudioPreviewWindow>,
    next_audio_preview_window_id: u64,
    bnk_preview_windows: Vec<BnkPreviewWindow>,
    next_bnk_preview_window_id: u64,
    geo_preview_windows: Vec<GeoPreviewWindow>,
    next_geo_preview_window_id: u64,
    file_preview_windows: Vec<FilePreviewWindow>,
    next_file_preview_window_id: u64,

    active_audio_window: Option<AudioWindowKey>,

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
    scn_marker_geo_overrides: std::collections::HashMap<usize, String>,
    scn_scene_error: Option<String>,
    selected_scn_node: Option<usize>,
    selected_scn_chunk: Option<usize>,
    hidden_scn_nodes: std::collections::BTreeSet<usize>,
    hidden_scn_chunks: std::collections::BTreeSet<usize>,
    scn_viewer: GeoViewerState,
    scn_view_height: f32,
    scn_embedded_texture_previews: std::collections::HashMap<String, DdsPreview>,

    geo_material_previews: Vec<Option<DdsPreview>>,
    geo_materials_loaded_path: Option<PathBuf>,
    geo_material_error: Option<String>,

    asset_links: AssetLinks,
    game_code_map: GameCodeMap,
    mods: Vec<ModPackage>,
    mods_error: Option<String>,
    new_mod_name: String,
    new_script_name: String,
    selected_mod_index: Option<usize>,
    mod_script_path: Option<PathBuf>,
    mod_script_text: String,
    mod_script_dirty: bool,
    mod_script_window_open: bool,
    mod_script_error: Option<String>,
    content_browser_folder: Option<PathBuf>,
    geo_animation_groups_path: Option<PathBuf>,
    geo_animation_groups: Vec<(String, Vec<PathBuf>)>,

    geo_viewer: GeoViewerState,
    geo_viewer_path: Option<PathBuf>,
    geo_view_height: f32,

    bik_preview: Option<BikPreview>,
    bik_preview_path: Option<PathBuf>,
    bik_texture: Option<egui::TextureHandle>,
    bik_error: Option<String>,
    bik_audio_error: Option<String>,
    bik_audio_wav: Option<Arc<[u8]>>,
    bik_audio_path: Option<PathBuf>,
    bik_audio_active: bool,
    pending_bik_audio_start_seconds: Option<f32>,
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

    next_loading_job_id: u64,
    active_loading_job: Option<ActiveLoadingJob>,
}

impl ModToolApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let gpu_target_format = cc.wgpu_render_state.as_ref().map(|rs| rs.target_format);
        Self {
            game_root: None,
            tree: Vec::new(),
            selected_file: None,
            status: "Choose your Little Britain install folder.".to_owned(),
            dark_mode: true,
            gpu_target_format,

            dds_preview: None,
            dds_preview_path: None,
            dds_error: None,
            texture_zoom: 1.0,
            dds_view_height: 420.0,
            texture_preview_windows: Vec::new(),
            next_texture_preview_window_id: 1,
            audio_preview_windows: Vec::new(),
            next_audio_preview_window_id: 1,
            bnk_preview_windows: Vec::new(),
            next_bnk_preview_window_id: 1,
            geo_preview_windows: Vec::new(),
            next_geo_preview_window_id: 1,
            file_preview_windows: Vec::new(),
            next_file_preview_window_id: 1,

            active_audio_window: None,

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
            game_code_map: GameCodeMap::default(),
            mods: Vec::new(),
            mods_error: None,
            new_mod_name: "MyLuaMod".to_owned(),
            new_script_name: "new_script".to_owned(),
            selected_mod_index: None,
            mod_script_path: None,
            mod_script_text: String::new(),
            mod_script_dirty: false,
            mod_script_window_open: false,
            mod_script_error: None,
            content_browser_folder: None,
            geo_animation_groups_path: None,
            geo_animation_groups: Vec::new(),

            geo_viewer: GeoViewerState::with_target_format(gpu_target_format),
            geo_viewer_path: None,
            geo_view_height: 520.0,

            scn_scene_models: Vec::new(),
            scn_scene_models_path: None,
            scn_scene_unresolved: Vec::new(),
            scn_marker_geo_overrides: std::collections::HashMap::new(),
            scn_scene_error: None,
            selected_scn_node: None,
            selected_scn_chunk: None,
            hidden_scn_nodes: std::collections::BTreeSet::new(),
            hidden_scn_chunks: std::collections::BTreeSet::new(),
            scn_viewer: GeoViewerState::with_target_format(gpu_target_format),
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
            pending_bik_audio_start_seconds: None,
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

            next_loading_job_id: 1,
            active_loading_job: None,
        }
    }

    fn open_game_folder(&mut self, ctx: &egui::Context) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            self.start_folder_load_job(ctx, folder, FolderLoadMode::Open);
        }
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        if let Some(root) = self.game_root.clone() {
            self.start_folder_load_job(ctx, root, FolderLoadMode::Rescan);
        }
    }


    fn start_loading_job<F>(
        &mut self,
        ctx: &egui::Context,
        kind: LoadingJobKind,
        title: String,
        work: F,
    ) where
        F: FnOnce(u64, mpsc::Sender<LoadingJobMessage>) + Send + 'static,
    {
        let id = self.next_loading_job_id;
        self.next_loading_job_id = self.next_loading_job_id.wrapping_add(1).max(1);

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || work(id, tx));

        self.active_loading_job = Some(ActiveLoadingJob {
            kind,
            task: LoadingTask::new(id, title),
            rx,
            pending_result: None,
            finished_at: None,
        });
        ctx.request_repaint();
    }

    fn send_loading_progress(
        tx: &mpsc::Sender<LoadingJobMessage>,
        id: u64,
        detail: impl Into<String>,
        fraction: Option<f32>,
    ) {
        let _ = tx.send(LoadingJobMessage::Progress {
            id,
            detail: detail.into(),
            fraction,
        });
    }

    fn send_loading_finished(
        tx: mpsc::Sender<LoadingJobMessage>,
        id: u64,
        result: LoadingJobResult,
    ) {
        let _ = tx.send(LoadingJobMessage::Finished { id, result });
    }

    fn is_loading_scene_path(&self, path: &Path) -> bool {
        matches!(
            self.active_loading_job.as_ref().map(|job| &job.kind),
            Some(LoadingJobKind::OpenScene(active_path)) if active_path.as_path() == path
        )
    }

    fn is_loading_bik_preview_path(&self, path: &Path) -> bool {
        matches!(
            self.active_loading_job.as_ref().map(|job| &job.kind),
            Some(LoadingJobKind::OpenBikPreview(active_path)) if active_path.as_path() == path
        )
    }

    fn is_loading_bik_audio_path(&self, path: &Path) -> bool {
        matches!(
            self.active_loading_job.as_ref().map(|job| &job.kind),
            Some(LoadingJobKind::LoadBikAudio(active_path)) if active_path.as_path() == path
        )
    }

    fn loading_title(action: &str, path: &Path) -> String {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        format!("{action} {name}...")
    }

    fn start_folder_load_job(&mut self, ctx: &egui::Context, root: PathBuf, mode: FolderLoadMode) {
        let title = match mode {
            FolderLoadMode::Open => "Opening game folder...".to_owned(),
            FolderLoadMode::Rescan => "Rescanning game folder...".to_owned(),
        };
        let kind = match mode {
            FolderLoadMode::Open => LoadingJobKind::OpenGameFolder,
            FolderLoadMode::Rescan => LoadingJobKind::RescanGameFolder,
        };

        self.start_loading_job(ctx, kind, title, move |id, tx| {
            let result = (|| -> Result<LoadingJobResult, String> {
                Self::send_loading_progress(&tx, id, "Scanning Data folder", Some(0.08));
                let tree = scan_game_data(&root).map_err(|err| err.to_string())?;

                Self::send_loading_progress(&tx, id, "Scanning EXE strings and mod-loader files", Some(0.32));
                let game_code_map = Self::scan_game_code_map(&root);

                Self::send_loading_progress(&tx, id, "Scanning Mods folder", Some(0.48));
                let mods = scan_mods(&root).map_err(|err| err.to_string());

                Self::send_loading_progress(&tx, id, "Building model texture links", Some(0.62));
                let asset_links = Self::build_asset_links_from_tree(&tree);

                Self::send_loading_progress(&tx, id, "Finalizing", Some(0.95));
                Ok(LoadingJobResult::FolderLoaded {
                    mode,
                    root,
                    tree,
                    game_code_map,
                    mods,
                    asset_links,
                })
            })();

            let result = match result {
                Ok(result) => result,
                Err(err) => LoadingJobResult::Error(err),
            };
            Self::send_loading_finished(tx, id, result);
        });
    }

    fn reset_loaded_asset_state(&mut self, clear_selection: bool) {
        if clear_selection {
            self.selected_file = None;
            self.content_browser_folder = None;
        } else if self
            .content_browser_folder
            .as_ref()
            .map(|path| !path.is_dir())
            .unwrap_or(false)
        {
            self.content_browser_folder = None;
        }

        self.dds_preview = None;
        self.dds_preview_path = None;
        self.dds_error = None;
        self.texture_zoom = 1.0;
        self.dds_view_height = 420.0;

        self.bnk_file = None;
        self.bnk_loaded_path = None;
        self.bnk_error = None;
        self.selected_bnk_entry = None;
        self.active_audio_window = None;

        self.anm_file = None;
        self.anm_loaded_path = None;
        self.anm_error = None;
        self.anm_geo_overrides.clear();
        self.active_geo_animation = None;

        self.geo_file = None;
        self.geo_loaded_path = None;
        self.geo_error = None;
        self.geo_viewer.reset_with_target_format(self.gpu_target_format);
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
        self.reset_scn_scene_state();

        self.reset_bik_state();
    }

    fn apply_folder_loaded(
        &mut self,
        mode: FolderLoadMode,
        root: PathBuf,
        tree: Vec<FileNode>,
        game_code_map: GameCodeMap,
        mods: Result<Vec<ModPackage>, String>,
        asset_links: AssetLinks,
    ) {
        self.game_root = Some(root);
        self.tree = tree;
        self.game_code_map = game_code_map;
        match mods {
            Ok(mods) => {
                self.mods = mods;
                self.mods_error = None;
                if self
                    .selected_mod_index
                    .map(|index| index >= self.mods.len())
                    .unwrap_or(false)
                {
                    self.selected_mod_index = None;
                }
            }
            Err(err) => {
                self.mods.clear();
                self.mods_error = Some(err);
                self.selected_mod_index = None;
            }
        }
        self.asset_links = asset_links;
        self.geo_animation_groups_path = None;
        self.geo_animation_groups.clear();
        self.reset_loaded_asset_state(mode == FolderLoadMode::Open);
        self.status = match mode {
            FolderLoadMode::Open => "Loaded Data folder.".to_owned(),
            FolderLoadMode::Rescan => "Rescanned Data folder.".to_owned(),
        };
    }

    fn poll_loading_jobs(&mut self, ctx: &egui::Context) {
        const READY_HOLD: Duration = Duration::from_millis(180);

        let mut result_to_apply: Option<LoadingJobResult> = None;
        let mut clear_job = false;
        let mut request_repaint = false;

        if let Some(job) = self.active_loading_job.as_mut() {
            // If the worker already finished and we are just showing "Ready" briefly,
            // wait a tiny moment before applying the result and hiding the overlay.
            if job.pending_result.is_some() {
                if let Some(finished_at) = job.finished_at {
                    if finished_at.elapsed() >= READY_HOLD {
                        result_to_apply = job.pending_result.take();
                        clear_job = true;
                    } else {
                        request_repaint = true;
                    }
                } else {
                    result_to_apply = job.pending_result.take();
                    clear_job = true;
                }
            } else {
                while let Ok(message) = job.rx.try_recv() {
                    match message {
                        LoadingJobMessage::Progress { id, detail, fraction } => {
                            if id == job.task.id {
                                job.task.set_progress(fraction, Some(detail));
                            }
                        }
                        LoadingJobMessage::Finished { id, result } => {
                            if id == job.task.id {
                                // If the overlay was visible, briefly show 100%.
                                // If the job finished before the overlay delay, apply instantly
                                // so fast actions still feel instant.
                                if job.task.should_show_overlay() {
                                    let done_label = match &result {
                                        LoadingJobResult::Error(_) => "Failed",
                                        _ => "Ready",
                                    };

                                    job.task.set_progress(Some(1.0), Some(done_label.to_owned()));
                                    job.finished_at = Some(Instant::now());
                                    job.pending_result = Some(result);
                                    request_repaint = true;
                                } else {
                                    result_to_apply = Some(result);
                                    clear_job = true;
                                }

                                break;
                            }
                        }
                    }
                }

                if !clear_job && job.pending_result.is_none() {
                    request_repaint = true;
                }
            }
        }

        if clear_job {
            self.active_loading_job = None;
        }

        if let Some(result) = result_to_apply {
            self.apply_loading_result(ctx, result);
        }

        if request_repaint {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn apply_loading_result(&mut self, ctx: &egui::Context, result: LoadingJobResult) {
        match result {
            LoadingJobResult::FolderLoaded {
                mode,
                root,
                tree,
                game_code_map,
                mods,
                asset_links,
            } => self.apply_folder_loaded(mode, root, tree, game_code_map, mods, asset_links),
            LoadingJobResult::SceneLoaded(data) => self.apply_loaded_scene(ctx, data),
            LoadingJobResult::BikPreviewLoaded { path, preview } => {
                if self.selected_file.as_ref() != Some(&path) {
                    return;
                }
                let first_frame = preview.first_frame.clone();
                self.bik_preview_path = Some(path);
                self.bik_preview = Some(preview);
                self.set_bik_texture_from_image(ctx, first_frame);
                self.bik_current_frame = 0;
                self.bik_current_time_seconds = 0.0;
                self.bik_error = None;
                self.bik_audio_error = None;
                ctx.request_repaint();
            }
            LoadingJobResult::BikAudioLoaded { path, wav_bytes } => {
                if self
                    .bik_preview
                    .as_ref()
                    .map(|preview| preview.path.as_path() != path.as_path())
                    .unwrap_or(true)
                {
                    return;
                }
                self.bik_audio_path = Some(path);
                self.bik_audio_wav = wav_bytes.map(Arc::<[u8]>::from);
                self.bik_audio_error = None;
                if self.bik_audio_wav.is_some() {
                    if let Some(start_seconds) = self.pending_bik_audio_start_seconds.take() {
                        self.start_bik_audio(ctx, start_seconds);
                    }
                } else {
                    self.pending_bik_audio_start_seconds = None;
                }
            }
            LoadingJobResult::Error(err) => {
                self.status = err.clone();
                match self.selected_extension().as_deref() {
                    Some("scn") => self.scn_error = Some(err),
                    Some("bik") => self.bik_error = Some(err),
                    _ => {}
                }
            }
        }
    }

    fn refresh_mod_workspace(&mut self) {
        let Some(root) = self.game_root.as_ref() else {
            self.mods.clear();
            self.mods_error = None;
            self.selected_mod_index = None;
            return;
        };

        match scan_mods(root) {
            Ok(mods) => {
                self.mods = mods;
                self.mods_error = None;
                if self
                    .selected_mod_index
                    .map(|index| index >= self.mods.len())
                    .unwrap_or(false)
                {
                    self.selected_mod_index = None;
                }
            }
            Err(err) => {
                self.mods_error = Some(err.to_string());
            }
        }
    }

    fn create_new_lua_mod(&mut self) {
        let Some(root) = self.game_root.as_ref() else {
            self.mods_error = Some("Open the Little Britain game folder first.".to_owned());
            return;
        };

        match create_lua_mod(root, &self.new_mod_name) {
            Ok(package) => {
                let script_to_open = package.scripts.first().cloned();
                self.status = format!("Created Lua mod: {}", package.manifest.name);
                self.refresh_mod_workspace();

                if let Some(script_path) = script_to_open {
                    self.open_mod_script(script_path);
                }
            }
            Err(err) => {
                self.mods_error = Some(err.to_string());
            }
        }
    }

    fn create_new_lua_script_for_selected_mod(&mut self) {
        let Some(index) = self.selected_mod_index else {
            self.mods_error = Some("Select a mod before adding a script.".to_owned());
            return;
        };

        let Some(package) = self.mods.get(index).cloned() else {
            self.mods_error = Some("Selected mod no longer exists. Refresh mods.".to_owned());
            return;
        };

        match create_lua_script(&package.path, &self.new_script_name) {
            Ok(script_path) => {
                self.status = format!(
                    "Created script: {}",
                    script_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                );
                self.refresh_mod_workspace();
                self.open_mod_script(script_path);
            }
            Err(err) => {
                self.mods_error = Some(err.to_string());
            }
        }
    }

    fn open_mod_script(&mut self, path: PathBuf) {
        match read_text_file(&path) {
            Ok(text) => {
                self.mod_script_path = Some(path);
                self.mod_script_text = text;
                self.mod_script_dirty = false;
                self.mod_script_window_open = true;
                self.mod_script_error = None;
            }
            Err(err) => {
                self.mod_script_error = Some(err.to_string());
            }
        }
    }

    fn save_mod_script(&mut self) {
        let Some(path) = self.mod_script_path.as_ref() else {
            return;
        };

        match write_text_file(path, &self.mod_script_text) {
            Ok(()) => {
                self.mod_script_dirty = false;
                self.mod_script_error = None;
                self.status = format!(
                    "Saved script: {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
            Err(err) => {
                self.mod_script_error = Some(err.to_string());
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
                                NodeKind::File => {
                                    match child.category.unwrap_or(AssetCategory::Unknown) {
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
                                    }
                                }
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

    fn find_file_node<'a>(nodes: &'a [FileNode], path: &Path) -> Option<&'a FileNode> {
        for node in nodes {
            if node.path == path {
                return Some(node);
            }

            if node.kind == NodeKind::Folder {
                if let Some(found) = Self::find_file_node(&node.children, path) {
                    return Some(found);
                }
            }
        }

        None
    }

    fn content_browser_entries(&self) -> Vec<FileNode> {
        let Some(folder) = self.content_browser_folder.as_ref() else {
            return self.tree.clone();
        };

        Self::find_file_node(&self.tree, folder)
            .map(|node| node.children.clone())
            .unwrap_or_else(|| self.tree.clone())
    }

    fn content_browser_path_label(&self) -> String {
        let Some(folder) = self.content_browser_folder.as_ref() else {
            return "Data".to_owned();
        };

        let Some(root) = self.game_root.as_ref() else {
            return folder.display().to_string();
        };

        folder
            .strip_prefix(root.join("Data"))
            .map(|path| {
                let suffix = path.display().to_string();
                if suffix.is_empty() {
                    "Data".to_owned()
                } else {
                    format!("Data\\{}", suffix)
                }
            })
            .unwrap_or_else(|_| folder.display().to_string())
    }

    fn asset_category_display_order() -> [AssetCategory; 11] {
        [
            AssetCategory::Scene,
            AssetCategory::Model,
            AssetCategory::Animation,
            AssetCategory::Texture,
            AssetCategory::AudioStream,
            AssetCategory::AudioBank,
            AssetCategory::Video,
            AssetCategory::Lighting,
            AssetCategory::Particle,
            AssetCategory::Log,
            AssetCategory::Unknown,
        ]
    }

    fn asset_category_sort_order(category: AssetCategory) -> u8 {
        match category {
            AssetCategory::Scene => 0,
            AssetCategory::Model => 1,
            AssetCategory::Animation => 2,
            AssetCategory::Texture => 3,
            AssetCategory::AudioStream => 4,
            AssetCategory::AudioBank => 5,
            AssetCategory::Video => 6,
            AssetCategory::Lighting => 7,
            AssetCategory::Particle => 8,
            AssetCategory::Log => 9,
            AssetCategory::Unknown => 10,
        }
    }

    fn asset_category_browser_title(category: AssetCategory) -> &'static str {
        match category {
            AssetCategory::Scene => "Levels / Scenes",
            AssetCategory::Model => "Models",
            AssetCategory::Animation => "Animations",
            AssetCategory::Texture => "Textures",
            AssetCategory::AudioStream => "Audio Files",
            AssetCategory::AudioBank => "Audio Banks",
            AssetCategory::Video => "Videos",
            AssetCategory::Lighting => "Lighting",
            AssetCategory::Particle => "Particles",
            AssetCategory::Log => "Logs",
            AssetCategory::Unknown => "Other Files",
        }
    }

    fn asset_badge(category: Option<AssetCategory>) -> &'static str {
        match category.unwrap_or(AssetCategory::Unknown) {
            AssetCategory::Texture => "DDS",
            AssetCategory::Model => "GEO",
            AssetCategory::Animation => "ANM",
            AssetCategory::Particle => "FX",
            AssetCategory::AudioStream => "AUD",
            AssetCategory::AudioBank => "BNK",
            AssetCategory::Video => "BIK",
            AssetCategory::Lighting => "LGT",
            AssetCategory::Scene => "SCN",
            AssetCategory::Log => "LOG",
            AssetCategory::Unknown => "FILE",
        }
    }

    fn show_content_browser(&mut self, ui: &mut egui::Ui) {
        ui.set_min_height(280.0);

        ui.horizontal(|ui| {
            ui.heading("Content Browser");
            ui.separator();
            ui.label(self.content_browser_path_label());

            if self.content_browser_folder.is_some() && ui.button("Data").clicked() {
                self.content_browser_folder = None;
            }

            if ui.button("Refresh").clicked() {
                self.rescan(ui.ctx());
            }
        });

        ui.separator();

        if self.tree.is_empty() {
            ui.label("Open the game folder to browse Data assets.");
            return;
        }

        let entries = self.content_browser_entries();
        let mut folders = entries
            .iter()
            .filter(|node| node.kind == NodeKind::Folder)
            .cloned()
            .collect::<Vec<_>>();
        let mut files = entries
            .into_iter()
            .filter(|node| node.kind == NodeKind::File)
            .collect::<Vec<_>>();

        folders.sort_by_key(|node| node.name.to_ascii_lowercase());
        files.sort_by_key(|node| {
            (
                Self::asset_category_sort_order(node.category.unwrap_or(AssetCategory::Unknown)),
                node.name.to_ascii_lowercase(),
            )
        });

        let body_height = ui.available_height().max(220.0);
        let body_width = ui.available_width();

        ui.allocate_ui_with_layout(
            egui::vec2(body_width, body_height),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(220.0, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.label("Folders");
                        egui::ScrollArea::vertical()
                            .id_salt("content_browser_folders")
                            .max_height((body_height - 26.0).max(180.0))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(200.0);
                                ui.vertical(|ui| {
                                    if let Some(current) = self.content_browser_folder.clone() {
                                        if let Some(parent) = current.parent() {
                                            let data_root = self.game_root.as_ref().map(|root| root.join("Data"));
                                            if data_root.as_ref() == Some(&current) {
                                                self.content_browser_folder = None;
                                            } else if ui.button(".. Data").clicked() {
                                                if data_root.as_ref() == Some(&parent.to_path_buf()) {
                                                    self.content_browser_folder = None;
                                                } else {
                                                    self.content_browser_folder = Some(parent.to_path_buf());
                                                }
                                            }
                                        }
                                    }

                                    for folder in &folders {
                                        if ui
                                            .add_sized(
                                                [ui.available_width(), 22.0],
                                                egui::Button::new(format!("[DIR] {}", folder.name)),
                                            )
                                            .clicked()
                                        {
                                            self.content_browser_folder = Some(folder.path.clone());
                                        }
                                    }
                                });
                            });
                    },
                );

                ui.separator();

                let asset_width = ui.available_width().max(260.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(asset_width, body_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("Assets ({})", files.len()));
                            ui.separator();
                            ui.small("Grouped by file type. All non-SCN files open in their own windows.");
                        });

                        egui::ScrollArea::vertical()
                            .id_salt("content_browser_assets")
                            .max_height((body_height - 26.0).max(180.0))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for category in Self::asset_category_display_order() {
                                    let category_files = files
                                        .iter()
                                        .filter(|file| file.category.unwrap_or(AssetCategory::Unknown) == category)
                                        .collect::<Vec<_>>();

                                    if category_files.is_empty() {
                                        continue;
                                    }

                                    let default_open = false;

                                    egui::CollapsingHeader::new(format!(
                                        "{} ({})",
                                        Self::asset_category_browser_title(category),
                                        category_files.len()
                                    ))
                                    .id_salt(format!("content_category_{category:?}"))
                                    .default_open(default_open)
                                    .show(ui, |ui| {
                                        let card_width = 154.0;
                                        let columns = ((ui.available_width() / card_width).floor() as usize).max(1);

                                        egui::Grid::new(format!("content_browser_asset_grid_{category:?}"))
                                            .num_columns(columns)
                                            .min_col_width(card_width)
                                            .spacing([8.0, 8.0])
                                            .show(ui, |ui| {
                                                for (index, file) in category_files.iter().enumerate() {
                                                    let selected = self.selected_file.as_ref() == Some(&file.path);
                                                    let frame = egui::Frame::group(ui.style()).fill(if selected {
                                                        egui::Color32::from_rgb(28, 54, 84)
                                                    } else {
                                                        egui::Color32::from_rgb(28, 28, 28)
                                                    });

                                                    frame.show(ui, |ui| {
                                                        ui.set_min_size(egui::vec2(card_width - 16.0, 64.0));
                                                        ui.vertical_centered(|ui| {
                                                            ui.label(
                                                                egui::RichText::new(Self::asset_badge(file.category))
                                                                    .monospace()
                                                                    .strong(),
                                                            );

                                                            let clicked = ui
                                                                .selectable_label(selected, &file.name)
                                                                .on_hover_text(file.path.display().to_string())
                                                                .clicked();

                                                            if clicked {
                                                                self.open_content_browser_asset(ui.ctx(), file);
                                                            }

                                                            if let Some(size) = file.size {
                                                                ui.small(format!("{} KB", (size + 1023) / 1024));
                                                            }
                                                        });
                                                    });

                                                    if (index + 1) % columns == 0 {
                                                        ui.end_row();
                                                    }
                                                }
                                            });
                                    });
                                }
                            });
                    },
                );
            },
        );
    }

    fn stop_bik_audio(&mut self) {
        if self.bik_audio_active {
            if let Some(player) = self.audio_player.as_ref() {
                player.stop();
            }
        }

        self.bik_audio_active = false;
    }

    fn start_bik_audio_load_job(&mut self, ctx: &egui::Context, path: PathBuf) {
        let title = Self::loading_title("Loading audio for", &path);
        self.start_loading_job(ctx, LoadingJobKind::LoadBikAudio(path.clone()), title, move |id, tx| {
            Self::send_loading_progress(&tx, id, "Extracting BIK audio", None);
            let result = match extract_bik_audio_wav(&path) {
                Ok(wav_bytes) => LoadingJobResult::BikAudioLoaded { path, wav_bytes },
                Err(err) => LoadingJobResult::Error(err.to_string()),
            };
            Self::send_loading_finished(tx, id, result);
        });
    }

    fn ensure_bik_audio_loaded(&mut self, ctx: &egui::Context, start_seconds: f32) -> bool {
        let Some(preview) = self.bik_preview.as_ref() else {
            return false;
        };

        if !preview.has_audio {
            return false;
        }

        let path = preview.path.clone();

        if self.bik_audio_path.as_ref() == Some(&path) {
            if self.bik_audio_wav.is_some() {
                return true;
            }
            self.pending_bik_audio_start_seconds = Some(start_seconds);
            if !self.is_loading_bik_audio_path(&path) {
                self.start_bik_audio_load_job(ctx, path);
            }
            return false;
        }

        self.bik_audio_path = Some(path.clone());
        self.bik_audio_wav = None;
        self.bik_audio_error = None;
        self.pending_bik_audio_start_seconds = Some(start_seconds);

        if !self.is_loading_bik_audio_path(&path) {
            self.start_bik_audio_load_job(ctx, path);
        }

        false
    }

    fn start_bik_audio(&mut self, ctx: &egui::Context, start_seconds: f32) {
        if !self.ensure_bik_audio_loaded(ctx, start_seconds) {
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
                    self.active_audio_window = None;
                    self.bik_audio_error = None;
                    self.bik_audio_active = true;
                    self.pending_bik_audio_start_seconds = None;
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
        self.pending_bik_audio_start_seconds = None;
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

            self.bik_texture = Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR));
        }
    }

    fn start_bik_preview_job(&mut self, ctx: &egui::Context, path: PathBuf) {
        let title = Self::loading_title("Opening", &path);
        self.start_loading_job(ctx, LoadingJobKind::OpenBikPreview(path.clone()), title, move |id, tx| {
            Self::send_loading_progress(&tx, id, "Reading BIK metadata and first frame", None);
            let result = match load_bik_preview(&path) {
                Ok(preview) => LoadingJobResult::BikPreviewLoaded { path, preview },
                Err(err) => LoadingJobResult::Error(err.to_string()),
            };
            Self::send_loading_finished(tx, id, result);
        });
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

        if self.bik_preview_path.as_ref() == Some(&path) || self.is_loading_bik_preview_path(&path) {
            return;
        }

        self.reset_bik_state();
        self.bik_preview_path = Some(path.clone());
        self.start_bik_preview_job(ctx, path);
    }

    fn start_bik_playback(&mut self, ctx: &egui::Context) {
        let Some((preview_path, estimated_total, first_frame)) =
            self.bik_preview.as_ref().map(|preview| {
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
        self.start_bik_audio(ctx, self.bik_current_time_seconds);
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

        let Some((total, fps, first_frame)) = self.bik_preview.as_ref().map(|preview| {
            (
                preview.total_duration_seconds(),
                preview.fps.max(0.001),
                preview.first_frame.clone(),
            )
        }) else {
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
            self.bik_clock_start_secs + Instant::now().duration_since(started_at).as_secs_f32()
        };

        while let Some((_, time_seconds, _)) = self.bik_frame_queue.front() {
            if *time_seconds <= target_time {
                let (frame_index, time_seconds, image) = self.bik_frame_queue.pop_front().unwrap();
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

    fn open_texture_preview_window(&mut self, title: String, preview: DdsPreview) {
        let id = self.next_texture_preview_window_id;
        self.next_texture_preview_window_id = self.next_texture_preview_window_id.wrapping_add(1);

        self.texture_preview_windows.push(TexturePreviewWindow {
            id,
            title,
            preview,
            zoom: 1.0,
            open: true,
        });
    }

    fn open_texture_path_preview_window(&mut self, ctx: &egui::Context, path: &Path) {
        let title = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        match load_dds_preview(ctx, path) {
            Ok(preview) => {
                self.open_texture_preview_window(title, preview);
                self.status = format!("Opened texture preview: {}", path.display());
            }
            Err(err) => {
                self.status = format!("Texture preview error: {}", err);
            }
        }
    }

    fn path_title(path: &Path) -> String {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string())
    }

    fn open_audio_preview_window(&mut self, path: &Path) {
        if let Some(window) = self
            .audio_preview_windows
            .iter_mut()
            .find(|window| window.path.as_path() == path)
        {
            window.open = true;
            self.status = format!("Opened audio preview: {}", path.display());
            return;
        }

        let id = self.next_audio_preview_window_id;
        self.next_audio_preview_window_id = self.next_audio_preview_window_id.wrapping_add(1);

        self.audio_preview_windows.push(AudioPreviewWindow {
            id,
            title: format!("Audio - {}", Self::path_title(path)),
            path: path.to_path_buf(),
            audio_player: None,
            audio_error: None,
            open: true,
        });
        self.status = format!("Opened audio preview: {}", path.display());
    }

    fn open_bnk_preview_window(&mut self, path: &Path) {
        if let Some(window) = self
            .bnk_preview_windows
            .iter_mut()
            .find(|window| window.path.as_path() == path)
        {
            window.open = true;
            self.status = format!("Opened BNK preview: {}", path.display());
            return;
        }

        let (bnk_file, selected_entry, error) = match load_bnk(path) {
            Ok(bnk) => {
                let selected_entry = if bnk.entries.is_empty() {
                    None
                } else {
                    Some(0)
                };
                (Some(bnk), selected_entry, None)
            }
            Err(err) => (None, None, Some(err.to_string())),
        };

        let id = self.next_bnk_preview_window_id;
        self.next_bnk_preview_window_id = self.next_bnk_preview_window_id.wrapping_add(1);

        self.bnk_preview_windows.push(BnkPreviewWindow {
            id,
            title: format!("Audio Bank - {}", Self::path_title(path)),
            path: path.to_path_buf(),
            bnk_file,
            error,
            audio_player: None,
            audio_error: None,
            selected_entry,
            open: true,
        });
        self.status = format!("Opened BNK preview: {}", path.display());
    }

    fn preview_text_for_file(path: &Path) -> (Option<String>, Option<String>) {
        const MAX_PREVIEW_BYTES: usize = 4096;

        let data = match fs::read(path) {
            Ok(data) => data,
            Err(err) => return (None, Some(err.to_string())),
        };

        if data.is_empty() {
            return (Some("(empty file)".to_owned()), None);
        }

        let preview_len = data.len().min(MAX_PREVIEW_BYTES);
        let sample = &data[..preview_len];
        let looks_text = sample
            .iter()
            .all(|byte| matches!(*byte, b'\n' | b'\r' | b'\t') || (*byte >= 0x20 && *byte < 0x7F));

        if looks_text {
            let mut text = String::from_utf8_lossy(sample).to_string();
            if data.len() > preview_len {
                text.push_str("\n\n... preview truncated ...");
            }
            return (Some(text), None);
        }

        let mut text = String::new();
        for (row, chunk) in sample.chunks(16).enumerate() {
            text.push_str(&format!("{:08X}: ", row * 16));
            for byte in chunk {
                text.push_str(&format!("{:02X} ", byte));
            }
            for _ in chunk.len()..16 {
                text.push_str("   ");
            }
            text.push_str("  ");
            for byte in chunk {
                let ch = if *byte >= 0x20 && *byte < 0x7F {
                    *byte as char
                } else {
                    '.'
                };
                text.push(ch);
            }
            text.push('\n');
        }

        if data.len() > preview_len {
            text.push_str("\n... preview truncated ...\n");
        }

        (Some(text), None)
    }

    fn open_file_preview_window(&mut self, path: &Path) {
        if let Some(window) = self
            .file_preview_windows
            .iter_mut()
            .find(|window| window.path.as_path() == path)
        {
            window.open = true;
            self.status = format!("Opened file preview: {}", path.display());
            return;
        }

        let id = self.next_file_preview_window_id;
        self.next_file_preview_window_id = self.next_file_preview_window_id.wrapping_add(1);
        let (preview_text, preview_error) = Self::preview_text_for_file(path);

        self.file_preview_windows.push(FilePreviewWindow {
            id,
            title: format!("File - {}", Self::path_title(path)),
            path: path.to_path_buf(),
            open: true,
            preview_text,
            preview_error,
        });
        self.status = format!("Opened file preview: {}", path.display());
    }

    fn should_open_in_main_preview(path: &Path, category: AssetCategory) -> bool {
        category == AssetCategory::Scene
            || path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("scn"))
                .unwrap_or(false)
    }

    fn open_content_browser_asset(&mut self, ctx: &egui::Context, file: &FileNode) {
        let category = file.category.unwrap_or_else(|| classify_path(&file.path));

        if Self::should_open_in_main_preview(&file.path, category) {
            self.selected_file = Some(file.path.clone());
            return;
        }

        self.open_asset_preview_window(ctx, &file.path, category);
    }

    fn open_asset_preview_window(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        category: AssetCategory,
    ) {
        match category {
            AssetCategory::Texture => self.open_texture_path_preview_window(ctx, path),
            AssetCategory::AudioStream => self.open_audio_preview_window(path),
            AssetCategory::AudioBank => self.open_bnk_preview_window(path),
            AssetCategory::Model => self.open_geo_preview_window(ctx, path),
            AssetCategory::Scene => self.selected_file = Some(path.to_path_buf()),
            _ => self.open_file_preview_window(path),
        }
    }

    fn model_texture_refs_for_geo(&self, geo_path: &Path, geo: &GeoFile) -> Vec<ModelTextureRef> {
        if let Some(texture_refs) = self.asset_links.model_to_textures.get(geo_path) {
            return texture_refs.clone();
        }

        geo.texture_names
            .iter()
            .map(|name| ModelTextureRef {
                name: name.clone(),
                resolved_path: Self::guess_geo_texture_path(geo_path, name),
            })
            .collect()
    }

    fn open_geo_preview_window(&mut self, ctx: &egui::Context, path: &Path) {
        if let Some(window) = self
            .geo_preview_windows
            .iter_mut()
            .find(|window| window.path.as_path() == path)
        {
            window.open = true;
            self.status = format!("Opened GEO preview: {}", path.display());
            return;
        }

        let id = self.next_geo_preview_window_id;
        self.next_geo_preview_window_id = self.next_geo_preview_window_id.wrapping_add(1);

        let mut viewer = GeoViewerState::with_target_format(self.gpu_target_format);
        let mut geo_file = None;
        let mut error = None;
        let mut material_previews = Vec::new();
        let mut material_errors = Vec::new();
        let mut texture_refs = Vec::new();
        let mut animation_groups = Vec::new();

        match load_geo(path) {
            Ok(geo) => {
                reset_geo_viewer(&mut viewer, &geo);

                for texture_name in &geo.texture_names {
                    let preview = if let Some(tex_path) =
                        Self::guess_geo_texture_path(path, texture_name)
                    {
                        match load_dds_preview(ctx, &tex_path) {
                            Ok(preview) => Some(preview),
                            Err(err) => {
                                material_errors.push(format!("{}: {}", tex_path.display(), err));
                                None
                            }
                        }
                    } else {
                        material_errors.push(format!("{}: missing", texture_name));
                        None
                    };

                    material_previews.push(preview);
                }

                texture_refs = self.model_texture_refs_for_geo(path, &geo);

                if geo.skeleton.is_some()
                    && matches!(
                        geo.asset_type,
                        GeoAssetType::SkinnedMesh | GeoAssetType::RigidProp
                    )
                {
                    animation_groups = self.animations_for_geo_grouped(path);
                }

                geo_file = Some(geo);
            }
            Err(err) => {
                error = Some(err.to_string());
            }
        }

        self.geo_preview_windows.push(GeoPreviewWindow {
            id,
            title: format!("Model - {}", Self::path_title(path)),
            path: path.to_path_buf(),
            geo_file,
            error,
            material_previews,
            material_error: if material_errors.is_empty() {
                None
            } else {
                Some(material_errors.join(" | "))
            },
            texture_refs,
            animation_groups,
            viewer,
            viewer_height: 420.0,
            active_animation: None,
            active_animation_file: None,
            active_animation_error: None,
            active_animation_playing: false,
            active_animation_loop: true,
            active_animation_time: 0.0,
            active_animation_speed: 1.0,
            open: true,
        });

        self.status = format!("Opened GEO preview: {}", path.display());
    }

    fn scn_preview_geo_path_for_stem(&self, stem: &str) -> Option<PathBuf> {
        let key = stem
            .trim()
            .trim_end_matches(".geo")
            .to_ascii_lowercase();

        if key.is_empty() {
            return None;
        }

        self.scn_scene_models
            .iter()
            .find(|model| {
                model.archetype.eq_ignore_ascii_case(&key)
                    || model
                        .path
                        .file_stem()
                        .and_then(|name| name.to_str())
                        .map(|name| name.eq_ignore_ascii_case(&key))
                        .unwrap_or(false)
            })
            .map(|model| model.path.clone())
    }

    fn set_geo_window_animation(window: &mut GeoPreviewWindow, path: PathBuf) {
        window.active_animation = Some(path.clone());
        window.active_animation_file = None;
        window.active_animation_error = None;
        window.active_animation_time = 0.0;
        window.active_animation_playing = false;

        match load_anm(&path) {
            Ok(anm) => {
                window.active_animation_playing = anm.rigid_clip.is_some();
                window.active_animation_file = Some(anm);
            }
            Err(err) => {
                window.active_animation_error = Some(err.to_string());
            }
        }
    }

    fn update_geo_preview_window_animation_clock(
        ctx: &egui::Context,
        window: &mut GeoPreviewWindow,
    ) {
        if !window.active_animation_playing {
            return;
        }

        let Some(anm) = window.active_animation_file.as_ref() else {
            return;
        };

        let Some(clip) = anm.rigid_clip.as_ref() else {
            return;
        };

        let dt = ctx.input(|i| i.unstable_dt).max(1.0 / 240.0);
        window.active_animation_time += dt * window.active_animation_speed.max(0.05);

        let duration = clip.duration_seconds.max(1.0 / clip.sample_rate.max(1.0));

        if window.active_animation_time > duration {
            if window.active_animation_loop {
                window.active_animation_time %= duration;
            } else {
                window.active_animation_time = duration;
                window.active_animation_playing = false;
            }
        }

        ctx.request_repaint();
    }

    fn show_geo_model_and_subset_panel(
        ui: &mut egui::Ui,
        geo: &GeoFile,
        window_path: &Path,
        texture_refs: &[ModelTextureRef],
        pending_texture_preview_paths: &mut Vec<PathBuf>,
    ) {
        ui.heading("Model");
        ui.separator();
        ui.label(Self::path_title(window_path));
        ui.small(format!("Type: {}", geo.asset_type.as_str()));
        ui.small(format!(
            "Vertices: {}  |  Faces: {}  |  Subsets: {}",
            geo.vertex_count,
            geo.faces.len(),
            geo.subsets.len()
        ));

        ui.separator();
        ui.heading("Textures");
        ui.separator();

        if texture_refs.is_empty() {
            ui.label("(none found)");
        } else {
            egui::ScrollArea::vertical()
                .id_salt(format!("geo_window_textures:{}", window_path.display()))
                .max_height(120.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for tex in texture_refs {
                        match &tex.resolved_path {
                            Some(path) => {
                                if ui.button(&tex.name).clicked() {
                                    pending_texture_preview_paths.push(path.clone());
                                }
                            }
                            None => {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    format!("{} (missing)", tex.name),
                                );
                            }
                        }
                    }
                });
        }

        ui.separator();
        ui.heading("Subsets");
        ui.separator();

        if geo.subsets.is_empty() {
            ui.label("(none found)");
        } else {
            egui::ScrollArea::vertical()
                .id_salt(format!("geo_window_subsets:{}", window_path.display()))
                .max_height(220.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, subset) in geo.subsets.iter().enumerate() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!(
                                "#{:02}  material={}  flags={}  start={}  count={}",
                                i, subset.material, subset.flags, subset.start, subset.count
                            ));

                            if let Some(tex_name) = geo.texture_names.get(subset.material) {
                                ui.label(" -> ");

                                if let Some(tex_ref) =
                                    texture_refs.iter().find(|t| t.name == *tex_name)
                                {
                                    match &tex_ref.resolved_path {
                                        Some(path) => {
                                            if ui.small_button(tex_name).clicked() {
                                                pending_texture_preview_paths.push(path.clone());
                                            }
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
                            }
                        });
                    }
                });
        }
    }

    fn show_geo_skeleton_panel(ui: &mut egui::Ui, geo: &GeoFile, window_path: &Path) {
        ui.heading("Skeleton / Bones");
        ui.separator();

        if let Some(skeleton) = &geo.skeleton {
            ui.label(format!("Bone count: {}", skeleton.bone_count));
            ui.separator();

            egui::ScrollArea::vertical()
                .id_salt(format!("geo_window_bones:{}", window_path.display()))
                .max_height(390.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, name) in skeleton.names.iter().enumerate() {
                        let parent_text = match skeleton.parent.get(i).and_then(|p| *p) {
                            Some(parent) => parent.to_string(),
                            None => "-".to_owned(),
                        };

                        ui.label(format!("#{:03}  parent={}  {}", i, parent_text, name));
                    }
                });
        } else {
            ui.label("No skeleton detected.");
        }
    }

    fn show_geo_animation_panel(
        ui: &mut egui::Ui,
        window: &mut GeoPreviewWindow,
        animation_groups: &[(String, Vec<PathBuf>)],
        active_animation: &Option<PathBuf>,
        geo_stem: &str,
    ) -> Option<PathBuf> {
        let mut pending_animation = None;

        ui.heading("Animations");
        ui.separator();

        if let Some(anm_path) = active_animation {
            let label = anm_path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| anm_path.display().to_string());
            ui.label(format!("Selected: {}", label));

            if let Some(anm) = &window.active_animation_file {
                ui.small(format!(
                    "Rig bones: {}  |  Duration hint: {:.2}s",
                    anm.rig_bone_count, anm.duration_hint_seconds
                ));

                if let Some(clip) = &anm.rigid_clip {
                    ui.small(format!(
                        "Rigid clip: {:.2}s at {:.1} fps",
                        clip.duration_seconds, clip.sample_rate
                    ));
                } else {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "This ANM has no decoded rigid-prop clip yet.",
                    );
                }
            }

            ui.horizontal(|ui| {
                let play_label = if window.active_animation_playing {
                    "Pause"
                } else {
                    "Play"
                };
                if ui.button(play_label).clicked() {
                    window.active_animation_playing = !window.active_animation_playing;
                }

                if ui.button("Stop").clicked() {
                    window.active_animation_playing = false;
                    window.active_animation_time = 0.0;
                }
            });

            ui.checkbox(&mut window.active_animation_loop, "Loop");
            ui.add(
                egui::Slider::new(&mut window.active_animation_speed, 0.1..=4.0)
                    .logarithmic(true)
                    .text("Speed"),
            );
            ui.label(format!("Time: {:.2}s", window.active_animation_time));
            ui.separator();
        }

        if let Some(err) = &window.active_animation_error {
            ui.colored_label(egui::Color32::RED, format!("Animation error: {}", err));
            ui.separator();
        }

        if animation_groups.is_empty() {
            ui.label("No animations found beside this GEO.");
            return pending_animation;
        }

        egui::ScrollArea::vertical()
            .id_salt(format!("geo_window_anims:{}", window.path.display()))
            .max_height(390.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (group_name, paths) in animation_groups {
                    egui::CollapsingHeader::new(group_name.as_str())
                        .id_salt(format!(
                            "geo_window_anim_group:{}:{}",
                            window.id, group_name
                        ))
                        .default_open(group_name == geo_stem)
                        .show(ui, |ui| {
                            for anm_path in paths {
                                let label = anm_path
                                    .file_name()
                                    .map(|name| name.to_string_lossy().to_string())
                                    .unwrap_or_else(|| anm_path.display().to_string());

                                let is_selected = active_animation.as_ref() == Some(anm_path);

                                if ui.selectable_label(is_selected, label).clicked() {
                                    pending_animation = Some(anm_path.clone());
                                }
                            }
                        });
                }
            });

        pending_animation
    }

    fn draw_geo_preview_windows(&mut self, ctx: &egui::Context) {
        let mut pending_texture_preview_paths = Vec::new();

        for window in &mut self.geo_preview_windows {
            Self::update_geo_preview_window_animation_clock(ctx, window);

            let mut open = window.open;
            egui::Window::new(window.title.clone())
                .id(egui::Id::new(("geo_preview_window", window.id)))
                .open(&mut open)
                .resizable(true)
                .default_size(egui::vec2(1020.0, 780.0))
                .show(ctx, |ui| {
                    ui.label(window.path.display().to_string());
                    ui.separator();

                    if let Some(err) = &window.error {
                        ui.colored_label(
                            egui::Color32::RED,
                            format!("Could not read GEO: {}", err),
                        );
                        return;
                    }

                    let Some(geo) = window.geo_file.clone() else {
                        ui.label("GEO selected, but no GEO info is loaded.");
                        return;
                    };
                    let window_path = window.path.clone();

                    let active_rigid_tag = window
                        .active_animation
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().to_string());

                    {
                        let active_rigid_clip = window
                            .active_animation_file
                            .as_ref()
                            .and_then(|anm| anm.rigid_clip.as_ref());

                        draw_geo_viewer(
                            ui,
                            &geo,
                            &window.material_previews,
                            active_rigid_clip,
                            window.active_animation_time,
                            active_rigid_tag.as_deref(),
                            &mut window.viewer,
                            window.viewer_height,
                        );
                    }

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
                        window.viewer_height = (window.viewer_height + delta).clamp(260.0, 900.0);
                        ui.ctx().request_repaint();
                    }

                    if let Some(err) = &window.material_error {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!("Texture load warning: {}", err),
                        );
                    }

                    ui.separator();

                    let texture_refs = window.texture_refs.clone();
                    let animation_groups = window.animation_groups.clone();
                    let active_animation = window.active_animation.clone();
                    let geo_stem = Self::asset_stem_lower(&window_path);
                    let mut pending_animation = None;

                    ui.columns(3, |columns| {
                        let (left_cols, rest) = columns.split_at_mut(1);
                        let left = &mut left_cols[0];
                        let (middle_cols, right_cols) = rest.split_at_mut(1);
                        let middle = &mut middle_cols[0];
                        let right = &mut right_cols[0];

                        Self::show_geo_model_and_subset_panel(
                            left,
                            &geo,
                            &window_path,
                            &texture_refs,
                            &mut pending_texture_preview_paths,
                        );
                        Self::show_geo_skeleton_panel(middle, &geo, &window_path);
                        pending_animation = Self::show_geo_animation_panel(
                            right,
                            window,
                            &animation_groups,
                            &active_animation,
                            &geo_stem,
                        );
                    });

                    if let Some(anm_path) = pending_animation {
                        Self::set_geo_window_animation(window, anm_path);
                    }
                });

            window.open = open;
        }

        self.geo_preview_windows.retain(|window| window.open);

        for path in pending_texture_preview_paths {
            self.open_texture_path_preview_window(ctx, &path);
        }
    }

    fn execute_audio_window_command(&mut self, command: AudioWindowCommand) {
        match command {
            AudioWindowCommand::PlayFile { path, source } => {
                self.play_audio_file_path(path, source)
            }
            AudioWindowCommand::PlayData {
                label,
                wav_bytes,
                source,
            } => {
                self.play_audio_bytes(label, wav_bytes, source);
            }
            AudioWindowCommand::PauseResume(source) => {
                if self.active_audio_window == Some(source) {
                    self.pause_or_resume_audio();
                }
            }
            AudioWindowCommand::Stop(source) => {
                if self.active_audio_window == Some(source) {
                    self.stop_audio();
                }
            }
            AudioWindowCommand::Seek { seconds, source } => {
                if self.active_audio_window == Some(source) {
                    self.seek_audio(seconds);
                }
            }
            AudioWindowCommand::SetVolume(volume) => {
                if let Some(player) = self.audio_player.as_ref() {
                    player.set_volume(volume);
                }
            }
        }
    }

    fn play_audio_file_path(&mut self, path: PathBuf, source: AudioWindowKey) {
        if !self.ensure_audio_player() {
            return;
        }

        if let Some(player) = self.audio_player.as_mut() {
            match player.play_file(&path) {
                Ok(()) => {
                    self.active_audio_window = Some(source);
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

    fn play_audio_bytes(&mut self, label: String, wav_bytes: Vec<u8>, source: AudioWindowKey) {
        if !self.ensure_audio_player() {
            return;
        }

        if let Some(player) = self.audio_player.as_mut() {
            match player.play_data(label.clone(), wav_bytes) {
                Ok(()) => {
                    self.active_audio_window = Some(source);
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

    fn audio_snapshot(&self) -> (bool, bool, f32, f32, Option<f32>, Option<String>) {
        Self::audio_snapshot_for(self.audio_player.as_ref())
    }

    fn audio_snapshot_for(
        player: Option<&AudioPlayer>,
    ) -> (bool, bool, f32, f32, Option<f32>, Option<String>) {
        if let Some(player) = player {
            (
                player.is_paused(),
                player.is_empty(),
                player.volume(),
                player.position().as_secs_f32(),
                player.duration().map(|duration| duration.as_secs_f32()),
                player.current_path().map(|path| path.to_owned()),
            )
        } else {
            (false, true, 1.0, 0.0, None, None)
        }
    }

    fn ensure_preview_audio_player(
        player: &mut Option<AudioPlayer>,
        audio_error: &mut Option<String>,
    ) -> bool {
        if player.is_some() {
            return true;
        }

        match AudioPlayer::new() {
            Ok(new_player) => {
                *player = Some(new_player);
                *audio_error = None;
                true
            }
            Err(err) => {
                *audio_error = Some(err.to_string());
                false
            }
        }
    }

    fn execute_preview_audio_command(
        player: &mut Option<AudioPlayer>,
        audio_error: &mut Option<String>,
        status: &mut String,
        command: AudioWindowCommand,
    ) {
        match command {
            AudioWindowCommand::PlayFile { path, .. } => {
                if !Self::ensure_preview_audio_player(player, audio_error) {
                    return;
                }

                if let Some(player) = player.as_mut() {
                    match player.play_file(&path) {
                        Ok(()) => {
                            *audio_error = None;
                            *status = format!("Playing {}", path.display());
                        }
                        Err(err) => {
                            *audio_error = Some(err.to_string());
                        }
                    }
                }
            }
            AudioWindowCommand::PlayData {
                label, wav_bytes, ..
            } => {
                if !Self::ensure_preview_audio_player(player, audio_error) {
                    return;
                }

                if let Some(player) = player.as_mut() {
                    match player.play_data(label.clone(), wav_bytes) {
                        Ok(()) => {
                            *audio_error = None;
                            *status = format!("Playing {label}");
                        }
                        Err(err) => {
                            *audio_error = Some(err.to_string());
                        }
                    }
                }
            }
            AudioWindowCommand::PauseResume(_) => {
                if let Some(player) = player.as_ref() {
                    if player.is_paused() {
                        player.resume();
                    } else {
                        player.pause();
                    }
                }
            }
            AudioWindowCommand::Stop(_) => {
                if let Some(player) = player.as_ref() {
                    player.stop();
                    *status = "Stopped audio preview.".to_owned();
                }
            }
            AudioWindowCommand::Seek { seconds, .. } => {
                if let Some(player) = player.as_ref() {
                    player.seek(Duration::from_secs_f32(seconds.max(0.0)));
                }
            }
            AudioWindowCommand::SetVolume(volume) => {
                if let Some(player) = player.as_ref() {
                    player.set_volume(volume);
                }
            }
        }
    }

    fn show_audio_transport_snapshot(
        ui: &mut egui::Ui,
        commands: &mut Vec<AudioWindowCommand>,
        source: AudioWindowKey,
        is_active: bool,
        is_paused: bool,
        is_empty: bool,
        volume: f32,
        position_secs: f32,
        duration_secs: Option<f32>,
        now_playing: &Option<String>,
    ) {
        let has_active_audio = is_active && !is_empty;

        ui.horizontal(|ui| {
            let pause_label = if has_active_audio && is_paused {
                "Resume"
            } else {
                "Pause"
            };
            if ui
                .add_enabled(has_active_audio, egui::Button::new(pause_label))
                .clicked()
            {
                commands.push(AudioWindowCommand::PauseResume(source));
            }

            if ui
                .add_enabled(has_active_audio, egui::Button::new("Stop"))
                .clicked()
            {
                commands.push(AudioWindowCommand::Stop(source));
            }

            ui.separator();

            let mut new_volume = volume;
            if ui
                .add(egui::Slider::new(&mut new_volume, 0.0..=2.0).text("Volume"))
                .changed()
            {
                commands.push(AudioWindowCommand::SetVolume(new_volume));
            }
        });

        let shown_position_secs = if has_active_audio { position_secs } else { 0.0 };
        let shown_duration_secs = if has_active_audio {
            duration_secs
        } else {
            None
        };
        let max_secs = shown_duration_secs.unwrap_or(shown_position_secs.max(1.0));
        let mut timeline_secs = shown_position_secs.min(max_secs);

        let response = ui.add_enabled(
            has_active_audio,
            egui::Slider::new(&mut timeline_secs, 0.0..=max_secs).show_value(false),
        );

        if response.changed() {
            commands.push(AudioWindowCommand::Seek {
                seconds: timeline_secs,
                source,
            });
        }

        ui.horizontal(|ui| {
            ui.label(Self::format_time(timeline_secs));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    shown_duration_secs
                        .map(Self::format_time)
                        .unwrap_or_else(|| "--:--".to_owned()),
                );
            });
        });
    }

    fn draw_audio_preview_windows(&mut self, ctx: &egui::Context) {
        let mut latest_status = None;

        for window in &mut self.audio_preview_windows {
            let mut open = window.open;
            let mut commands = Vec::new();
            let mut window_status = None;

            egui::Window::new(window.title.clone())
                .id(egui::Id::new(("audio_preview_window", window.id)))
                .open(&mut open)
                .resizable(true)
                .default_size(egui::vec2(460.0, 210.0))
                .show(ctx, |ui| {
                    ui.label(window.path.display().to_string());
                    ui.separator();

                    ui.horizontal(|ui| {
                        if ui.button("Play").clicked() {
                            commands.push(AudioWindowCommand::PlayFile {
                                path: window.path.clone(),
                                source: AudioWindowKey::Audio(window.id),
                            });
                        }
                    });

                    let (is_paused, is_empty, volume, position_secs, duration_secs, now_playing) =
                        Self::audio_snapshot_for(window.audio_player.as_ref());

                    if !is_empty && !is_paused {
                        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / 60.0));
                    }

                    let source = AudioWindowKey::Audio(window.id);
                    Self::show_audio_transport_snapshot(
                        ui,
                        &mut commands,
                        source,
                        true,
                        is_paused,
                        is_empty,
                        volume,
                        position_secs,
                        duration_secs,
                        &now_playing,
                    );

                    if let Some(err) = &window.audio_error {
                        ui.separator();
                        ui.colored_label(egui::Color32::RED, format!("Audio error: {err}"));
                    }
                });

            for command in commands {
                let mut status = String::new();
                Self::execute_preview_audio_command(
                    &mut window.audio_player,
                    &mut window.audio_error,
                    &mut status,
                    command,
                );
                if !status.is_empty() {
                    window_status = Some(status);
                }
            }

            if let Some(player) = window.audio_player.as_ref() {
                if !player.is_empty() && !player.is_paused() {
                    ctx.request_repaint_after(Duration::from_secs_f32(1.0 / 60.0));
                }
            }

            if let Some(status) = window_status {
                latest_status = Some(status);
            }

            window.open = open;
        }

        self.audio_preview_windows.retain(|window| window.open);

        if let Some(status) = latest_status {
            self.status = status;
        }
    }

    fn draw_bnk_preview_windows(&mut self, ctx: &egui::Context) {
        let mut latest_status = None;

        for window in &mut self.bnk_preview_windows {
            let mut open = window.open;
            let mut commands = Vec::new();
            let mut window_status = None;

            egui::Window::new(window.title.clone())
                .id(egui::Id::new(("bnk_preview_window", window.id)))
                .open(&mut open)
                .resizable(true)
                .default_size(egui::vec2(620.0, 520.0))
                .show(ctx, |ui| {
                    ui.label(window.path.display().to_string());
                    ui.separator();

                    if let Some(bnk) = &window.bnk_file {
                        ui.horizontal(|ui| {
                            ui.label(format!("Entries: {}", bnk.entry_count));
                            ui.separator();
                            ui.label(format!("Bank size: {} bytes", bnk.file_size));
                        });

                        if let Some(index) = window.selected_entry {
                            if let Some(entry) = bnk.entries.get(index) {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(format!("Selected: #{:03}", entry.index));
                                    ui.separator();
                                    ui.label(format!("{} Hz", entry.sample_rate));
                                    ui.separator();
                                    ui.label(format!("{} bytes", entry.byte_len));
                                    ui.separator();
                                    ui.label(format!("Format: {}", format_name(entry.format_word)));
                                    if let Some(seconds) = entry.estimated_duration_seconds() {
                                        ui.separator();
                                        ui.label(format!("{seconds:.2}s"));
                                    }
                                });
                            }
                        }

                        ui.horizontal(|ui| {
                            if ui.button("Play selected entry").clicked() {
                                if let Some(index) = window.selected_entry {
                                    let file_name = bnk
                                        .path
                                        .file_name()
                                        .and_then(|name| name.to_str())
                                        .unwrap_or("bank.bnk");
                                    let label = format!("{file_name} [entry {index:03}]");

                                    match bnk.entry_wav_bytes(index) {
                                        Ok(wav_bytes) => {
                                            window.audio_error = None;
                                            commands.push(AudioWindowCommand::PlayData {
                                                label,
                                                wav_bytes,
                                                source: AudioWindowKey::Bnk(window.id),
                                            });
                                        }
                                        Err(err) => {
                                            window.audio_error = Some(err.to_string());
                                        }
                                    }
                                } else {
                                    window.audio_error =
                                        Some("Select a BNK entry first.".to_owned());
                                }
                            }
                        });

                        let (
                            is_paused,
                            is_empty,
                            volume,
                            position_secs,
                            duration_secs,
                            now_playing,
                        ) = Self::audio_snapshot_for(window.audio_player.as_ref());

                        if !is_empty && !is_paused {
                            ctx.request_repaint_after(Duration::from_secs_f32(1.0 / 60.0));
                        }

                        let source = AudioWindowKey::Bnk(window.id);
                        Self::show_audio_transport_snapshot(
                            ui,
                            &mut commands,
                            source,
                            true,
                            is_paused,
                            is_empty,
                            volume,
                            position_secs,
                            duration_secs,
                            &now_playing,
                        );

                        ui.separator();
                        ui.heading(format!("Entries ({})", bnk.entry_count));
                        ui.separator();

                        egui::ScrollArea::vertical()
                            .id_salt(format!("bnk_window_entries:{}", window.id))
                            .max_height(260.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for entry in &bnk.entries {
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

                                    let is_selected = window.selected_entry == Some(entry.index);

                                    if ui.selectable_label(is_selected, label).clicked() {
                                        window.selected_entry = Some(entry.index);
                                    }
                                }
                            });
                    } else if let Some(err) = &window.error {
                        ui.colored_label(egui::Color32::RED, format!("Could not read BNK: {err}"));
                    } else {
                        ui.label("BNK selected, but no bank info is loaded.");
                    }

                    if let Some(err) = &window.audio_error {
                        ui.separator();
                        ui.colored_label(egui::Color32::RED, format!("Audio error: {err}"));
                    }
                });

            for command in commands {
                let mut status = String::new();
                Self::execute_preview_audio_command(
                    &mut window.audio_player,
                    &mut window.audio_error,
                    &mut status,
                    command,
                );
                if !status.is_empty() {
                    window_status = Some(status);
                }
            }

            if let Some(player) = window.audio_player.as_ref() {
                if !player.is_empty() && !player.is_paused() {
                    ctx.request_repaint_after(Duration::from_secs_f32(1.0 / 60.0));
                }
            }

            if let Some(status) = window_status {
                latest_status = Some(status);
            }

            window.open = open;
        }

        self.bnk_preview_windows.retain(|window| window.open);

        if let Some(status) = latest_status {
            self.status = status;
        }
    }

    fn draw_file_preview_windows(&mut self, ctx: &egui::Context) {
        for window in &mut self.file_preview_windows {
            let mut open = window.open;
            egui::Window::new(window.title.clone())
                .id(egui::Id::new(("file_preview_window", window.id)))
                .open(&mut open)
                .resizable(true)
                .default_size(egui::vec2(560.0, 420.0))
                .show(ctx, |ui| {
                    ui.label(window.path.display().to_string());
                    ui.separator();

                    let ext = window
                        .path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .unwrap_or("(none)");
                    let category = classify_path(&window.path);
                    let size = fs::metadata(&window.path)
                        .ok()
                        .map(|m| m.len())
                        .unwrap_or(0);

                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Extension: {ext}"));
                        ui.separator();
                        ui.label(format!("Category: {}", category_name(category)));
                        ui.separator();
                        ui.label(format!("Size: {size} bytes"));
                    });

                    ui.separator();
                    ui.heading("Preview");
                    ui.separator();

                    if let Some(err) = &window.preview_error {
                        ui.colored_label(egui::Color32::RED, format!("Preview error: {err}"));
                    } else if let Some(text) = &window.preview_text {
                        egui::ScrollArea::both()
                            .id_salt(format!("file_window_preview:{}", window.id))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.monospace(text.as_str());
                            });
                    } else {
                        ui.label("No preview available for this file yet.");
                    }
                });

            window.open = open;
        }

        self.file_preview_windows.retain(|window| window.open);
    }

    fn draw_texture_preview_windows(&mut self, ctx: &egui::Context) {
        for window in &mut self.texture_preview_windows {
            let mut open = window.open;
            egui::Window::new(window.title.clone())
                .id(egui::Id::new(("texture_preview_window", window.id)))
                .open(&mut open)
                .resizable(true)
                .default_size(egui::vec2(520.0, 420.0))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} x {}  |  mipmaps {}",
                            window.preview.width, window.preview.height, window.preview.mipmaps
                        ));
                        ui.separator();
                        ui.label("Zoom");
                        ui.add(
                            egui::Slider::new(&mut window.zoom, 0.25..=8.0)
                                .logarithmic(true)
                                .text("x"),
                        );
                        if ui.button("Reset").clicked() {
                            window.zoom = 1.0;
                        }
                    });

                    ui.separator();

                    let available = ui.available_size().max(egui::vec2(120.0, 120.0));
                    let tex_size = window.preview.texture.size_vec2();
                    let fit_scale = (available.x / tex_size.x)
                        .min(available.y / tex_size.y)
                        .min(1.0);
                    let desired_size = tex_size * fit_scale.max(0.1) * window.zoom;

                    egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let response = ui.image((window.preview.texture.id(), desired_size));
                            if response.hovered() {
                                let scroll_y = ui.ctx().input(|i| i.smooth_scroll_delta.y);
                                if scroll_y.abs() > 0.0 {
                                    let zoom_factor = (1.0 + scroll_y * 0.001).clamp(0.5, 1.5);
                                    window.zoom = (window.zoom * zoom_factor).clamp(0.25, 8.0);
                                    ui.ctx().request_repaint();
                                }
                            }
                        });
                });

            window.open = open;
        }

        self.texture_preview_windows.retain(|window| window.open);
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

    fn ensure_geo_animation_groups_loaded(&mut self, geo_path: &Path) {
        if self.geo_animation_groups_path.as_deref() == Some(geo_path) {
            return;
        }

        self.geo_animation_groups = self.animations_for_geo_grouped(geo_path);
        self.geo_animation_groups_path = Some(geo_path.to_path_buf());
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

        let duration = clip.duration_seconds.max(1.0 / clip.sample_rate.max(1.0));

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

    fn ensure_scn_loaded(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_file.clone() else {
            self.scn_file = None;
            self.scn_loaded_path = None;
            self.scn_error = None;
            self.selected_scn_node = None;
            self.selected_scn_chunk = None;
            self.hidden_scn_nodes.clear();
            self.hidden_scn_chunks.clear();
            return;
        };

        let is_scn = path
            .extension()
            .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("scn"))
            .unwrap_or(false);

        if !is_scn {
            if self.scn_file.is_some() || self.scn_loaded_path.is_some() || self.scn_scene_models_path.is_some() {
                self.scn_file = None;
                self.scn_loaded_path = None;
                self.scn_error = None;
                self.reset_scn_scene_state();
            }
            return;
        }

        if self.scn_loaded_path.as_ref() == Some(&path) || self.is_loading_scene_path(&path) {
            return;
        }

        self.scn_file = None;
        self.scn_error = None;
        self.scn_loaded_path = Some(path.clone());
        self.reset_scn_scene_state();
        self.start_open_scene_job(ctx, path);
    }

    fn reset_scn_scene_state(&mut self) {
        self.scn_scene_models.clear();
        self.scn_scene_models_path = None;
        self.scn_scene_unresolved.clear();
        self.scn_marker_geo_overrides.clear();
        self.scn_scene_error = None;
        self.selected_scn_node = None;
        self.selected_scn_chunk = None;
        self.hidden_scn_nodes.clear();
        self.hidden_scn_chunks.clear();
        self.scn_viewer.reset_with_target_format(self.gpu_target_format);
        self.scn_view_height = 520.0;
        self.scn_embedded_texture_previews.clear();
    }

    fn start_open_scene_job(&mut self, ctx: &egui::Context, path: PathBuf) {
        let tree = self.tree.clone();
        let title = Self::loading_title("Opening", &path);
        self.start_loading_job(ctx, LoadingJobKind::OpenScene(path.clone()), title, move |id, tx| {
            let result = (|| -> Result<LoadingJobResult, String> {
                Self::send_loading_progress(&tx, id, "Reading SCN", Some(0.05));
                let scn = load_scn(&path).map_err(|err| err.to_string())?;

                Self::send_loading_progress(&tx, id, "Finding map models and textures", Some(0.16));
                let data = Self::load_scn_scene_data(path, scn, tree, id, &tx)?;
                Ok(LoadingJobResult::SceneLoaded(data))
            })();

            let result = match result {
                Ok(result) => result,
                Err(err) => LoadingJobResult::Error(err),
            };
            Self::send_loading_finished(tx, id, result);
        });
    }

    fn load_scn_scene_data(
        path: PathBuf,
        scn: ScnFile,
        tree: Vec<FileNode>,
        job_id: u64,
        tx: &mpsc::Sender<LoadingJobMessage>,
    ) -> Result<LoadedSceneData, String> {
        let mut texture_nodes = Vec::new();
        let mut model_nodes = Vec::new();

        Self::collect_files_by_category(&tree, AssetCategory::Texture, &mut texture_nodes);
        Self::collect_files_by_category(&tree, AssetCategory::Model, &mut model_nodes);

        let scn_parent = path.parent().map(|parent| parent.to_path_buf());
        let is_in_scn_folder = |candidate: &Path| {
            scn_parent
                .as_ref()
                .and_then(|parent| {
                    candidate
                        .parent()
                        .map(|candidate_parent| candidate_parent == parent.as_path())
                })
                .unwrap_or(false)
        };

        let mut textures_by_name = std::collections::HashMap::<String, PathBuf>::new();
        for node in texture_nodes {
            if !is_in_scn_folder(&node.path) {
                continue;
            }
            if let Some(name) = node.path.file_name().and_then(|s| s.to_str()) {
                textures_by_name.insert(name.to_ascii_lowercase(), node.path.clone());
            }
        }

        let mut geo_by_stem = std::collections::HashMap::<String, PathBuf>::new();
        for node in model_nodes {
            if !is_in_scn_folder(&node.path) {
                continue;
            }
            if let Some(stem) = node.path.file_stem().and_then(|s| s.to_str()) {
                geo_by_stem.insert(stem.to_ascii_lowercase(), node.path.clone());
            }
        }

        let mut archetypes = std::collections::BTreeSet::<String>::new();
        for node in &scn.nodes {
            let archetype = node.archetype.trim();
            if !archetype.is_empty() {
                archetypes.insert(archetype.to_ascii_lowercase());
            }
        }

        let marker_geo_overrides = Self::infer_scn_marker_geo_overrides(&scn, &geo_by_stem, &path);

        let mut model_keys = archetypes.clone();
        for stem in marker_geo_overrides.values() {
            model_keys.insert(stem.clone());
        }

        let total_models = model_keys.len().max(1);
        let mut models = Vec::new();
        let mut unresolved = Vec::new();
        let mut failed = Vec::new();

        for (index, archetype) in model_keys.into_iter().enumerate() {
            let model_progress = 0.20 + 0.48 * (index as f32 / total_models as f32);
            Self::send_loading_progress(
                tx,
                job_id,
                format!("Loading GEO {}/{}", index + 1, total_models),
                Some(model_progress),
            );

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
                                Some(tex_path) => match load_dds_raw_preview(&tex_path) {
                                    Ok(raw) => Some(LoadedDdsPreview { path: tex_path, raw }),
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

                        models.push(LoadedSceneGeoModel {
                            archetype: archetype.clone(),
                            path: model_path.clone(),
                            geo,
                            textures,
                        });
                    }
                    Err(err) => failed.push(format!("{archetype}: {err}")),
                },
                None => {
                    if archetypes.contains(&archetype) {
                        unresolved.push(archetype);
                    }
                }
            }
        }

        Self::send_loading_progress(tx, job_id, "Collecting embedded SCN textures", Some(0.75));

        let mut embedded_texture_keys = std::collections::BTreeSet::<String>::new();
        for chunk in &scn.mesh_chunks {
            for name in &chunk.texture_names {
                embedded_texture_keys.insert(name.to_ascii_lowercase());
            }
        }

        let total_embedded = embedded_texture_keys.len().max(1);
        let mut embedded_textures = std::collections::HashMap::new();

        for (index, key) in embedded_texture_keys.into_iter().enumerate() {
            let texture_progress = 0.75 + 0.15 * (index as f32 / total_embedded as f32);

            Self::send_loading_progress(
                tx,
                job_id,
                format!("Loading map texture {}/{}", index + 1, total_embedded),
                Some(texture_progress),
            );

            let Some(tex_path) = textures_by_name.get(&key).cloned() else {
                continue;
            };

            match load_dds_raw_preview(&tex_path) {
                Ok(raw) => {
                    embedded_textures.insert(key, LoadedDdsPreview { path: tex_path, raw });
                }
                Err(err) => failed.push(format!("SCN texture {}: {}", tex_path.display(), err)),
            }
        }

        Self::send_loading_progress(tx, job_id, "Preparing map preview", Some(0.92));
        models.sort_by_key(|model| model.archetype.clone());

        Ok(LoadedSceneData {
            path,
            scn,
            models,
            embedded_textures,
            unresolved,
            marker_geo_overrides,
            failed,
        })
    }

    fn loaded_dds_to_preview(ctx: &egui::Context, loaded: LoadedDdsPreview) -> DdsPreview {
        dds_preview_from_raw(ctx, loaded.path.display().to_string(), loaded.raw)
    }

    fn apply_loaded_scene(&mut self, ctx: &egui::Context, data: LoadedSceneData) {
        if self.selected_file.as_ref() != Some(&data.path) {
            return;
        }

        self.scn_file = Some(data.scn);
        self.scn_loaded_path = Some(data.path.clone());
        self.reset_scn_scene_state();
        self.scn_scene_models_path = Some(data.path.clone());
        self.scn_scene_unresolved = data.unresolved;
        self.scn_marker_geo_overrides = data.marker_geo_overrides;

        self.scn_scene_models = data
            .models
            .into_iter()
            .map(|model| SceneGeoModel {
                archetype: model.archetype,
                path: model.path,
                geo: model.geo,
                textures: model
                    .textures
                    .into_iter()
                    .map(|texture| texture.map(|loaded| Self::loaded_dds_to_preview(ctx, loaded)))
                    .collect(),
            })
            .collect();

        self.scn_embedded_texture_previews = data
            .embedded_textures
            .into_iter()
            .map(|(key, loaded)| (key, Self::loaded_dds_to_preview(ctx, loaded)))
            .collect();

        if !data.failed.is_empty() {
            self.scn_scene_error = Some(format!(
                "Failed to load {} assets: {}",
                data.failed.len(),
                data.failed.join(" | ")
            ));
        }

        if let Some(scn) = self.scn_file.as_ref() {
            reset_scene_viewer(&mut self.scn_viewer, scn);
        }
        self.status = format!(
            "Opened map: {}",
            data.path.file_name().unwrap_or_default().to_string_lossy()
        );
        ctx.request_repaint();
    }

    fn ensure_scn_scene_loaded(&mut self, _ctx: &egui::Context) {
        // SCN scene loading is now handled by start_open_scene_job/apply_loaded_scene.
        // This function remains as a compatibility hook for the central ensure_* pipeline.
    }

    fn existing_geo_stem(
        geo_by_stem: &std::collections::HashMap<String, PathBuf>,
        stem: &str,
    ) -> Option<String> {
        let key = stem.trim().to_ascii_lowercase();
        geo_by_stem.contains_key(&key).then_some(key)
    }

    fn trailing_number(value: &str) -> Option<usize> {
        let digits_rev: String = value
            .chars()
            .rev()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();

        if digits_rev.is_empty() {
            return None;
        }

        digits_rev.chars().rev().collect::<String>().parse().ok()
    }

    fn strip_trailing_number(value: &str) -> &str {
        value
            .trim_end_matches(|ch: char| ch.is_ascii_digit())
            .trim_end_matches(|ch: char| ch == '_' || ch == '-' || ch == ' ')
    }

    fn existing_variant_by_number(
        geo_by_stem: &std::collections::HashMap<String, PathBuf>,
        number: usize,
        variants: &[&str],
        zero_based: bool,
    ) -> Option<String> {
        if variants.is_empty() {
            return None;
        }

        let index = if zero_based {
            number % variants.len()
        } else {
            number.saturating_sub(1) % variants.len()
        };

        Self::existing_geo_stem(geo_by_stem, variants[index])
    }

    fn existing_numbered_stem(
        geo_by_stem: &std::collections::HashMap<String, PathBuf>,
        prefix: &str,
        number: usize,
        variant_count: usize,
        zero_based: bool,
    ) -> Option<String> {
        if variant_count == 0 {
            return None;
        }

        let index = if zero_based {
            number % variant_count
        } else {
            number.saturating_sub(1) % variant_count
        };
        let display_number = if zero_based { index } else { index + 1 };
        let stem = format!("{prefix}{display_number:02}");
        Self::existing_geo_stem(geo_by_stem, &stem)
    }

    fn existing_marker_prefix_alias(
        geo_by_stem: &std::collections::HashMap<String, PathBuf>,
        marker_name: &str,
        aliases: &[(&str, &str)],
    ) -> Option<String> {
        aliases.iter().find_map(|(marker_prefix, geo_stem)| {
            marker_name
                .starts_with(marker_prefix)
                .then(|| Self::existing_geo_stem(geo_by_stem, geo_stem))
                .flatten()
        })
    }

    fn infer_folder_marker_geo_stem(
        folder_name: &str,
        marker_name: &str,
        geo_by_stem: &std::collections::HashMap<String, PathBuf>,
    ) -> Option<String> {
        let number = Self::trailing_number(marker_name);

        match folder_name {
            "daffyd" => {
                if marker_name == "player_start" {
                    return Self::existing_geo_stem(geo_by_stem, "dafydd");
                }
                if marker_name.starts_with("myfanwy") {
                    return Self::existing_geo_stem(geo_by_stem, "myfanwy");
                }
                if marker_name.starts_with("gay") {
                    return Self::existing_variant_by_number(
                        geo_by_stem,
                        number.unwrap_or(1),
                        &["gay_a", "gay_b", "gay_c", "gay_d", "gay_e", "gay_f"],
                        false,
                    );
                }
                if marker_name.starts_with("cyclist") {
                    return Self::existing_variant_by_number(
                        geo_by_stem,
                        number.unwrap_or(1),
                        &["cyclist", "cyclist_b", "cyclist_c"],
                        false,
                    );
                }
                if marker_name.starts_with("car_movingleft") {
                    return Self::existing_numbered_stem(
                        geo_by_stem,
                        "car",
                        number.unwrap_or(1),
                        6,
                        false,
                    );
                }
            }
            "vicky" => {
                if marker_name == "player_start" {
                    return Self::existing_geo_stem(geo_by_stem, "vicky");
                }
            }
            "divinggame" => {
                if marker_name.starts_with("loustand") {
                    return Self::existing_geo_stem(geo_by_stem, "lou");
                }
                if marker_name.starts_with("lifeguardstand") {
                    return Self::existing_geo_stem(geo_by_stem, "lifeguard");
                }
                if marker_name == "chairnode" {
                    return Self::existing_geo_stem(geo_by_stem, "andy");
                }
                if marker_name == "render_chair" {
                    return Self::existing_geo_stem(geo_by_stem, "wheelchair");
                }
                if marker_name.starts_with("board") && !marker_name.starts_with("boardlocator") {
                    return Self::existing_geo_stem(geo_by_stem, "board");
                }
            }
            "footballgame" => {
                if marker_name == "emily_start" {
                    return Self::existing_geo_stem(geo_by_stem, "emily");
                }
                if marker_name == "startposition" {
                    return Self::existing_geo_stem(geo_by_stem, "florence");
                }
                if marker_name == "goalcentre" {
                    return Self::existing_geo_stem(geo_by_stem, "goalkeeper");
                }
                if marker_name.starts_with("defender") {
                    return Self::existing_numbered_stem(
                        geo_by_stem,
                        "football_kid",
                        number.unwrap_or(0) + 1,
                        5,
                        false,
                    );
                }
                if marker_name.starts_with("targetzone") {
                    return Self::existing_geo_stem(geo_by_stem, "target");
                }
            }
            "froggame" | "pccharacterviewer" => {
                if marker_name.starts_with("letty_pos") {
                    return Self::existing_geo_stem(geo_by_stem, "letty");
                }
                if marker_name == "start_point" {
                    return Self::existing_geo_stem(geo_by_stem, "frog");
                }

                return Self::existing_marker_prefix_alias(
                    geo_by_stem,
                    marker_name,
                    &[
                        ("softfrog01a", "softfrog1a"),
                        ("softfrog01", "softfrog1"),
                        ("softfrog02", "softfrog2"),
                        ("beachball", "beachball"),
                        ("ornament01", "ornament01"),
                        ("globefrog", "globefrog"),
                        ("cushion", "cushion"),
                        ("prince", "prince"),
                        ("phone", "phone"),
                        ("clock", "clock"),
                        ("cake", "cake"),
                        ("bowl01a", "bowl01a"),
                        ("bowl01", "bowl01"),
                        ("vase02a", "vase02a"),
                        ("vase02", "vase02"),
                        ("vase01a", "vase01a"),
                        ("vase01", "vase01"),
                    ],
                );
            }
            "maggieandjudy" => {
                if marker_name.ends_with("_teacup") {
                    return Self::existing_geo_stem(geo_by_stem, "teacup");
                }
                if marker_name.ends_with("_plate") || marker_name == "plate_position" {
                    return Self::existing_geo_stem(geo_by_stem, "plate");
                }

                return Self::existing_marker_prefix_alias(
                    geo_by_stem,
                    marker_name,
                    &[
                        ("brownie", "brownie"),
                        ("maggie", "maggie"),
                        ("judy", "judy"),
                        ("vicar", "vicar"),
                        ("mp", "mp"),
                    ],
                );
            }
            "pirategame" => {
                if marker_name == "board" {
                    return Self::existing_geo_stem(geo_by_stem, "board");
                }
                if marker_name.starts_with("pirate") {
                    return Self::existing_numbered_stem(
                        geo_by_stem,
                        "pirate",
                        number.unwrap_or(0),
                        10,
                        true,
                    );
                }
                if marker_name.starts_with("line_pos") {
                    return Self::existing_numbered_stem(
                        geo_by_stem,
                        "line",
                        number.unwrap_or(0),
                        10,
                        true,
                    );
                }
            }
            _ => {}
        }

        None
    }

    fn infer_marker_geo_stem(
        marker_name: &str,
        folder_name: &str,
        geo_by_stem: &std::collections::HashMap<String, PathBuf>,
    ) -> Option<String> {
        let marker_name = marker_name.trim().to_ascii_lowercase();
        if marker_name.is_empty() {
            return None;
        }

        let is_camera_marker = marker_name.contains("camera")
            || marker_name.contains("cameratarget")
            || marker_name.ends_with("cam")
            || marker_name.contains("cam_")
            || marker_name.contains("camtarget");

        if is_camera_marker
            || marker_name.starts_with("checkpoint")
            || marker_name.starts_with("path")
            || marker_name.starts_with("lane")
            || marker_name.contains("_path")
            || marker_name.starts_with("return")
            || marker_name.starts_with("tochair")
        {
            return None;
        }

        if let Some(stem) =
            Self::infer_folder_marker_geo_stem(folder_name, &marker_name, geo_by_stem)
        {
            return Some(stem);
        }

        if let Some(stem) = Self::existing_geo_stem(geo_by_stem, &marker_name) {
            return Some(stem);
        }

        let stripped = Self::strip_trailing_number(&marker_name);
        if stripped != marker_name {
            if let Some(stem) = Self::existing_geo_stem(geo_by_stem, stripped) {
                return Some(stem);
            }
        }

        None
    }

    fn infer_scn_marker_geo_overrides(
        scn: &ScnFile,
        geo_by_stem: &std::collections::HashMap<String, PathBuf>,
        scn_path: &Path,
    ) -> std::collections::HashMap<usize, String> {
        let folder_name = scn_path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .map(|name| name.to_ascii_lowercase())
            .unwrap_or_default();

        let mut out = std::collections::HashMap::new();
        for node in scn.nodes.iter().filter(|node| node.is_marker()) {
            if let Some(stem) = Self::infer_marker_geo_stem(&node.name, &folder_name, geo_by_stem) {
                out.insert(node.index, stem);
            }
        }

        out
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
            let preview =
                if let Some(tex_path) = Self::guess_geo_texture_path(&geo_path, texture_name) {
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
                    self.active_audio_window = Some(AudioWindowKey::Main);
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
                    self.active_audio_window = Some(AudioWindowKey::Main);
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
            self.active_audio_window = None;
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
            egui::Slider::new(&mut timeline_secs, 0.0..=max_secs).show_value(false),
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

    fn scn_marker_kind(name: &str) -> &'static str {
        let lower = name.trim().to_ascii_lowercase();

        let is_camera = lower.contains("camera")
            || lower.contains("cameratarget")
            || lower.ends_with("cam")
            || lower.contains("cam_")
            || lower.contains("camtarget");

        if lower.is_empty() {
            "Unnamed marker"
        } else if lower == "player_start" {
            "Player start"
        } else if lower == "player_end" {
            "Player end"
        } else if lower.starts_with("checkpoint") {
            "Checkpoint"
        } else if lower == "startposition"
            || lower == "start_point"
            || lower.contains("spawn")
        {
            "Spawn marker"
        } else if is_camera {
            "Camera marker"
        } else if lower.starts_with("path")
            || lower.starts_with("lane")
            || lower.contains("_path")
        {
            "Route marker"
        } else if lower.starts_with("car_")
            || lower.starts_with("dec_car_")
            || lower.starts_with("car_moving")
        {
            "Traffic marker"
        } else if lower.starts_with("gay")
            || lower.starts_with("cyclist")
            || lower.starts_with("vicky")
            || lower.starts_with("dafydd")
            || lower.starts_with("myfanwy")
            || lower.starts_with("andy")
            || lower.starts_with("lou")
            || lower.starts_with("lifeguard")
            || lower.starts_with("marjorie")
            || lower.starts_with("female")
            || lower.starts_with("male")
            || lower.starts_with("letty")
            || lower.starts_with("emily")
            || lower.starts_with("florence")
            || lower.starts_with("football_kid")
            || lower.starts_with("defender")
            || lower.starts_with("pirate")
            || lower.starts_with("judy")
            || lower.starts_with("maggie")
            || lower.starts_with("brownie")
            || lower.starts_with("vicar")
            || lower == "mp"
            || lower == "character"
        {
            "Actor marker"
        } else if lower.starts_with("target")
            || lower.starts_with("targetzone")
            || lower.starts_with("board")
            || lower.starts_with("stand_table")
            || lower.starts_with("node_table")
            || lower.starts_with("line_pos")
            || lower.starts_with("return")
            || lower.starts_with("tochair")
            || lower.starts_with("easy")
            || lower.starts_with("medium")
            || lower.starts_with("hard")
            || lower.starts_with("vase")
            || lower.starts_with("cushion")
            || lower.starts_with("beachball")
            || lower.starts_with("ornament")
            || lower.starts_with("bowl")
            || lower.starts_with("softfrog")
            || lower.starts_with("globefrog")
            || lower.starts_with("prince")
            || lower.starts_with("phone")
            || lower.starts_with("clock")
            || lower.starts_with("cake")
            || lower.starts_with("chairnode")
            || lower == "render_chair"
            || lower == "plate_position"
            || lower.ends_with("_plate")
            || lower.ends_with("_teacup")
            || lower == "goalcentre"
            || lower.contains("goalline")
            || lower == "leftpost"
            || lower == "rightpost"
            || lower == "crossbar"
            || lower == "lefttouchline"
            || lower == "righttouchline"
            || lower == "celebrate_position"
            || lower == "topright"
            || lower == "bottomright"
            || lower == "bottomleft"
            || lower == "topleft"
            || lower.starts_with("frog_window")
        {
            "Gameplay target"
        } else if lower.starts_with("frog") {
            "Actor marker"
        } else {
            "Generic marker"
        }
    }

    fn summarize_scn_markers(scn: &ScnFile) -> ScnMarkerSummary {
        let mut summary = ScnMarkerSummary::default();

        for node in &scn.nodes {
            if !node.is_marker() {
                continue;
            }

            match Self::scn_marker_kind(&node.name) {
                "Actor marker" => summary.actor_like += 1,
                "Camera marker" => summary.cameras += 1,
                "Checkpoint" => summary.checkpoints += 1,
                "Gameplay target" => summary.gameplay_targets += 1,
                "Route marker" => summary.path_nodes += 1,
                "Player start" => summary.player_starts += 1,
                "Player end" => summary.player_ends += 1,
                "Spawn marker" => summary.spawns += 1,
                "Traffic marker" => summary.traffic += 1,
                _ => summary.other += 1,
            }
        }

        summary
    }

    fn show_scn_quick_summary(&self, ui: &mut egui::Ui, scn: &ScnFile) {
        let marker_summary = Self::summarize_scn_markers(scn);

        ui.heading("Level Summary");
        ui.separator();

        ui.label(format!("Nodes: {}", scn.nodes.len()));
        ui.label(format!("Renderable nodes: {}", scn.renderable_count()));
        ui.label(format!("Marker nodes: {}", scn.marker_count()));
        ui.label(format!(
            "Embedded chunks: {}",
            scn.embedded_mesh_chunk_count()
        ));
        ui.label(format!("Embedded tris: {}", scn.embedded_triangle_count()));
        ui.label(format!("Resolved GEOs: {}", self.scn_scene_models.len()));
        ui.label(format!(
            "Marker GEO previews: {}",
            self.scn_marker_geo_overrides.len()
        ));
        ui.label(format!(
            "Missing archetypes: {}",
            self.scn_scene_unresolved.len()
        ));
        ui.label(format!("Hidden nodes: {}", self.hidden_scn_nodes.len()));
        ui.label(format!("Hidden chunks: {}", self.hidden_scn_chunks.len()));

        if scn.marker_count() > 0 {
            ui.separator();
            ui.heading("Marker Groups");
            ui.separator();

            let rows = [
                ("Actor-like", marker_summary.actor_like),
                ("Player starts", marker_summary.player_starts),
                ("Player ends", marker_summary.player_ends),
                ("Spawns", marker_summary.spawns),
                ("Cameras", marker_summary.cameras),
                ("Checkpoints", marker_summary.checkpoints),
                ("Route markers", marker_summary.path_nodes),
                ("Gameplay targets", marker_summary.gameplay_targets),
                ("Traffic markers", marker_summary.traffic),
                ("Other markers", marker_summary.other),
            ];

            for (label, count) in rows {
                if count > 0 {
                    ui.label(format!("{label}: {count}"));
                }
            }
        }
    }

    fn extract_ascii_strings(data: &[u8], min_len: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut current = Vec::new();

        for &byte in data {
            if (0x20..=0x7e).contains(&byte) {
                current.push(byte);
            } else {
                if current.len() >= min_len {
                    out.push(String::from_utf8_lossy(&current).to_string());
                }
                current.clear();
            }
        }

        if current.len() >= min_len {
            out.push(String::from_utf8_lossy(&current).to_string());
        }

        out
    }

    fn extract_utf16le_strings(data: &[u8], min_len: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut current = Vec::new();

        for pair in data.chunks_exact(2) {
            let word = u16::from_le_bytes([pair[0], pair[1]]);
            if (0x20..=0x7e).contains(&word) {
                current.push(word);
            } else {
                if current.len() >= min_len {
                    out.push(String::from_utf16_lossy(&current));
                }
                current.clear();
            }
        }

        if current.len() >= min_len {
            out.push(String::from_utf16_lossy(&current));
        }

        out
    }

    fn read_lossy_text(path: &Path) -> anyhow::Result<String> {
        let bytes = fs::read(path)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn push_unique_limited(out: &mut Vec<String>, value: impl Into<String>, limit: usize) {
        if out.len() >= limit {
            return;
        }

        let value = value
            .into()
            .trim()
            .trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == ',' || ch == ';')
            .trim()
            .to_owned();

        if value.is_empty() {
            return;
        }

        if !out
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&value))
        {
            out.push(value);
        }
    }

    fn quoted_value(line: &str) -> Option<String> {
        let start = line.find('"')? + 1;
        let rest = &line[start..];
        let end = rest.find('"')?;
        Some(rest[..end].to_owned())
    }

    fn looks_like_asset_ref(value: &str) -> bool {
        let lower = value.to_ascii_lowercase();
        [
            ".scn", ".geo", ".anm", ".dds", ".bnk", ".bik", ".lgt", ".ps2", ".psf", ".ogg", ".wav",
            ".xml",
        ]
        .iter()
        .any(|ext| lower.contains(ext))
    }

    fn symbol_function_name(line: &str) -> Option<String> {
        let line = line.trim().trim_end_matches('\\').trim();
        if !(line.ends_with("();") || line.ends_with("()")) {
            return None;
        }

        let open = line.find('(')?;
        let before = line[..open].trim();
        let name = before.split_whitespace().last()?;

        let mut chars = name.chars();
        let first = chars.next()?;
        if !first.is_ascii_alphabetic() && first != '_' {
            return None;
        }

        if chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            Some(name.to_owned())
        } else {
            None
        }
    }

    fn ingest_symbol_dump(map: &mut GameCodeMap, path: &Path) -> anyhow::Result<()> {
        let text = Self::read_lossy_text(path)?;
        map.symbol_dump_path = Some(path.to_path_buf());

        let mut section = String::new();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.starts_with("// SECTION") {
                section = line.to_ascii_lowercase();
                continue;
            }

            if section.contains("rtti") {
                if let Some(start) = line.find("_(") {
                    let rest = &line[start + 2..];
                    if let Some(end) = rest.find(')') {
                        Self::push_unique_limited(&mut map.rtti_classes, &rest[..end], 300);
                    }
                }
            }

            if let Some(name) = Self::symbol_function_name(line) {
                Self::push_unique_limited(&mut map.function_names, name, 500);
            }

            let Some(value) = Self::quoted_value(line) else {
                continue;
            };

            if section.contains("resource") {
                Self::push_unique_limited(&mut map.resource_names, value.clone(), 300);
                Self::push_unique_limited(&mut map.asset_refs, value, 1500);
            } else if section.contains("source") {
                Self::push_unique_limited(&mut map.source_paths, value, 250);
            } else if section.contains("game mode") {
                Self::push_unique_limited(&mut map.game_modes, value, 100);
            } else if section.contains("lbfe") {
                Self::push_unique_limited(&mut map.frontend_functions, value, 150);
            } else if section.contains("character") {
                Self::push_unique_limited(&mut map.character_names, value, 150);
            }
        }

        Ok(())
    }

    fn ingest_game_code_token(map: &mut GameCodeMap, token: &str) {
        let cleaned = token
            .trim()
            .trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == ',' || ch == ';')
            .trim();

        if cleaned.len() < 3 {
            return;
        }

        let lower = cleaned.to_ascii_lowercase();

        Self::push_unique_limited(&mut map.raw_tokens, cleaned, 12000);

        if Self::looks_like_asset_ref(cleaned) {
            Self::push_unique_limited(&mut map.asset_refs, cleaned, 2500);
        }

        if lower.contains(".cpp")
            || lower.contains(".h")
            || lower.contains(".pdb")
            || lower.contains("::")
            || lower.contains("\\engine\\")
            || lower.contains("\\game\\")
            || lower.contains("/engine/")
            || lower.contains("/game/")
        {
            Self::push_unique_limited(&mut map.code_refs, cleaned, 1000);
        }

        if lower.contains(".cpp")
            || lower.contains(".h")
            || lower.contains(".pdb")
            || lower.contains("\\engine\\")
            || lower.contains("\\game\\")
            || lower.contains("/engine/")
            || lower.contains("/game/")
        {
            Self::push_unique_limited(&mut map.source_paths, cleaned, 400);
        }

        if lower.contains("script")
            || lower.contains("gamenode")
            || lower.contains("xmlparser")
            || lower == "anim"
            || lower == "target"
            || lower == "move"
            || lower == "wait"
            || lower == "loop"
        {
            Self::push_unique_limited(&mut map.script_tokens, cleaned, 200);
        }
    }

    fn scan_modloader_bridge(map: &mut GameCodeMap, game_root: &Path) {
        let bink_proxy_path = game_root.join("binkw32.dll");
        if bink_proxy_path.is_file() {
            map.bink_proxy_path = Some(bink_proxy_path.clone());
            if let Ok(bytes) = fs::read(&bink_proxy_path) {
                let mut bridge_tokens = Vec::new();
                for token in Self::extract_ascii_strings(&bytes, 4)
                    .into_iter()
                    .chain(Self::extract_utf16le_strings(&bytes, 4))
                {
                    let lower = token.to_ascii_lowercase();
                    if lower.contains("binkw32_real")
                        || lower.contains("modloader")
                        || lower.contains("loadlibrary")
                        || lower.contains("getprocaddress")
                    {
                        Self::push_unique_limited(&mut bridge_tokens, token, 80);
                    }
                }

                if bridge_tokens
                    .iter()
                    .any(|token| token.eq_ignore_ascii_case("binkw32_real.dll"))
                {
                    Self::push_unique_limited(
                        &mut map.injection_notes,
                        "binkw32.dll looks like a Bink proxy and references binkw32_real.dll.",
                        40,
                    );
                }

                if bridge_tokens
                    .iter()
                    .any(|token| token.to_ascii_lowercase().contains("modloader"))
                {
                    Self::push_unique_limited(
                        &mut map.injection_notes,
                        "The Bink proxy references modloader.dll.",
                        40,
                    );
                } else {
                    Self::push_unique_limited(
                        &mut map.injection_notes,
                        "The Bink proxy does not expose a modloader.dll string; modloader injection may need to be wired separately.",
                        40,
                    );
                }

                for token in bridge_tokens {
                    Self::push_unique_limited(&mut map.modloader_tokens, token, 200);
                }
            }
        }

        let real_bink_path = game_root.join("binkw32_real.dll");
        if real_bink_path.is_file() {
            map.real_bink_path = Some(real_bink_path);
        }

        let modloader_path = game_root.join("modloader.dll");
        if modloader_path.is_file() {
            map.modloader_path = Some(modloader_path.clone());
            if let Ok(bytes) = fs::read(&modloader_path) {
                for token in Self::extract_ascii_strings(&bytes, 4)
                    .into_iter()
                    .chain(Self::extract_utf16le_strings(&bytes, 4))
                {
                    let lower = token.to_ascii_lowercase();
                    if lower.contains("modloader")
                        || lower.contains("gameapi")
                        || lower.starts_with("mod_")
                        || lower.contains(".lua")
                        || lower.contains(".patch")
                        || lower.contains("hook")
                    {
                        Self::push_unique_limited(&mut map.modloader_tokens, token, 200);
                    }
                }
            }
        }
    }

    fn scan_game_code_map(game_root: &Path) -> GameCodeMap {
        let mut map = GameCodeMap::default();

        let code_root = if game_root.join("LittleBritain.exe").is_file() {
            game_root.to_path_buf()
        } else {
            game_root
                .parent()
                .filter(|parent| parent.join("LittleBritain.exe").is_file())
                .map(|parent| parent.to_path_buf())
                .unwrap_or_else(|| game_root.to_path_buf())
        };

        let exe_path = code_root.join("LittleBritain.exe");
        if exe_path.is_file() {
            map.exe_path = Some(exe_path.clone());
        } else {
            map.error = Some(format!(
                "LittleBritain.exe was not found at {}",
                exe_path.display()
            ));
        }

        let symbol_dump_path = code_root.join("COMPLETE_symbol_dump.h");
        if symbol_dump_path.is_file() {
            if let Err(err) = Self::ingest_symbol_dump(&mut map, &symbol_dump_path) {
                map.error = Some(format!(
                    "Could not parse {}: {}",
                    symbol_dump_path.display(),
                    err
                ));
            }
        }

        let strings_path = code_root.join("strings.txt");
        if strings_path.is_file() {
            match fs::read(&strings_path) {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    map.strings_path = Some(strings_path);
                    for line in text.lines() {
                        Self::ingest_game_code_token(&mut map, line);
                    }
                    for token in Self::extract_ascii_strings(&bytes, 4)
                        .into_iter()
                        .chain(Self::extract_utf16le_strings(&bytes, 4))
                    {
                        Self::ingest_game_code_token(&mut map, &token);
                    }
                }
                Err(err) => {
                    map.error = Some(format!("Could not read strings.txt: {err}"));
                }
            }
        }

        if exe_path.is_file() {
            match fs::read(&exe_path) {
                Ok(bytes) => {
                    for token in Self::extract_ascii_strings(&bytes, 4)
                        .into_iter()
                        .chain(Self::extract_utf16le_strings(&bytes, 4))
                    {
                        Self::ingest_game_code_token(&mut map, &token);
                    }
                }
                Err(err) => {
                    map.error = Some(format!("Could not scan LittleBritain.exe strings: {err}"));
                }
            }
        }

        Self::scan_modloader_bridge(&mut map, &code_root);

        map
    }

    fn asset_ref_file_name_lower(asset_ref: &str) -> Option<String> {
        let normalized = asset_ref.replace('\\', "/");
        let file_name = normalized.rsplit('/').next()?.trim();
        if file_name.is_empty() {
            None
        } else {
            Some(file_name.to_ascii_lowercase())
        }
    }

    fn code_refs_for_selected_path(path: &Path, map: &GameCodeMap) -> Vec<String> {
        let mut sibling_names = std::collections::BTreeSet::new();

        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            sibling_names.insert(name.to_ascii_lowercase());
        }

        if let Some(parent) = path.parent() {
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        sibling_names.insert(name.to_ascii_lowercase());
                    }
                }
            }
        }

        let mut out = Vec::new();
        for asset_ref in map.asset_refs.iter().chain(map.resource_names.iter()) {
            let Some(file_name) = Self::asset_ref_file_name_lower(asset_ref) else {
                continue;
            };

            if sibling_names.contains(&file_name) {
                Self::push_unique_limited(&mut out, asset_ref, 150);
            }
        }

        out
    }

    fn show_limited_code_list(ui: &mut egui::Ui, title: &str, values: &[String], limit: usize) {
        egui::CollapsingHeader::new(format!("{} ({})", title, values.len()))
            .default_open(false)
            .show(ui, |ui| {
                if values.is_empty() {
                    ui.small("None found.");
                    return;
                }

                for value in values.iter().take(limit) {
                    ui.monospace(value);
                }

                if values.len() > limit {
                    ui.small(format!("...and {} more", values.len() - limit));
                }
            });
    }

    fn show_game_code_map(ui: &mut egui::Ui, map: &GameCodeMap, selected_path: Option<&Path>) {
        egui::CollapsingHeader::new("EXE code map")
            .default_open(false)
            .show(ui, |ui| {
                ui.small(
                    "This is a reverse-engineering map from the EXE strings and symbol dump. It shows compiled-code clues, not editable source code.",
                );

                if let Some(err) = &map.error {
                    ui.colored_label(egui::Color32::YELLOW, err);
                }

                if let Some(path) = &map.exe_path {
                    ui.label(format!("EXE: {}", path.display()));
                }
                if let Some(path) = &map.symbol_dump_path {
                    ui.label(format!("Symbols: {}", path.display()));
                }
                if let Some(path) = &map.strings_path {
                    ui.label(format!("Strings: {}", path.display()));
                }
                if let Some(path) = &map.bink_proxy_path {
                    ui.label(format!("Bink proxy: {}", path.display()));
                }
                if let Some(path) = &map.real_bink_path {
                    ui.label(format!("Real Bink: {}", path.display()));
                }
                if let Some(path) = &map.modloader_path {
                    ui.label(format!("Mod loader: {}", path.display()));
                }

                ui.separator();
                ui.label(format!("RTTI classes: {}", map.rtti_classes.len()));
                ui.label(format!("Functions/patterns: {}", map.function_names.len()));
                ui.label(format!("Source/code refs: {}", map.code_refs.len()));
                ui.label(format!("Hardcoded asset refs: {}", map.asset_refs.len()));
                ui.label(format!("Script/XML tokens: {}", map.script_tokens.len()));
                ui.label(format!("Mod loader tokens: {}", map.modloader_tokens.len()));

                if !map.injection_notes.is_empty() {
                    ui.separator();
                    ui.heading("Mod-loader bridge");
                    for note in &map.injection_notes {
                        ui.small(note);
                    }
                }

                if let Some(path) = selected_path {
                    let refs = Self::code_refs_for_selected_path(path, map);
                    egui::CollapsingHeader::new(format!(
                        "Hardcoded refs matching this folder ({})",
                        refs.len()
                    ))
                    .default_open(false)
                    .show(ui, |ui| {
                        if refs.is_empty() {
                            ui.small("No exact file-name matches found in EXE strings/symbol dump.");
                        } else {
                            for value in refs.iter().take(80) {
                                ui.monospace(value);
                            }
                            if refs.len() > 80 {
                                ui.small(format!("...and {} more", refs.len() - 80));
                            }
                        }
                    });
                }

                Self::show_limited_code_list(ui, "Game modes", &map.game_modes, 80);
                Self::show_limited_code_list(ui, "Frontend entry points", &map.frontend_functions, 80);
                Self::show_limited_code_list(ui, "Character names", &map.character_names, 80);
                Self::show_limited_code_list(ui, "RTTI classes", &map.rtti_classes, 120);
                Self::show_limited_code_list(ui, "Function names", &map.function_names, 160);
                Self::show_limited_code_list(ui, "Source/debug paths", &map.source_paths, 120);
                Self::show_limited_code_list(ui, "Script/XML clues", &map.script_tokens, 120);
                Self::show_limited_code_list(
                    ui,
                    "Mod-loader/proxy clues",
                    &map.modloader_tokens,
                    120,
                );
                Self::show_limited_code_list(ui, "Hardcoded asset refs", &map.asset_refs, 160);
            });
    }

    fn show_mod_workspace(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Mod workspace")
            .default_open(true)
            .show(ui, |ui| {
                ui.small(
                    "Lua is the planned runtime scripting language. These packages live in the game's Mods folder.",
                );

                if self.game_root.is_none() {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Open the Little Britain game folder to create or edit mods.",
                    );
                    return;
                }

                ui.horizontal(|ui| {
                    if ui.small_button("Refresh").clicked() {
                        self.refresh_mod_workspace();
                    }

                    ui.label(format!("Mods: {}", self.mods.len()));
                });

                ui.horizontal(|ui| {
                    ui.label("New:");
                    ui.text_edit_singleline(&mut self.new_mod_name);
                });

                if ui.button("Create Lua Mod").clicked() {
                    self.create_new_lua_mod();
                }

                if self.selected_mod_index.is_some() {
                    ui.horizontal(|ui| {
                        ui.label("Script:");
                        ui.text_edit_singleline(&mut self.new_script_name);
                    });

                    if ui.button("Add Script To Selected Mod").clicked() {
                        self.create_new_lua_script_for_selected_mod();
                    }
                } else {
                    ui.small("Select a mod to add more Lua scripts.");
                }

                if let Some(err) = &self.mods_error {
                    ui.colored_label(egui::Color32::RED, err);
                }

                if let Some(err) = &self.mod_script_error {
                    ui.colored_label(egui::Color32::RED, err);
                }

                ui.separator();

                if self.mods.is_empty() {
                    ui.small("No mod packages found yet.");
                    return;
                }

                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let packages = self.mods.clone();
                        for (index, package) in packages.iter().enumerate() {
                            let selected = self.selected_mod_index == Some(index);
                            let title = format!(
                                "{}  v{}",
                                package.manifest.name, package.manifest.version
                            );

                            egui::CollapsingHeader::new(title)
                                .default_open(selected)
                                .show(ui, |ui| {
                                    if ui.selectable_label(selected, "Select mod").clicked() {
                                        self.selected_mod_index = Some(index);
                                    }

                                    ui.label(format!("Path: {}", package.path.display()));
                                    ui.label(format!("Manifest: {}", package.manifest_path.display()));
                                    ui.label(format!("Language: {}", package.manifest.language));
                                    ui.label(format!(
                                        "Entry script: {}",
                                        package.manifest.entry_script
                                    ));
                                    ui.small(&package.manifest.description);

                                    ui.separator();
                                    ui.label(format!("Scripts: {}", package.scripts.len()));
                                    for script_path in &package.scripts {
                                        let name = script_path
                                            .strip_prefix(&package.path)
                                            .unwrap_or(script_path)
                                            .display()
                                            .to_string();

                                        if ui.button(name).clicked() {
                                            self.open_mod_script(script_path.clone());
                                        }
                                    }

                                    ui.label(format!("Assets: {}", package.assets.len()));
                                    ui.label(format!("Patches: {}", package.patches.len()));
                                });
                        }
                    });
            });
    }

    fn show_mod_script_editor(&mut self, ui: &mut egui::Ui) {
        let Some(path) = self.mod_script_path.clone() else {
            ui.label("Select a file or create a Lua mod.");
            return;
        };

        ui.horizontal(|ui| {
            ui.heading("Lua Script");
            if self.mod_script_dirty {
                ui.colored_label(egui::Color32::YELLOW, "modified");
            }
        });

        ui.label(path.display().to_string());

        ui.horizontal(|ui| {
            if ui.button("Save Script").clicked() {
                self.save_mod_script();
            }

            if ui.button("Close Script").clicked() {
                self.mod_script_path = None;
                self.mod_script_text.clear();
                self.mod_script_dirty = false;
                self.mod_script_window_open = false;
                self.mod_script_error = None;
            }
        });

        if let Some(err) = &self.mod_script_error {
            ui.colored_label(egui::Color32::RED, err);
        }

        ui.separator();

        let response = ui.add(
            egui::TextEdit::multiline(&mut self.mod_script_text)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(34)
                .lock_focus(true),
        );

        if response.changed() {
            self.mod_script_dirty = true;
        }

        ui.small("Ctrl+S saves this script while it is open.");
    }

    fn show_mod_script_editor_window(&mut self, ctx: &egui::Context) {
        if self.mod_script_path.is_none() || !self.mod_script_window_open {
            return;
        }

        let mut open = self.mod_script_window_open;
        egui::Window::new("Lua Script Editor")
            .id(egui::Id::new("lua_script_editor_window"))
            .open(&mut open)
            .resizable(true)
            .default_size(egui::vec2(760.0, 620.0))
            .show(ctx, |ui| {
                self.show_mod_script_editor(ui);
            });
        self.mod_script_window_open = open;
    }

    fn current_scn_selection(
        selected_scn_node: Option<usize>,
        selected_scn_chunk: Option<usize>,
    ) -> Option<SceneSelection> {
        selected_scn_node
            .map(SceneSelection::Node)
            .or_else(|| selected_scn_chunk.map(SceneSelection::MeshChunk))
    }

    fn apply_scn_selection(
        selected_scn_node: &mut Option<usize>,
        selected_scn_chunk: &mut Option<usize>,
        selection: SceneSelection,
    ) {
        match selection {
            SceneSelection::Node(index) => {
                *selected_scn_node = Some(index);
                *selected_scn_chunk = None;
            }
            SceneSelection::MeshChunk(index) => {
                *selected_scn_chunk = Some(index);
                *selected_scn_node = None;
            }
        }
    }

    fn scn_item_is_visible(
        hidden_scn_nodes: &std::collections::BTreeSet<usize>,
        hidden_scn_chunks: &std::collections::BTreeSet<usize>,
        selection: SceneSelection,
    ) -> bool {
        match selection {
            SceneSelection::Node(index) => !hidden_scn_nodes.contains(&index),
            SceneSelection::MeshChunk(index) => !hidden_scn_chunks.contains(&index),
        }
    }

    fn set_scn_item_visibility(
        hidden_scn_nodes: &mut std::collections::BTreeSet<usize>,
        hidden_scn_chunks: &mut std::collections::BTreeSet<usize>,
        selection: SceneSelection,
        visible: bool,
    ) {
        match selection {
            SceneSelection::Node(index) => {
                if visible {
                    hidden_scn_nodes.remove(&index);
                } else {
                    hidden_scn_nodes.insert(index);
                }
            }
            SceneSelection::MeshChunk(index) => {
                if visible {
                    hidden_scn_chunks.remove(&index);
                } else {
                    hidden_scn_chunks.insert(index);
                }
            }
        }
    }

    fn apply_scn_transform_point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
        [
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        ]
    }

    fn scn_chunk_focus_point(chunk: &ScnMeshChunk) -> Option<[f32; 3]> {
        let mut min = [0.0f32; 3];
        let mut max = [0.0f32; 3];
        let mut have_bounds = false;

        for vertex in &chunk.vertices {
            let world_pos = if let Some(transform) = &chunk.transform {
                Self::apply_scn_transform_point(transform, vertex.position)
            } else {
                vertex.position
            };

            if !have_bounds {
                min = world_pos;
                max = world_pos;
                have_bounds = true;
            } else {
                min[0] = min[0].min(world_pos[0]);
                min[1] = min[1].min(world_pos[1]);
                min[2] = min[2].min(world_pos[2]);
                max[0] = max[0].max(world_pos[0]);
                max[1] = max[1].max(world_pos[1]);
                max[2] = max[2].max(world_pos[2]);
            }
        }

        have_bounds.then_some([
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ])
    }

    fn scn_chunk_fallback_name(chunk: &ScnMeshChunk) -> String {
        chunk
            .texture_names
            .first()
            .and_then(|name| {
                let trimmed = name.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(
                        Path::new(trimmed)
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .unwrap_or(trimmed)
                            .to_owned(),
                    )
                }
            })
            .unwrap_or_else(|| format!("entry {}", chunk.entry_index))
    }

    fn scn_chunk_display_name(
        names: &std::collections::BTreeSet<String>,
        chunk: &ScnMeshChunk,
    ) -> String {
        let Some(first_name) = names.iter().next() else {
            return Self::scn_chunk_fallback_name(chunk);
        };

        let extra_count = names.len().saturating_sub(1);
        if extra_count == 0 {
            first_name.clone()
        } else {
            format!("{} +{}", first_name, extra_count)
        }
    }

    fn scn_chunk_group_name(
        names: &std::collections::BTreeSet<String>,
        chunk: &ScnMeshChunk,
    ) -> String {
        let groups = names
            .iter()
            .map(|name| Self::scn_node_group_name(name))
            .collect::<std::collections::BTreeSet<_>>();

        if let Some(first_group) = groups.iter().next() {
            let extra_count = groups.len().saturating_sub(1);
            if extra_count == 0 {
                first_group.clone()
            } else {
                format!("{} +{}", first_group, extra_count)
            }
        } else {
            Self::scn_node_group_name(&Self::scn_chunk_fallback_name(chunk))
        }
    }

    fn show_scn_scene_items_inspector(
        ui: &mut egui::Ui,
        scn: &ScnFile,
        scn_scene_unresolved: &[String],
        selected_scn_node: &mut Option<usize>,
        selected_scn_chunk: &mut Option<usize>,
        hidden_scn_nodes: &mut std::collections::BTreeSet<usize>,
        hidden_scn_chunks: &mut std::collections::BTreeSet<usize>,
        scn_viewer: &mut GeoViewerState,
        embedded_texture_previews: &std::collections::HashMap<String, DdsPreview>,
        marker_geo_overrides: &std::collections::HashMap<usize, String>,
        pending_embedded_texture_preview: &mut Option<(String, DdsPreview)>,
        pending_marker_geo_preview: &mut Option<String>,
    ) {
        if !scn_scene_unresolved.is_empty() {
            egui::CollapsingHeader::new("Missing archetypes")
                .default_open(false)
                .show(ui, |ui| {
                    for name in scn_scene_unresolved {
                        ui.monospace(name);
                    }
                });
        }

        ui.separator();
        ui.heading("Scene items");
        ui.separator();

        ui.horizontal(|ui| {
            let hidden_total = hidden_scn_nodes.len() + hidden_scn_chunks.len();
            ui.small(format!(
                "Hidden: {} nodes, {} chunks",
                hidden_scn_nodes.len(),
                hidden_scn_chunks.len()
            ));

            if hidden_total > 0 && ui.small_button("Show all").clicked() {
                hidden_scn_nodes.clear();
                hidden_scn_chunks.clear();
                ui.ctx().request_repaint();
            }
        });

        let mut grouped_markers: std::collections::BTreeMap<
            String,
            Vec<(usize, String, [f32; 3], u32, u16, Option<String>)>,
        > = std::collections::BTreeMap::new();

        for node in scn.nodes.iter().filter(|node| node.is_marker()) {
            grouped_markers
                .entry(Self::scn_marker_kind(&node.name).to_owned())
                .or_default()
                .push((
                    node.index,
                    node.name.clone(),
                    node.translation,
                    node.record_offset,
                    node.flags,
                    marker_geo_overrides.get(&node.index).cloned(),
                ));
        }

        egui::CollapsingHeader::new("Markers")
            .id_salt(format!("scene_markers:{}", scn.path.display()))
            .default_open(false)
            .show(ui, |ui| {
                if grouped_markers.is_empty() {
                    ui.small("No named marker/gameplay nodes were found in this SCN.");
                    return;
                }

                ui.small(
                    "Markers are SCN nodes with no GEO archetype. The game uses their names as anchors for starts, cameras, routes, actors, and gameplay targets.",
                );

                egui::ScrollArea::vertical()
                    .id_salt(format!("scene_markers_scroll:{}", scn.path.display()))
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (kind, markers) in &grouped_markers {
                            egui::CollapsingHeader::new(format!("{} ({})", kind, markers.len()))
                                .default_open(false)
                                .show(ui, |ui| {
                                    for (
                                        index,
                                        name,
                                        translation,
                                        record_offset,
                                        flags,
                                        preview_geo,
                                    ) in markers
                                    {
                                        let display_name = if name.trim().is_empty() {
                                            "(unnamed)"
                                        } else {
                                            name.as_str()
                                        };
                                        let label = preview_geo
                                            .as_ref()
                                            .map(|stem| {
                                                format!("#{:04} {}  [geo: {}]", index, display_name, stem)
                                            })
                                            .unwrap_or_else(|| {
                                                format!("#{:04} {}", index, display_name)
                                            });
                                        let hover_text = format!(
                                            "kind={}\npreview_geo={}\npos=({:.2}, {:.2}, {:.2})\nrec=0x{:08X}\nflags=0x{:04X}",
                                            kind,
                                            preview_geo.as_deref().unwrap_or("(none)"),
                                            translation[0],
                                            translation[1],
                                            translation[2],
                                            record_offset,
                                            flags,
                                        );
                                        let is_selected = *selected_scn_node == Some(*index);
                                        let is_visible = !hidden_scn_nodes.contains(index);

                                        ui.horizontal(|ui| {
                                            if ui
                                                .small_button(if is_visible { "Hide" } else { "Show" })
                                                .clicked()
                                            {
                                                Self::set_scn_item_visibility(
                                                    hidden_scn_nodes,
                                                    hidden_scn_chunks,
                                                    SceneSelection::Node(*index),
                                                    !is_visible,
                                                );
                                                ui.ctx().request_repaint();
                                            }

                                            let text = if is_visible {
                                                egui::RichText::new(label.clone())
                                            } else {
                                                egui::RichText::new(label.clone()).weak()
                                            };
                                            let response = ui.selectable_label(is_selected, text);
                                            if response.clicked() {
                                                Self::apply_scn_selection(
                                                    selected_scn_node,
                                                    selected_scn_chunk,
                                                    SceneSelection::Node(*index),
                                                );
                                                focus_scene_viewer_on_point(scn_viewer, *translation);
                                                ui.ctx().request_repaint();
                                            }
                                            response.on_hover_text(hover_text);
                                        });
                                    }
                                });
                        }
                    });
            });

        let mut grouped_scene_nodes: std::collections::BTreeMap<
            String,
            Vec<(usize, String, String, [f32; 3], u32, u16)>,
        > = std::collections::BTreeMap::new();

        for node in &scn.nodes {
            grouped_scene_nodes
                .entry(Self::scn_node_group_name(&node.name))
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
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(format!("scene_nodes_scroll:{}", scn.path.display()))
                    .max_height(260.0)
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
                                for (index, name, archetype, translation, record_offset, flags) in
                                    nodes
                                {
                                    let display_name = if name.trim().is_empty() {
                                        "(unnamed)"
                                    } else {
                                        name.as_str()
                                    };
                                    let label =
                                        format!("#{:04} {} [{}]", index, display_name, archetype);
                                    let hover_text = format!(
                                        "pos=({:.2}, {:.2}, {:.2})\nrec=0x{:08X}\nflags=0x{:04X}",
                                        translation[0],
                                        translation[1],
                                        translation[2],
                                        record_offset,
                                        flags,
                                    );
                                    let is_selected = *selected_scn_node == Some(*index);
                                    let is_visible = !hidden_scn_nodes.contains(index);

                                    ui.horizontal(|ui| {
                                        if ui
                                            .small_button(if is_visible { "Hide" } else { "Show" })
                                            .clicked()
                                        {
                                            Self::set_scn_item_visibility(
                                                hidden_scn_nodes,
                                                hidden_scn_chunks,
                                                SceneSelection::Node(*index),
                                                !is_visible,
                                            );
                                            ui.ctx().request_repaint();
                                        }

                                        let text = if is_visible {
                                            egui::RichText::new(label.clone())
                                        } else {
                                            egui::RichText::new(label.clone()).weak()
                                        };
                                        let response = ui.selectable_label(is_selected, text);
                                        if response.clicked() {
                                            Self::apply_scn_selection(
                                                selected_scn_node,
                                                selected_scn_chunk,
                                                SceneSelection::Node(*index),
                                            );
                                            focus_scene_viewer_on_point(scn_viewer, *translation);
                                            ui.ctx().request_repaint();
                                        }
                                        response.on_hover_text(hover_text);
                                    });
                                }
                            });
                        }
                    });
            });

        egui::CollapsingHeader::new("Embedded map chunks")
            .id_salt(format!("scene_chunks:{}", scn.path.display()))
            .default_open(false)
            .show(ui, |ui| {
                let mut chunk_names_by_record =
                    std::collections::BTreeMap::<u32, std::collections::BTreeSet<String>>::new();
                for node in &scn.nodes {
                    let trimmed_name = node.name.trim();
                    if !trimmed_name.is_empty() {
                        chunk_names_by_record
                            .entry(node.record_offset)
                            .or_default()
                            .insert(trimmed_name.to_owned());
                    }
                }

                let mut grouped_chunks: std::collections::BTreeMap<
                    String,
                    Vec<(usize, String, String, Option<[f32; 3]>)>,
                > = std::collections::BTreeMap::new();

                for (chunk_index, chunk) in scn.mesh_chunks.iter().enumerate() {
                    let name_hints = chunk_names_by_record
                        .get(&chunk.entry_offset)
                        .cloned()
                        .unwrap_or_default();
                    if !name_hints.is_empty() {
                        continue;
                    }
                    let display_name = Self::scn_chunk_display_name(&name_hints, chunk);
                    let group_name = Self::scn_chunk_group_name(&name_hints, chunk);
                    let texture_hint = chunk
                        .texture_names
                        .first()
                        .map(String::as_str)
                        .unwrap_or("(no texture)");
                    let names_line = if name_hints.is_empty() {
                        "Names: none".to_owned()
                    } else {
                        format!(
                            "Names: {}",
                            name_hints.iter().cloned().collect::<Vec<_>>().join(", ")
                        )
                    };
                    let hover_text = format!(
                        "{}\nentry_off=0x{:08X}\nrecord_kind={}\nvertices={}\nindices={}\ntextures={}\nfirst_texture={}",
                        names_line,
                        chunk.entry_offset,
                        chunk.record_kind,
                        chunk.vertex_count,
                        chunk.index_count,
                        chunk.texture_names.len(),
                        texture_hint,
                    );

                    grouped_chunks.entry(group_name).or_default().push((
                        chunk_index,
                        display_name,
                        hover_text,
                        Self::scn_chunk_focus_point(chunk),
                    ));
                }

                egui::ScrollArea::vertical()
                    .id_salt(format!("scene_chunks_scroll:{}", scn.path.display()))
                    .max_height(220.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if grouped_chunks.is_empty() {
                            ui.small(
                                "All named embedded chunks are already represented by scene nodes. Unreferenced map chunks will show here.",
                            );
                        }
                        for (group_name, chunks) in &grouped_chunks {
                            egui::CollapsingHeader::new(format!(
                                "{} ({})",
                                group_name,
                                chunks.len()
                            ))
                            .default_open(false)
                            .show(ui, |ui| {
                                for (chunk_index, display_name, hover_text, focus_point) in chunks {
                                    let chunk = &scn.mesh_chunks[*chunk_index];
                                    let label = format!(
                                        "#{:04} {}  entry={}  tris={}",
                                        chunk_index,
                                        display_name,
                                        chunk.entry_index,
                                        chunk.indices.len() / 3,
                                    );
                                    let is_selected = *selected_scn_chunk == Some(*chunk_index);
                                    let is_visible = !hidden_scn_chunks.contains(chunk_index);

                                    ui.horizontal(|ui| {
                                        if ui
                                            .small_button(if is_visible { "Hide" } else { "Show" })
                                            .clicked()
                                        {
                                            Self::set_scn_item_visibility(
                                                hidden_scn_nodes,
                                                hidden_scn_chunks,
                                                SceneSelection::MeshChunk(*chunk_index),
                                                !is_visible,
                                            );
                                            ui.ctx().request_repaint();
                                        }

                                        let text = if is_visible {
                                            egui::RichText::new(label.clone())
                                        } else {
                                            egui::RichText::new(label.clone()).weak()
                                        };
                                        let response = ui.selectable_label(is_selected, text);
                                        if response.clicked() {
                                            Self::set_scn_item_visibility(
                                                hidden_scn_nodes,
                                                hidden_scn_chunks,
                                                SceneSelection::MeshChunk(*chunk_index),
                                                true,
                                            );
                                            Self::apply_scn_selection(
                                                selected_scn_node,
                                                selected_scn_chunk,
                                                SceneSelection::MeshChunk(*chunk_index),
                                            );
                                            if let Some(center) = focus_point {
                                                focus_scene_viewer_on_point(scn_viewer, *center);
                                            }
                                            ui.ctx().request_repaint();
                                        }
                                        response.on_hover_text(hover_text);
                                    });
                                }
                            });
                        }
                    });
            });

        if let Some(selection) =
            Self::current_scn_selection(*selected_scn_node, *selected_scn_chunk)
        {
            let is_visible =
                Self::scn_item_is_visible(hidden_scn_nodes, hidden_scn_chunks, selection);

            ui.separator();
            ui.heading("Selected scene item");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label(format!(
                    "Visible: {}",
                    if is_visible { "yes" } else { "no" }
                ));
                if ui
                    .small_button(if is_visible { "Hide item" } else { "Show item" })
                    .clicked()
                {
                    Self::set_scn_item_visibility(
                        hidden_scn_nodes,
                        hidden_scn_chunks,
                        selection,
                        !is_visible,
                    );
                    ui.ctx().request_repaint();
                }
            });

            match selection {
                SceneSelection::Node(selected_index) => {
                    if let Some(node) = scn.nodes.get(selected_index) {
                        let display_name = if node.name.trim().is_empty() {
                            "(unnamed)"
                        } else {
                            node.name.as_str()
                        };

                        ui.label("Type: Scene node");
                        ui.label(format!("Name: {}", display_name));
                        ui.label(format!("Archetype: {}", node.archetype_label()));
                        ui.label(format!(
                            "Position: ({:.2}, {:.2}, {:.2})",
                            node.translation[0], node.translation[1], node.translation[2]
                        ));
                        ui.label(format!("Record: 0x{:08X}", node.record_offset));
                        ui.label(format!("Flags: 0x{:04X}", node.flags));

                        if node.is_marker() {
                            ui.label(format!(
                                "Marker kind: {}",
                                Self::scn_marker_kind(&node.name)
                            ));
                            if let Some(preview_geo) = marker_geo_overrides.get(&node.index) {
                                ui.horizontal(|ui| {
                                    ui.label("Preview GEO:");
                                    if ui.button(preview_geo).clicked() {
                                        *pending_marker_geo_preview = Some(preview_geo.clone());
                                    }
                                });
                            }
                        }

                        ui.separator();
                        ui.heading("Logic / scripts");
                        ui.small(
                            "No standalone script file is attached to this SCN object. The game appears to use compiled C++ logic and node/marker names as lookup anchors.",
                        );
                        ui.small(
                            "Once the runtime Lua modloader is wired, we can add our own scripts that target this name or marker kind.",
                        );
                    }
                }
                SceneSelection::MeshChunk(selected_chunk_index) => {
                    if let Some(chunk) = scn.mesh_chunks.get(selected_chunk_index) {
                        ui.label("Type: Embedded mesh chunk");
                        ui.label(format!("Chunk index: {}", selected_chunk_index));
                        ui.label(format!("Entry index: {}", chunk.entry_index));
                        ui.label(format!("Entry offset: 0x{:08X}", chunk.entry_offset));
                        ui.label(format!("Record kind: {}", chunk.record_kind));
                        ui.label(format!("Vertices: {}", chunk.vertex_count));
                        ui.label(format!("Indices: {}", chunk.index_count));
                        ui.label(format!("Texture spans: {}", chunk.texture_spans.len()));

                        if let Some(transform_index) = chunk.transform_index {
                            ui.label(format!("Transform index: {}", transform_index));
                        } else {
                            ui.label("Transform index: none");
                        }

                        if !chunk.texture_names.is_empty() {
                            ui.separator();
                            ui.heading("Chunk textures");
                            ui.separator();

                            for (slot, name) in chunk.texture_names.iter().enumerate() {
                                let trimmed = name.trim();

                                if trimmed.is_empty() {
                                    ui.label(format!("slot {}: solid/material color", slot));
                                    continue;
                                }

                                let key = trimmed.to_ascii_lowercase();
                                if let Some(preview) = embedded_texture_previews.get(&key) {
                                    if ui.button(trimmed).clicked() {
                                        *pending_embedded_texture_preview =
                                            Some((trimmed.to_owned(), preview.clone()));
                                    }
                                } else {
                                    ui.colored_label(
                                        egui::Color32::YELLOW,
                                        format!("slot {}: {} (not previewed)", slot, trimmed),
                                    );
                                }
                            }
                        }

                        let referenced_nodes: Vec<_> = scn
                            .nodes
                            .iter()
                            .filter(|node| node.record_offset == chunk.entry_offset)
                            .collect();

                        if !referenced_nodes.is_empty() {
                            ui.separator();
                            ui.heading("Nodes sharing this record");
                            ui.separator();

                            for node in referenced_nodes {
                                let display_name = if node.name.trim().is_empty() {
                                    "(unnamed)"
                                } else {
                                    node.name.as_str()
                                };

                                ui.monospace(format!(
                                    "#{:04}  {:<22}  {:<12}",
                                    node.index,
                                    display_name,
                                    node.archetype_label()
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    fn build_asset_links(&self) -> AssetLinks {
        Self::build_asset_links_from_tree(&self.tree)
    }

    fn build_asset_links_from_tree(tree: &[FileNode]) -> AssetLinks {
        let mut links = AssetLinks::default();

        let mut texture_nodes = Vec::new();
        let mut model_nodes = Vec::new();

        Self::collect_files_by_category(tree, AssetCategory::Texture, &mut texture_nodes);
        Self::collect_files_by_category(tree, AssetCategory::Model, &mut model_nodes);

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

        self.poll_loading_jobs(ui.ctx());

        self.ensure_bnk_loaded();
        self.ensure_dds_preview_loaded(ui.ctx());
        self.ensure_bik_preview_loaded(ui.ctx());
        self.poll_bik_decoder(ui.ctx());
        self.update_bik_playback_clock(ui.ctx());
        self.ensure_anm_loaded();
        self.ensure_active_geo_animation_loaded();
        self.ensure_geo_loaded();
        self.ensure_scn_loaded(ui.ctx());
        self.ensure_scn_scene_loaded(ui.ctx());
        self.ensure_geo_materials_loaded(ui.ctx());

        if ui
            .ctx()
            .input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S))
        {
            if self.mod_script_window_open && self.mod_script_path.is_some() {
                self.save_mod_script();
            } else if let (Some(scn), Some(path)) = (&self.scn_file, &self.scn_loaded_path) {
                match scn.save_scn(path) {
                    Ok(_) => {
                        self.status = format!(
                            "Saved: {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                    }
                    Err(e) => {
                        self.scn_error = Some(format!("Save failed: {}", e));
                    }
                }
            }
        }

        let mut pending_jump: Option<PathBuf> = None;
        let mut pending_texture_preview_path: Option<PathBuf> = None;
        let mut pending_embedded_texture_preview: Option<(String, DdsPreview)> = None;
        let mut pending_marker_geo_preview: Option<String> = None;
        if let Some(player) = self.audio_player.as_ref() {
            if !player.is_empty() {
                ui.ctx().request_repaint();
            }
        }

        self.show_mod_script_editor_window(ui.ctx());

        egui::Panel::top("top_bar").show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open game folder").clicked() {
                    self.open_game_folder(ui.ctx());
                }

                if ui
                    .add_enabled(self.game_root.is_some(), egui::Button::new("Rescan"))
                    .clicked()
                {
                    self.rescan(ui.ctx());
                }

                if ui
                    .add_enabled(self.scn_file.is_some(), egui::Button::new("Save"))
                    .clicked()
                {
                    if let (Some(scn), Some(path)) = (&self.scn_file, &self.scn_loaded_path) {
                        match scn.save_scn(path) {
                            Ok(_) => {
                                self.status = format!(
                                    "Saved: {}",
                                    path.file_name().unwrap_or_default().to_string_lossy()
                                );
                            }
                            Err(e) => {
                                self.scn_error = Some(format!("Save failed: {}", e));
                            }
                        }
                    }
                }

                let theme_label = if self.dark_mode {
                    "Light mode"
                } else {
                    "Dark mode"
                };

                if ui.button(theme_label).clicked() {
                    self.dark_mode = !self.dark_mode;
                }

                ui.separator();
                ui.label(&self.status);
            });
        });

        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(340.0)
            .show_inside(ui, |ui| {
                ui.heading("Inspector");
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_salt("inspector_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if matches!(self.selected_extension().as_deref(), Some("scn")) {
                            if let Some(scn) = self.scn_file.as_ref() {
                                self.show_scn_quick_summary(ui, scn);

                                Self::show_scn_scene_items_inspector(
                                    ui,
                                    scn,
                                    &self.scn_scene_unresolved,
                                    &mut self.selected_scn_node,
                                    &mut self.selected_scn_chunk,
                                    &mut self.hidden_scn_nodes,
                                    &mut self.hidden_scn_chunks,
                                    &mut self.scn_viewer,
                                    &self.scn_embedded_texture_previews,
                                    &self.scn_marker_geo_overrides,
                                    &mut pending_embedded_texture_preview,
                                    &mut pending_marker_geo_preview,
                                );

                                ui.separator();
                            }
                        }

                        self.show_mod_workspace(ui);
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

                            ui.separator();
                            Self::show_game_code_map(ui, &self.game_code_map, Some(path.as_path()));

                            ui.separator();

                            if ext == "dds" {
                                if let Some(preview) = &self.dds_preview {
                                    ui.label(format!("Width: {}", preview.width));
                                    ui.label(format!("Height: {}", preview.height));
                                    ui.label(format!("Mipmaps: {}", preview.mipmaps));
                                }

                                if let Some(err) = &self.dds_error {
                                    ui.colored_label(egui::Color32::RED, format!("DDS preview error: {}", err));
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
                        if let Some(anm) = self.anm_file.as_ref() {
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
                                if let Some(scn) = self.scn_file.as_ref() {
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
                                    ui.label(format!(
                                        "Hidden nodes: {}",
                                        self.hidden_scn_nodes.len()
                                    ));
                                    ui.label(format!(
                                        "Hidden chunks: {}",
                                        self.hidden_scn_chunks.len()
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
                            ui.label("Select a file or create a Lua mod.");
                        }
                    });
            });

        egui::Panel::bottom("content_browser")
            .resizable(true)
            .default_size(340.0)
            .min_size(280.0)
            .max_size(720.0)
            .show_inside(ui, |ui| {
                self.show_content_browser(ui);
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
                            let selected_bnk_entry = &mut self.selected_bnk_entry;

                            ui.heading(format!("Entries ({})", entry_count));
                            ui.separator();

                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for entry in &bnk.entries {
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

                                    let is_selected = *selected_bnk_entry == Some(entry.index);

                                    if ui.selectable_label(is_selected, label).clicked() {
                                        *selected_bnk_entry = Some(entry.index);
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
                        if let Some(scn) = self.scn_file.as_mut() {
                            if (self.selected_scn_node.is_some() || self.selected_scn_chunk.is_some())
                                && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape))
                            {
                                self.selected_scn_node = None;
                                self.selected_scn_chunk = None;
                                ui.ctx().request_repaint();
                            }

                            let selected_scene_item = Self::current_scn_selection(
                                self.selected_scn_node,
                                self.selected_scn_chunk,
                            );

                            // Fill the remaining preview panel with the SCN viewport.
                            // The old draggable resize strip/text field was removed, so a fixed
                            // stored height would leave an empty area above the content browser.
                            let scn_view_height = ui.available_height().max(180.0);
                            self.scn_view_height = scn_view_height;

                            let scene_marker_kinds = scn
                                .nodes
                                .iter()
                                .filter(|node| node.is_marker())
                                .map(|node| (node.index, Self::scn_marker_kind(&node.name)))
                                .collect::<std::collections::BTreeMap<usize, &'static str>>();

                            if let Some(picked_item) = draw_scene_viewer(
                                ui,
                                scn,
                                &self.scn_scene_models,
                                &self.scn_embedded_texture_previews,
                                &mut self.scn_viewer,
                                scn_view_height,
                                selected_scene_item,
                                &self.hidden_scn_nodes,
                                &self.hidden_scn_chunks,
                                &self.scn_marker_geo_overrides,
                                &scene_marker_kinds,
                            ) {
                                Self::apply_scn_selection(
                                    &mut self.selected_scn_node,
                                    &mut self.selected_scn_chunk,
                                    picked_item,
                                );
                                ui.ctx().request_repaint();
                            }

                            if let Some(err) = &self.scn_scene_error {
                                ui.colored_label(
                                    egui::Color32::RED,
                                    format!("SCN scene load error: {}", err),
                                );
                            }
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
                        let loaded_geo_path = self.geo_loaded_path.clone();
                        let show_animations = self
                            .geo_file
                            .as_ref()
                            .map(|geo| {
                                geo.skeleton.is_some()
                                    && matches!(
                                        geo.asset_type,
                                        GeoAssetType::SkinnedMesh | GeoAssetType::RigidProp
                                    )
                            })
                            .unwrap_or(false);

                        if show_animations {
                            if let Some(path) = loaded_geo_path.as_deref() {
                                self.ensure_geo_animation_groups_loaded(path);
                            }
                        } else {
                            self.geo_animation_groups_path = None;
                            self.geo_animation_groups.clear();
                        }

                        let animation_groups = if show_animations {
                            self.geo_animation_groups.clone()
                        } else {
                            Vec::new()
                        };

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
                            let model_texture_refs = loaded_geo_path
                                .as_ref()
                                .and_then(|path| self.asset_links.model_to_textures.get(path).cloned());

                            let geo_stem = loaded_geo_path
                                .as_ref()
                                .map(|path| Self::asset_stem_lower(path))
                                .unwrap_or_default();

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
                                                            pending_texture_preview_path =
                                                                Some(path.clone());
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
                                                            Some(path) => {
                                                                if ui.small_button(tex_name).clicked() {
                                                                    pending_texture_preview_path =
                                                                        Some(path.clone());
                                                                }
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
                                                            pending_texture_preview_path =
                                                                Some(path.clone());
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
                                                            Some(path) => {
                                                                if ui.small_button(tex_name).clicked() {
                                                                    pending_texture_preview_path =
                                                                        Some(path.clone());
                                                                }
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

        if let Some((title, preview)) = pending_embedded_texture_preview {
            self.open_texture_preview_window(title, preview);
        }

        if let Some(preview_geo) = pending_marker_geo_preview {
            if let Some(path) = self.scn_preview_geo_path_for_stem(&preview_geo) {
                self.open_geo_preview_window(ui.ctx(), &path);
            } else {
                self.status = format!(
                    "Could not find GEO preview file for marker preview: {}",
                    preview_geo
                );
            }
        }

        if let Some(path) = pending_texture_preview_path {
            self.open_texture_path_preview_window(ui.ctx(), &path);
        }

        self.draw_file_preview_windows(ui.ctx());
        self.draw_texture_preview_windows(ui.ctx());
        self.draw_audio_preview_windows(ui.ctx());
        self.draw_bnk_preview_windows(ui.ctx());
        self.draw_geo_preview_windows(ui.ctx());

        if let Some(path) = pending_jump {
            self.jump_to_file(path);
        }

        loading::draw_loading_overlay(ui.ctx(), self.active_loading_job.as_ref().map(|job| &job.task));
    }
}
