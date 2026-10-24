//! The compact view, in the spirit of EarTrumpet: the device you're listening on at the top, then
//! every app that's playing with its own slider and the device it plays through.

use eframe::egui::{self, vec2, Align, Layout, RichText, Sense, TextureHandle, Ui};

use super::strip::{self, mute_button};
use super::{theme, wid, MixerApp};
use crate::model::{AppStrip, Command, Device, Flow, Snapshot};

/// Result of drawing a slider row: what the user changed this frame.
#[derive(Default)]
struct RowChange {
    volume: Option<f32>,
    mute: Option<bool>,
}

impl MixerApp {
    pub(super) fn compact(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let outputs: Vec<Device> = snap
            .devices
            .iter()
            .filter(|d| d.flow == Flow::Render)
            .cloned()
            .collect();
        let selected = self
            .compact_device
            .as_ref()
            .and_then(|id| outputs.iter().find(|d| &d.id == id))
            .or_else(|| outputs.iter().find(|d| d.is_default))
            .or(outputs.first())
            .cloned();
        let Some(dev) = selected else {
            ui.add_space(30.0);
            ui.label(RichText::new("No playback devices found").strong());
            ui.label(
                RichText::new("Plug in or enable an output device in Windows sound settings.")
                    .size(12.0)
                    .color(theme::TEXT_DIM),
            );
            return;
        };

        self.device_card(ui, &outputs, &dev);
        ui.add_space(8.0);
        self.output_switch(ui);
        ui.add_space(12.0);
        self.app_list(ui, snap, false);
    }

    /// Every app that makes sound, each with its own slider; apps that are silent are folded away.
    /// `always_pills` shows the output pill on every row (Pro mode's side panel, where sending an
    /// app to a cable is the point); otherwise it only appears when it has something to say.
    pub(super) fn app_list(&mut self, ui: &mut Ui, snap: &Snapshot, always_pills: bool) {
        let default_id = snap
            .devices
            .iter()
            .find(|d| d.flow == Flow::Render && d.is_default)
            .map(|d| d.id.clone());
        let routing_on = self.config.experimental_routing;
        let routed = |app: &AppStrip| routing_on || Some(&app.device_id) != default_id.as_ref();
        let (playing, idle): (Vec<AppStrip>, Vec<AppStrip>) = self
            .all_apps(snap, true)
            .into_iter()
            .partition(|a| a.active);

        let list_top = ui.cursor().top();
        let scroll = egui::ScrollArea::vertical()
            .id_salt("app-list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if playing.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Nothing is playing").strong());
                    ui.label(
                        RichText::new("Apps appear here as soon as they make sound.")
                            .size(12.0)
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(8.0);
                }
                for app in &playing {
                    self.app_row(ui, snap, app, always_pills || routed(app));
                    ui.add_space(6.0);
                }
                if !idle.is_empty() {
                    egui::CollapsingHeader::new(
                        RichText::new(format!("Not playing ({})", idle.len()))
                            .size(12.0)
                            .color(theme::TEXT_DIM),
                    )
                    .default_open(false)
                    .show(ui, |ui| {
                        for app in &idle {
                            self.app_row(ui, snap, app, always_pills || routed(app));
                            ui.add_space(6.0);
                        }
                    });
                }
            });
        // Remember how tall the content wants the window to be (everything above the list, the
        // list itself, and the window's bottom margin) so the taskbar flyout can fit it.
        self.simple_height
            .set(list_top + scroll.content_size.y + 14.0);
    }

    /// The big card: which output you're controlling, and its volume.
    fn device_card(&mut self, ui: &mut Ui, outputs: &[Device], dev: &Device) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(theme::R_CARD)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("OUTPUT")
                            .size(11.0)
                            .strong()
                            .color(theme::TEXT_DIM),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let width = ui.available_width().min(240.0);
                        let resp =
                            strip::chip(
                                ui,
                                &strip::label_among(&dev.name, outputs.iter().map(|d| d.name.as_str())),
                                width,
                                false,
                            )
                                .on_hover_text("Choose which output to control");
                        egui::Popup::menu(&resp).show(|ui| {
                            ui.set_min_width(220.0);
                            for d in outputs {
                                let label = format!(
                                    "{}{}",
                                    d.name,
                                    if d.is_default { "  (default)" } else { "" }
                                );
                                if ui.selectable_label(d.id == dev.id, label).clicked() {
                                    self.compact_device = Some(d.id.clone());
                                    ui.close();
                                }
                            }
                            if !dev.is_default {
                                ui.separator();
                                if ui.button("Make this the Windows default").clicked() {
                                    self.backend.send(Command::SetDefaultDevice {
                                        id: dev.id.clone(),
                                        flow: dev.flow,
                                    });
                                    ui.close();
                                }
                            }
                        });
                    });
                });
                ui.add_space(4.0);
                let change = self.slider_row(
                    ui,
                    wid(("cdev", &dev.id)),
                    dev.volume,
                    dev.muted,
                    dev.peak,
                    theme::ACCENT_OUT,
                );
                if let Some(volume) = change.volume {
                    self.backend.send(Command::SetDeviceVolume {
                        id: dev.id.clone(),
                        volume,
                    });
                }
                if let Some(muted) = change.mute {
                    self.backend.send(Command::SetDeviceMute {
                        id: dev.id.clone(),
                        muted,
                    });
                }
            });
    }

    fn app_row(&mut self, ui: &mut Ui, snap: &Snapshot, app: &AppStrip, pill: bool) {
        let icon: Option<TextureHandle> = self.icon(ui.ctx(), &app.key, &app.icon);
        egui::Frame::new()
            .fill(theme::PANEL)
            .corner_radius(theme::R_CARD)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    if let Some(tex) = &icon {
                        ui.add(egui::Image::new(tex).fit_to_exact_size(vec2(22.0, 22.0)));
                    } else {
                        ui.add_space(22.0);
                    }
                    let color = if app.active {
                        theme::TEXT
                    } else {
                        theme::TEXT_DIM
                    };
                    let reserve = if pill { 150.0 } else { 20.0 };
                    let name = strip::truncate(ui, &app.name, ui.available_width() - reserve, 13.0);
                    ui.label(RichText::new(name).size(13.0).color(color))
                        .on_hover_text(&app.name);
                    if pill {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            self.device_chip(ui, snap, app, 116.0);
                        });
                    }
                });
                ui.add_space(2.0);
                let change = self.slider_row(
                    ui,
                    wid(("capp", &app.device_id, &app.key)),
                    app.volume,
                    app.muted,
                    app.peak,
                    theme::ACCENT,
                );
                self.send_app(
                    app,
                    super::StripOutput {
                        volume: change.volume,
                        mute: change.mute,
                    },
                );
            });
    }

    /// Mute button, a draggable slider with a live level under it, and the percentage.
    fn slider_row(
        &self,
        ui: &mut Ui,
        id: egui::Id,
        volume: f32,
        muted: bool,
        peak: f32,
        accent: egui::Color32,
    ) -> RowChange {
        let mut change = RowChange::default();
        ui.horizontal(|ui| {
            if mute_button(ui, muted, vec2(34.0, 26.0)).clicked() {
                change.mute = Some(!muted);
            }
            let pct_w = 46.0;
            let slider_w = (ui.available_width() - pct_w - 8.0).max(60.0);
            let (rect, resp) =
                ui.allocate_exact_size(vec2(slider_w, 26.0), Sense::click_and_drag());
            let mut v = volume;
            if let Some(p) = resp.interact_pointer_pos() {
                if resp.dragged() || resp.is_pointer_button_down_on() {
                    v = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                }
            }
            if resp.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0 {
                    v = (v + scroll.signum() * 0.02).clamp(0.0, 1.0);
                }
            }
            if (v - volume).abs() > f32::EPSILON {
                change.volume = Some(v);
            }
            paint_h_slider(ui, rect, id, v, muted, peak, accent);
            let pct = if muted {
                RichText::new("muted").size(11.0).color(theme::MUTE)
            } else {
                RichText::new(format!("{}%", crate::model::percent(v)))
                    .size(13.0)
                    .monospace()
                    .color(theme::TEXT)
            };
            ui.label(pct);
        });
        change
    }
}

/// Horizontal slider with a thin live meter underneath, drawn by hand so the meter can share the
/// track.
fn paint_h_slider(
    ui: &Ui,
    rect: egui::Rect,
    id: egui::Id,
    v: f32,
    muted: bool,
    peak: f32,
    accent: egui::Color32,
) {
    use egui::{pos2, CornerRadius, Rect};
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 16.0, 8.0));
    p.rect_filled(track, CornerRadius::same(4), theme::METER_BG);
    let color = if muted { theme::TEXT_DIM } else { accent };
    let knob_x = track.left() + track.width() * v;
    let fill = Rect::from_min_max(track.min, pos2(knob_x, track.bottom()));
    p.rect_filled(fill, CornerRadius::same(4), color);

    // Live level: a thin bar under the track, smoothed a little so it doesn't flicker.
    let level = if muted {
        0.0
    } else {
        crate::model::db_to_meter(crate::model::amp_to_db(peak))
    };
    let smooth_id = id.with("lvl");
    let prev = ui.data(|d| d.get_temp::<f32>(smooth_id)).unwrap_or(0.0);
    let shown = if level > prev {
        level
    } else {
        prev - (prev - level) * 0.25
    };
    ui.data_mut(|d| d.insert_temp(smooth_id, shown));
    let lvl_rect = Rect::from_min_size(
        pos2(track.left(), track.bottom() + 5.0),
        vec2(track.width() * shown, 2.0),
    );
    p.rect_filled(
        lvl_rect,
        CornerRadius::same(1),
        theme::METER_GREEN.linear_multiply(0.8),
    );

    let knob = pos2(knob_x, track.center().y);
    p.circle_filled(knob, 8.5, theme::TEXT);
    p.circle_stroke(knob, 8.5, egui::Stroke::new(2.0, color));
}
