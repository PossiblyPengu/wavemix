//! The bus equaliser editor and the ducking controls.

use eframe::egui::{self, pos2, vec2, Align, Color32, Layout, RichText, Sense, Stroke, Ui};

use super::console::{db_to_y, hz_to_x, x_to_hz, y_to_db, EQ_DB_RANGE};
use super::{strip, theme, MixerApp};
use crate::engine::dsp::param_eq_response_db;
use crate::engine::params::{BandKind, ParamEqParams, BUS_EQ_BANDS, HARDWARE_STRIPS, STRIPS};
use crate::engine::runtime::ENGINE_RATE;

/// Where a band's handle sits: its frequency, and its gain (filters that only cut sit on 0 dB).
fn handle_pos(rect: egui::Rect, p: &ParamEqParams, band: usize) -> egui::Pos2 {
    let b = &p.bands[band];
    let gain = if b.kind.has_gain() { b.gain_db } else { 0.0 };
    pos2(hz_to_x(rect, b.freq), db_to_y(rect, gain))
}

/// The graph and the controls for the selected band.
fn bus_eq_editor(ui: &mut Ui, p: &mut ParamEqParams, selected: &mut usize, accent: Color32) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut p.enabled, "Enable equalizer");
        ui.label(
            RichText::new(
                "Drag a dot: sideways for frequency, up and down for gain. Scroll on it for width.",
            )
            .size(11.0)
            .color(theme::TEXT_DIM()),
        );
        if ui.small_button("Reset").clicked() {
            *p = ParamEqParams {
                enabled: p.enabled,
                ..ParamEqParams::default()
            };
        }
    });
    ui.add_space(4.0);

    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 170.0), Sense::click_and_drag());
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, theme::METER_BG());
    for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
        let x = hz_to_x(rect, hz);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, theme::BORDER()),
        );
        let label = if hz >= 1000.0 {
            format!("{}k", hz / 1000.0)
        } else {
            format!("{hz}")
        };
        painter.text(
            pos2(x + 2.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(9.0),
            theme::TEXT_DIM(),
        );
    }
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = db_to_y(rect, db);
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(if db == 0.0 { 1.5 } else { 1.0 }, theme::BORDER()),
        );
        painter.text(
            pos2(rect.left() + 3.0, y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:+.0}"),
            egui::FontId::proportional(9.0),
            theme::TEXT_DIM(),
        );
    }
    let curve_color = if p.enabled { accent } else { theme::TEXT_DIM() };
    let pts: Vec<egui::Pos2> = (0..=240)
        .map(|k| {
            let x = rect.left() + rect.width() * k as f32 / 240.0;
            pos2(
                x,
                db_to_y(
                    rect,
                    param_eq_response_db(p, ENGINE_RATE as f32, x_to_hz(rect, x)),
                ),
            )
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(2.0, curve_color)));

    // Choosing and dragging dots.
    let drag_id = resp.id.with("dragging");
    let nearest = |pt: egui::Pos2, p: &ParamEqParams| {
        (0..BUS_EQ_BANDS)
            .map(|k| (k, handle_pos(rect, p, k).distance(pt)))
            .filter(|(_, d)| *d < 24.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(k, _)| k)
    };
    if resp.drag_started() || resp.clicked() {
        if let Some(k) = resp.interact_pointer_pos().and_then(|pt| nearest(pt, p)) {
            *selected = k;
            ui.data_mut(|d| d.insert_temp(drag_id, Some(k)));
        } else {
            ui.data_mut(|d| d.insert_temp(drag_id, None::<usize>));
        }
    }
    if resp.dragged() {
        let which: Option<usize> = ui.data(|d| d.get_temp::<Option<usize>>(drag_id)).flatten();
        if let (Some(k), Some(pt)) = (which, resp.interact_pointer_pos()) {
            let b = &mut p.bands[k];
            b.freq = x_to_hz(rect, pt.x).clamp(20.0, 20_000.0);
            if b.kind.has_gain() {
                b.gain_db = (y_to_db(rect, pt.y) * 2.0).round() / 2.0;
            }
            b.on = true;
            p.enabled = true;
        }
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(k) = resp.hover_pos().and_then(|pt| nearest(pt, p)) {
                let b = &mut p.bands[k];
                b.q = (b.q * (1.0 + scroll.signum() * 0.08)).clamp(0.2, 10.0);
                *selected = k;
            }
        }
    }
    let painter = ui.painter();
    for k in 0..BUS_EQ_BANDS {
        let at = handle_pos(rect, p, k);
        let b = &p.bands[k];
        let colour = if b.on { curve_color } else { theme::TEXT_DIM() };
        let radius = if k == *selected { 9.0 } else { 7.0 };
        painter.circle_filled(at, radius, colour);
        painter.circle_stroke(at, radius, Stroke::new(2.0, theme::BG()));
        painter.text(
            at,
            egui::Align2::CENTER_CENTER,
            format!("{}", k + 1),
            egui::FontId::proportional(9.0),
            theme::ON_ACCENT(),
        );
    }

    // The selected band's numbers.
    ui.add_space(6.0);
    *selected = (*selected).min(BUS_EQ_BANDS - 1);
    let b = &mut p.bands[*selected];
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Band {}", *selected + 1)).strong());
        ui.checkbox(&mut b.on, "On");
        egui::ComboBox::from_id_salt("band-kind")
            .selected_text(b.kind.label())
            .show_ui(ui, |ui| {
                for kind in BandKind::ALL {
                    ui.selectable_value(&mut b.kind, kind, kind.label());
                }
            });
        ui.add(
            egui::DragValue::new(&mut b.freq)
                .range(20.0..=20_000.0)
                .suffix(" Hz")
                .speed(5.0),
        );
        if b.kind.has_gain() {
            ui.add(
                egui::DragValue::new(&mut b.gain_db)
                    .range(-EQ_DB_RANGE..=EQ_DB_RANGE)
                    .suffix(" dB")
                    .speed(0.1),
            );
        }
        if matches!(
            b.kind,
            BandKind::Peak | BandKind::HighPass | BandKind::LowPass
        ) {
            ui.add(
                egui::DragValue::new(&mut b.q)
                    .range(0.2..=10.0)
                    .prefix("Q ")
                    .speed(0.02),
            );
        }
    });
}

/// Headphone crossfeed and stereo width, for one output.
fn spatial_controls(ui: &mut Ui, p: &mut crate::engine::params::BusParams) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Stereo").strong());
        ui.label(
            RichText::new("Crossfeed makes headphones sound less inside the head; width narrows or widens the picture.")
                .size(11.0)
                .color(theme::TEXT_DIM()),
        );
        if ui.small_button("Reset").clicked() {
            p.crossfeed = 0.0;
            p.width = 1.0;
        }
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::Slider::new(&mut p.crossfeed, 0.0..=1.0)
                .custom_formatter(|v, _| {
                    if v < 0.005 {
                        "off".to_string()
                    } else {
                        format!("{:.0}%", v * 100.0)
                    }
                })
                .text("Crossfeed"),
        );
        ui.add_space(12.0);
        ui.add(
            egui::Slider::new(&mut p.width, 0.0..=2.0)
                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                .text("Width"),
        );
    });
}

impl MixerApp {
    /// The editor panel for one output bus's equaliser.
    pub(super) fn bus_editor(
        &mut self,
        ui: &mut Ui,
        b: usize,
        p: &mut crate::engine::params::BusParams,
    ) {
        let accent = super::console::bus_colors()[b];
        egui::Frame::new()
            .fill(theme::PANEL())
            .stroke(Stroke::new(1.0, theme::BORDER()))
            .corner_radius(theme::R_CARD)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} equalizer", self.config.bus_title(b)))
                            .size(13.0)
                            .strong()
                            .color(accent),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.bus_editing = None;
                        }
                    });
                });
                ui.separator();
                let mut band = self.eq_band;
                bus_eq_editor(ui, &mut p.eq, &mut band, accent);
                self.eq_band = band;
                ui.add_space(8.0);
                ui.separator();
                spatial_controls(ui, p);
            });
    }

    /// The "ChatMix" button in the engine bar: one dial that trades game sound for chat sound.
    pub(super) fn chatmix_button(&mut self, ui: &mut Ui) {
        let mut params = self.engine.params();
        let on = params.chatmix.enabled;
        let resp = strip::toggle(ui, "ChatMix", on, vec2(82.0, 30.0), theme::ACCENT())
            .on_hover_text("One dial that turns game sound down as you favour chat, and back");
        let before = params.chatmix;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(300.0);
            ui.checkbox(&mut params.chatmix.enabled, "Balance game and chat");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Game is");
                strip_picker(ui, "chatmix-game", &mut params.chatmix.game, &self.config);
            });
            ui.horizontal(|ui| {
                ui.label("Chat is");
                strip_picker(ui, "chatmix-chat", &mut params.chatmix.chat, &self.config);
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Game").size(11.0).color(theme::TEXT_DIM()));
                ui.spacing_mut().slider_width = 170.0;
                ui.add(
                    egui::Slider::new(&mut params.chatmix.balance, -1.0..=1.0).show_value(false),
                );
                ui.label(RichText::new("Chat").size(11.0).color(theme::TEXT_DIM()));
                if ui.small_button("Centre").clicked() {
                    params.chatmix.balance = 0.0;
                }
            });
            if params.chatmix.game == params.chatmix.chat {
                ui.label(
                    RichText::new("Pick two different inputs.")
                        .size(11.0)
                        .color(theme::NOTE_TEXT()),
                );
            }
        });
        if params.chatmix != before {
            self.commit_params(params);
        }
    }

    /// The "Ducking" button in the engine bar, with its settings in a menu.
    pub(super) fn duck_button(&mut self, ui: &mut Ui) {
        let mut params = self.engine.params();
        let on = params.duck.enabled;
        let resp = strip::toggle(ui, "Ducking", on, vec2(86.0, 30.0), theme::ACCENT())
            .on_hover_text("Turn music and app sound down while you talk");
        let before = params.duck;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(280.0);
            ui.checkbox(&mut params.duck.enabled, "Turn apps down while I talk");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Listen to");
                egui::ComboBox::from_id_salt("duck-source")
                    .selected_text(self.config.strip_title(params.duck.source))
                    .show_ui(ui, |ui| {
                        for i in 0..HARDWARE_STRIPS {
                            ui.selectable_value(
                                &mut params.duck.source,
                                i,
                                self.config.strip_title(i),
                            );
                        }
                    });
            });
            ui.add(
                egui::Slider::new(&mut params.duck.reduction_db, 3.0..=30.0)
                    .text("Turn down by (dB)"),
            );
            ui.add(
                egui::Slider::new(&mut params.duck.threshold_db, -60.0..=-10.0)
                    .text("When louder than (dB)"),
            );
            ui.add(
                egui::Slider::new(&mut params.duck.release_ms, 100.0..=2000.0)
                    .text("Bring back after (ms)"),
            );
            ui.label(
                RichText::new("Applies to the two virtual inputs, where app sound arrives.")
                    .size(11.0)
                    .color(theme::TEXT_DIM()),
            );
        });
        if params.duck != before {
            self.commit_params(params);
        }
    }
}

/// A drop-down that picks one of the mixer's input strips.
fn strip_picker(ui: &mut Ui, id: &str, value: &mut usize, config: &crate::config::Config) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(config.strip_title(*value))
        .show_ui(ui, |ui| {
            for i in 0..STRIPS {
                ui.selectable_value(value, i, config.strip_title(i));
            }
        });
}

/// "2026-10-25 14-03-07", from the local clock.
fn timestamp() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let t = unsafe {
            let mut t = std::mem::zeroed();
            GetLocalTime(&mut t);
            t
        };
        format!(
            "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        )
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!("{secs}")
    }
}

/// Where recordings go: Music\Wavemix, or the temp folder if there is no Music folder.
fn recordings_dir() -> std::path::PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join("Music"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
        .join("Wavemix")
}

fn clock(seconds: f64) -> String {
    let s = seconds as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

impl MixerApp {
    /// Start or stop recording. `bus` overrides the remembered choice of what to record.
    pub(super) fn set_recording(&mut self, on: bool, bus: Option<usize>) {
        if let Some(b) = bus {
            self.config.record_bus = b.min(crate::engine::params::BUSES - 1);
            self.config.save();
        }
        let bus = self.config.record_bus.min(crate::engine::params::BUSES - 1);
        if !on {
            if let Some((path, secs)) = self.engine.stop_recording() {
                self.recording_note = Some(format!("Saved {} ({})", path.display(), clock(secs)));
            }
            return;
        }
        if self.engine.recording().is_some() {
            return;
        }
        let path = recordings_dir().join(format!(
            "Wavemix {} {}.wav",
            self.config.bus_title(bus),
            timestamp()
        ));
        self.recording_note = match self.engine.start_recording(bus, path) {
            Ok(()) => None,
            Err(e) => Some(format!("Couldn't start recording: {e}")),
        };
    }

    /// Record button and the choice of what to record, in the engine bar.
    pub(super) fn record_button(&mut self, ui: &mut Ui) {
        let active = self.engine.recording();
        let bus = self.config.record_bus.min(crate::engine::params::BUSES - 1);
        let label = match &active {
            Some((_, _, secs)) => format!("Stop  {}", clock(*secs)),
            None => "Record".to_string(),
        };
        let red = theme::MUTE();
        let resp = strip::toggle(ui, &label, active.is_some(), vec2(92.0, 30.0), red)
            .on_hover_text(match &active {
                Some((_, path, _)) => format!("Recording to {}\nClick to stop", path.display()),
                None => "Record one output to a WAV file".to_string(),
            });
        if resp.clicked() {
            self.set_recording(active.is_none(), None);
        }
        if active.is_some() {
            // Keep the timer ticking.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        // Which output to capture.
        let pick = strip::chip(ui, &self.config.bus_title(bus), 62.0, false)
            .on_hover_text("Which output to record");
        egui::Popup::menu(&pick).show(|ui| {
            ui.set_min_width(180.0);
            strip::caption(ui, "RECORD THIS OUTPUT");
            for b in 0..crate::engine::params::BUSES {
                if ui
                    .selectable_label(b == bus, self.config.bus_title(b))
                    .clicked()
                    && active.is_none()
                {
                    self.config.record_bus = b;
                    self.config.save();
                    ui.close();
                }
            }
            if active.is_some() {
                ui.label(
                    RichText::new("Stop the recording to change it.")
                        .size(11.0)
                        .color(theme::TEXT_DIM()),
                );
            }
            ui.separator();
            if ui.button("Open the recordings folder").clicked() {
                let dir = recordings_dir();
                let _ = std::fs::create_dir_all(&dir);
                #[cfg(windows)]
                {
                    let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                }
                ui.close();
            }
        });
        if let Some(note) = &self.recording_note {
            strip::stat_chip(ui, note, theme::NOTE_TEXT());
        }
    }
}
