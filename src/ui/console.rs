//! The mixer console: apps on the left, each with a visible output chip; playback devices and
//! microphones on the right.

use eframe::egui::{self, vec2, RichText, Ui};

use super::strip::{self, Glyph, Strip, STRIP_W};
use super::{theme, wid, MixerApp};
use crate::model::{AppStrip, Command, Device, Flow, Snapshot};

/// Padding a panel adds around its strips: 14 each side plus the 1 px border.
const PANEL_PAD_X: f32 = 30.0;
/// Space between strips.
const STRIP_GAP: f32 = 10.0;
/// Space between panels.
const PANEL_GAP: f32 = 12.0;
/// Height taken by a panel's title row and padding.
const PANEL_CHROME_H: f32 = 44.0;

/// Width a panel needs for `n` strips.
fn panel_width(n: usize) -> f32 {
    n as f32 * STRIP_W + n.saturating_sub(1) as f32 * STRIP_GAP + PANEL_PAD_X
}

/// A titled, rounded region that groups related strips. It fills `size`, so panels line up and
/// reach the bottom of the window.
fn panel(ui: &mut Ui, size: egui::Vec2, title: &str, count: usize, body: impl FnOnce(&mut Ui)) {
    ui.allocate_ui(size, |ui| {
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_min_size(size - vec2(PANEL_PAD_X, 20.0 + 2.0));
                ui.set_max_width(size.x - PANEL_PAD_X);
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

        // Use the whole window: the faders take the height left over, and the apps panel takes
        // the width the docked output and microphone panels don't need.
        let avail = ui.available_size();
        let height = avail
            .y
            .max(strip::MIN_TRACK_H + strip::STRIP_CHROME_H + PANEL_CHROME_H);
        let track_h = (height - PANEL_CHROME_H - strip::STRIP_CHROME_H).max(strip::MIN_TRACK_H);
        let out_w = panel_width(outputs.len().max(1));
        let in_w = if inputs.is_empty() {
            0.0
        } else {
            panel_width(inputs.len()) + PANEL_GAP
        };
        let apps_w = (avail.x - out_w - in_w - PANEL_GAP - 2.0).max(panel_width(2));

        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = PANEL_GAP;
            panel(ui, vec2(apps_w, height), "APPS", apps.len(), |ui| {
                if apps.is_empty() {
                    self.empty_apps(ui);
                    return;
                }
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = STRIP_GAP;
                            for app in &apps {
                                self.app_strip(ui, snap, app, track_h);
                            }
                        });
                    });
            });
            panel(ui, vec2(out_w, height), "OUTPUTS", outputs.len(), |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = STRIP_GAP;
                    for d in &outputs {
                        self.device_strip(ui, d, theme::ACCENT_OUT, track_h);
                    }
                });
            });
            if !inputs.is_empty() {
                panel(
                    ui,
                    vec2(in_w - PANEL_GAP, height),
                    "MICROPHONES",
                    inputs.len(),
                    |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = STRIP_GAP;
                            for d in &inputs {
                                self.device_strip(ui, d, theme::ACCENT_IN, track_h);
                            }
                        });
                    },
                );
            }
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

    fn app_strip(&mut self, ui: &mut Ui, snap: &Snapshot, app: &AppStrip, track_h: f32) {
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
            track_h,
            status: Some(app.active),
        }
        .show(ui, |ui| self.device_chip(ui, snap, app, STRIP_W - 4.0));
        self.send_app(app, out);
    }

    fn device_strip(&self, ui: &mut Ui, d: &Device, accent: egui::Color32, track_h: f32) {
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
            track_h,
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
