//! The mixer console: apps on the left, each with a visible output chip; playback devices and
//! microphones on the right.

use eframe::egui::{self, vec2, RichText, Ui};

use super::strip::{self, Glyph, Strip, STRIP_W};
use super::{theme, wid, MixerApp};
use crate::model::{AppStrip, Command, Device, Flow, Snapshot};

/// A titled, rounded region that groups related strips.
fn panel(ui: &mut Ui, title: &str, count: usize, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(title)
                            .size(11.0)
                            .strong()
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        RichText::new(count.to_string())
                            .size(11.0)
                            .color(theme::TEXT_DIM),
                    );
                });
                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    body(ui);
                });
            });
        });
}

impl MixerApp {
    pub(super) fn console(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let apps = self.all_apps(snap, self.config.show_idle);
        let outputs: Vec<&Device> = snap
            .devices
            .iter()
            .filter(|d| d.flow == Flow::Render)
            .collect();
        let inputs: Vec<&Device> = snap
            .devices
            .iter()
            .filter(|d| d.flow == Flow::Capture)
            .collect();

        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    panel(ui, "APPS", apps.len(), |ui| {
                        if apps.is_empty() {
                            self.empty_apps(ui);
                        }
                        for app in &apps {
                            self.app_strip(ui, snap, app);
                        }
                    });
                    panel(ui, "OUTPUTS", outputs.len(), |ui| {
                        for d in &outputs {
                            self.device_strip(ui, d, theme::ACCENT_OUT);
                        }
                    });
                    if !inputs.is_empty() {
                        panel(ui, "MICROPHONES", inputs.len(), |ui| {
                            for d in &inputs {
                                self.device_strip(ui, d, theme::ACCENT_IN);
                            }
                        });
                    }
                });
            });
    }

    fn empty_apps(&self, ui: &mut Ui) {
        ui.allocate_ui(vec2(200.0, 200.0), |ui| {
            ui.add_space(60.0);
            ui.label(RichText::new("Nothing is playing").strong());
            ui.label(
                RichText::new(
                    "Start music or a video and the app will appear here with its own fader.",
                )
                .size(12.0)
                .color(theme::TEXT_DIM),
            );
        });
    }

    fn app_strip(&mut self, ui: &mut Ui, snap: &Snapshot, app: &AppStrip) {
        let icon = self.icon(ui.ctx(), &app.key, &app.icon);
        let out = Strip {
            id: wid(("app", &app.device_id, &app.key)),
            title: &app.name,
            icon: icon.as_ref(),
            glyph: Glyph::None,
            volume: app.volume,
            muted: app.muted,
            peak: app.peak,
            accent: theme::ACCENT,
            status: Some(app.active),
        }
        .show(ui, |ui| self.device_chip(ui, snap, app, STRIP_W - 4.0));
        self.send_app(app, out);
    }

    fn device_strip(&self, ui: &mut Ui, d: &Device, accent: egui::Color32) {
        let out = Strip {
            id: wid(("device", &d.id)),
            title: strip::short_device_name(&d.name),
            icon: None,
            glyph: if d.flow == Flow::Render {
                Glyph::Speaker
            } else {
                Glyph::Mic
            },
            volume: d.volume,
            muted: d.muted,
            peak: d.peak,
            accent,
            status: None,
        }
        .show(ui, |ui| self.default_badge(ui, d, STRIP_W - 4.0));
        if let Some(volume) = out.volume {
            self.backend.send(Command::SetDeviceVolume {
                id: d.id.clone(),
                volume,
            });
        }
        if let Some(muted) = out.mute {
            self.backend.send(Command::SetDeviceMute {
                id: d.id.clone(),
                muted,
            });
        }
    }
}
