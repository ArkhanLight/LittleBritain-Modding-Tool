mod app;
mod audio_player;
mod bnk;
mod dds_preview;
mod fs_tree;
mod geo;
mod geo_viewer;

fn main() {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([1000.0, 700.0]),
        ..Default::default()
    };

    let result = eframe::run_native(
        "Little Britain Mod Tool",
        options,
        Box::new(|cc| Ok(Box::new(app::ModToolApp::new(cc)))),
    );

    if let Err(err) = result {
        eprintln!("Failed to start app:");
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}
