//! The "update available" banner, the Settings row, and the schedule for checking.

use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, RichText, Ui};

use super::{strip, theme, MixerApp};
use crate::update::{self, State};

/// First check shortly after launch (so start-up stays quick), then twice a day.
const FIRST_CHECK: Duration = Duration::from_secs(8);
const RECHECK: Duration = Duration::from_secs(12 * 3600);

impl MixerApp {
    fn wake(ctx: &egui::Context) -> impl Fn() + Send + 'static {
        let ctx = ctx.clone();
        move || ctx.request_repaint()
    }

    fn update_state(&self) -> State {
        self.update.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Runs every frame: starts checks on schedule, and installs a finished download.
    pub(super) fn drive_updates(&mut self, ctx: &egui::Context) {
        // For trying the banner without a real release: WAVEMIX_FAKE_UPDATE=9.9.9
        if let Some(v) = std::env::var_os("WAVEMIX_FAKE_UPDATE") {
            if self.update_checked.is_none() {
                self.update_checked = Some(Instant::now());
                if let Ok(mut s) = self.update.lock() {
                    *s = State::Available(update::Release {
                        version: v.to_string_lossy().into_owned(),
                        installer_url: String::new(),
                        notes: "Faster meters, a calmer dark theme and smoother animations.".into(),
                    });
                }
            }
            return;
        }
        let due = match self.update_checked {
            None => self.started.elapsed() > FIRST_CHECK,
            Some(at) => at.elapsed() > RECHECK,
        };
        if due && self.config.check_updates {
            self.update_checked = Some(Instant::now());
            update::check(&self.update, Self::wake(ctx));
        }
        if let State::Ready(path) = self.update_state() {
            // Hand over to the installer and get out of its way.
            if let Ok(mut s) = self.update.lock() {
                *s = State::Idle;
            }
            match update::install_and_restart(&path) {
                Ok(()) => {
                    #[cfg(windows)]
                    {
                        self.shell.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Err(e) => {
                    if let Ok(mut s) = self.update.lock() {
                        *s = State::Failed(e);
                    }
                }
            }
        }
        if matches!(self.update_state(), State::Checking | State::Downloading(_)) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }

    /// Whether a banner should be showing.
    pub(super) fn update_banner_wanted(&self) -> bool {
        match self.update_state() {
            State::Available(r) => r.version != self.config.skipped_update,
            State::Downloading(_) => true,
            State::Failed(_) => self.update_failed_visible,
            _ => false,
        }
    }

    pub(super) fn update_banner(&mut self, ui: &mut Ui) {
        let state = self.update_state();
        let ctx = ui.ctx().clone();
        egui::Frame::new()
            .fill(theme::TINT_NOTE())
            .stroke(egui::Stroke::new(
                1.0,
                theme::ACCENT().linear_multiply(0.45),
            ))
            .corner_radius(theme::R_CARD)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let wide = ui.available_width() > 460.0;
                let mut buttons = |ui: &mut Ui| match &state {
                    State::Available(r) => {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let go = strip::toggle(
                            ui,
                            "Update now",
                            true,
                            vec2(104.0, 30.0),
                            theme::ACCENT(),
                        )
                        .clicked();
                        let later = strip::ghost_button(ui, "Later", false).clicked();
                        if go {
                            self.update_failed_visible = true;
                            update::download(&self.update, r.clone(), Self::wake(&ctx));
                        }
                        if later {
                            self.config.skipped_update = r.version.clone();
                            self.config.save();
                        }
                    }
                    State::Failed(_) if strip::ghost_button(ui, "Dismiss", false).clicked() => {
                        self.update_failed_visible = false;
                    }
                    _ => {}
                };
                let (title, detail) = match &state {
                    State::Available(r) => (
                        format!("Wavemix {} is available", r.version),
                        r.notes.lines().next().unwrap_or("").to_string(),
                    ),
                    State::Downloading(v) => (
                        format!("Downloading Wavemix {v}…"),
                        "It will install and restart on its own.".to_string(),
                    ),
                    State::Failed(e) => ("Couldn't update".to_string(), e.clone()),
                    _ => (String::new(), String::new()),
                };
                let text = |ui: &mut Ui| {
                    ui.label(RichText::new(&title).strong());
                    if !detail.is_empty() {
                        ui.label(RichText::new(&detail).size(12.0).color(theme::TEXT_DIM()));
                    }
                };
                if wide {
                    ui.horizontal(|ui| {
                        ui.vertical(text);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            buttons(ui)
                        });
                    });
                } else {
                    text(ui);
                    ui.add_space(6.0);
                    ui.horizontal(|ui| buttons(ui));
                }
            });
        ui.add_space(12.0);
    }

    /// Settings: automatic checks, a button to check now, and what happened.
    pub(super) fn update_settings(&mut self, ui: &mut Ui) {
        let mut auto = self.config.check_updates;
        if ui
            .checkbox(&mut auto, "Check for updates automatically")
            .on_hover_text("Looks at GitHub for a newer Wavemix now and then. Nothing is sent but the request.")
            .changed()
        {
            self.config.check_updates = auto;
            self.config.save();
        }
        let state = self.update_state();
        ui.horizontal(|ui| {
            let busy = matches!(state, State::Checking | State::Downloading(_));
            if ui
                .add_enabled(!busy, egui::Button::new("Check now"))
                .clicked()
            {
                self.config.skipped_update.clear();
                self.update_failed_visible = true;
                update::check(&self.update, Self::wake(ui.ctx()));
            }
            let (text, color) = match &state {
                State::Checking => ("Checking…".to_string(), theme::TEXT_DIM()),
                State::UpToDate => (
                    format!("You're up to date (Wavemix {})", env!("CARGO_PKG_VERSION")),
                    theme::TEXT_DIM(),
                ),
                State::Available(r) => (
                    format!("Wavemix {} is available", r.version),
                    theme::NOTE_TEXT(),
                ),
                State::Downloading(v) => (format!("Downloading {v}…"), theme::TEXT_DIM()),
                State::Failed(e) => (format!("Couldn't check: {e}"), theme::NOTE_TEXT()),
                _ => (
                    format!("Wavemix {}", env!("CARGO_PKG_VERSION")),
                    theme::TEXT_DIM(),
                ),
            };
            ui.label(RichText::new(text).size(12.0).color(color));
        });
    }
}
