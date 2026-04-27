use eframe::egui;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum LoadingProgress {
    Indeterminate,
    Fraction { value: f32, label: Option<String> },
}

impl Default for LoadingProgress {
    fn default() -> Self {
        Self::Indeterminate
    }
}

#[derive(Debug)]
pub struct LoadingTask {
    pub id: u64,
    pub title: String,
    pub detail: Option<String>,
    pub progress: LoadingProgress,
    pub started_at: Instant,
    pub overlay_delay: Duration,
}

impl LoadingTask {
    pub fn new(id: u64, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            detail: None,
            progress: LoadingProgress::Indeterminate,
            started_at: Instant::now(),
            overlay_delay: Duration::from_millis(150),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn should_show_overlay(&self) -> bool {
        self.elapsed() >= self.overlay_delay
    }

    pub fn set_progress(&mut self, value: Option<f32>, detail: Option<String>) {
        self.detail = detail.clone();
        self.progress = match value {
            Some(value) => LoadingProgress::Fraction {
                value: value.clamp(0.0, 1.0),
                label: detail,
            },
            None => LoadingProgress::Indeterminate,
        };
    }
}

pub fn draw_loading_overlay(ctx: &egui::Context, task: Option<&LoadingTask>) {
    let Some(task) = task else {
        return;
    };

    if !task.should_show_overlay() {
        return;
    }

    let screen_rect = ctx.screen_rect();

    egui::Area::new(egui::Id::new("global_loading_overlay_blocker"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen_rect.min)
        .show(ctx, |ui| {
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
            ui.allocate_rect(rect, egui::Sense::click_and_drag());
        });

    egui::Area::new(egui::Id::new("global_loading_overlay"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::window(ui.style())
                .fill(egui::Color32::from_rgb(24, 24, 26))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(85)))
                .corner_radius(egui::CornerRadius::same(12))
                .show(ui, |ui| {
                    ui.set_min_width(360.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(4.0);
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.heading(&task.title);

                        if let Some(detail) = &task.detail {
                            ui.add_space(2.0);
                            ui.small(detail);
                        }

                        ui.add_space(12.0);

                        match &task.progress {
                            LoadingProgress::Fraction { value, label } => {
                                let mut bar = egui::ProgressBar::new(*value).desired_width(300.0);
                                if label.is_some() {
                                    bar = bar.show_percentage();
                                }
                                ui.add(bar);
                            }
                            LoadingProgress::Indeterminate => {
                                let phase = (task.elapsed().as_secs_f32() * 0.65).fract();
                                let value = 0.18 + 0.64 * phase;
                                ui.add(
                                    egui::ProgressBar::new(value)
                                        .animate(true)
                                        .desired_width(300.0),
                                );
                            }
                        }

                        ui.add_space(4.0);
                    });
                });
        });
}
