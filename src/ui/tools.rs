//! The bus equaliser editor, compressor, ducking controls, and AutoEQ loader.

use eframe::egui::{self, pos2, vec2, Align, Color32, Layout, RichText, Sense, Stroke, Ui};

use super::console::{db_to_y, hz_to_x, x_to_hz, y_to_db, EQ_DB_RANGE};
use super::{strip, theme, MixerApp};
use crate::engine::autoeq;
use crate::engine::dsp::param_eq_response_db;
use crate::engine::params::{
    BandKind, BusParams, CompParams, ParamEqParams, BUS_EQ_BANDS, BUSES, HARDWARE_STRIPS, STRIPS,
};
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

/// Bus-wide compressor section in the bus editor.
fn bus_comp_editor(ui: &mut Ui, c: &mut CompParams, gain_reduction_db: f32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Compressor").strong());
        ui.checkbox(&mut c.enabled, "Enable");
        if c.enabled {
            ui.label(
                RichText::new(format!("reducing {:.1} dB", -gain_reduction_db))
                    .size(12.0)
                    .color(theme::TEXT_DIM()),
            );
        }
    });
    if !c.enabled {
        return;
    }
    ui.label(
        RichText::new(
            "Evens out the summed mix: anything above the threshold is turned down by the ratio.",
        )
        .size(11.0)
        .color(theme::TEXT_DIM()),
    );
    // Gain-reduction meter: grows to the left from zero.
    let (rect, _) = ui.allocate_exact_size(vec2(260.0, 10.0), Sense::hover());
    ui.painter().rect_filled(rect, 4.0, theme::METER_BG());
    let frac = (-gain_reduction_db / 24.0).clamp(0.0, 1.0);
    let fill = egui::Rect::from_min_max(
        egui::pos2(rect.right() - rect.width() * frac, rect.top()),
        rect.max,
    );
    ui.painter().rect_filled(fill, 4.0, theme::METER_YELLOW());
    ui.add_space(4.0);
    ui.add(
        egui::Slider::new(&mut c.threshold_db, -60.0..=0.0)
            .text("Threshold")
            .suffix(" dB"),
    );
    ui.add(
        egui::Slider::new(&mut c.ratio, 1.0..=20.0)
            .text("Ratio")
            .suffix(" : 1"),
    );
    ui.add(
        egui::Slider::new(&mut c.attack_ms, 0.5..=100.0)
            .logarithmic(true)
            .text("Attack")
            .suffix(" ms"),
    );
    ui.add(
        egui::Slider::new(&mut c.release_ms, 10.0..=1000.0)
            .logarithmic(true)
            .text("Release")
            .suffix(" ms"),
    );
    ui.add(
        egui::Slider::new(&mut c.makeup_db, 0.0..=24.0)
            .text("Make-up gain")
            .suffix(" dB"),
    );
}

/// AutoEQ profile loader: paste a path, click Load, or drag-and-drop a .txt file.
fn autoeq_section(ui: &mut Ui, b: usize, p: &mut BusParams) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("AutoEQ").strong());
        ui.label(
            RichText::new(
                "Load a headphone EQ profile from AutoEQ (autoeq.app or headphonedb.com).",
            )
            .size(11.0)
            .color(theme::TEXT_DIM()),
        );
    });

    // --- drag-and-drop: check for a dropped .txt file ---
    let dropped_path = ui.ctx().input(|i| {
        i.raw
            .dropped_files
            .iter()
            .find(|f| {
                f.path
                    .as_ref()
                    .map_or(false, |p| p.extension().map_or(false, |e| e.eq_ignore_ascii_case("txt")))
            })
            .and_then(|f| f.path.clone())
    });
    let hovering_txt = ui.ctx().input(|i| {
        !i.raw.hovered_files.is_empty()
    });

    let path_id = egui::Id::new(("autoeq-path", b));
    let note_id = egui::Id::new(("autoeq-note", b));
    let mut path: String = ui.data(|d| d.get_temp(path_id).unwrap_or_default());

    let load_path: Option<std::path::PathBuf> = if let Some(dp) = dropped_path {
        Some(dp)
    } else {
        None
    };

    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut path)
                .hint_text("Path to ParametricEQ.txt …")
                .desired_width(300.0),
        );
        if ui
            .button("Load")
            .on_hover_text("Read the file and apply its filters to this bus's equalizer")
            .clicked()
            && !path.is_empty()
        {
            let path_copy = path.clone();
            match std::fs::read_to_string(&path_copy) {
                Ok(text) => match autoeq::parse(&text) {
                    Ok((preamp_db, eq)) => {
                        p.eq = eq;
                        ui.data_mut(|d| d.insert_temp::<String>(path_id, String::new()));
                        ui.data_mut(|d| {
                            d.insert_temp(
                                note_id,
                                format!(
                                    "Applied. Preamp: {preamp_db:+.1} dB — adjust the bus fader to match."
                                ),
                            )
                        });
                    }
                    Err(e) => {
                        ui.data_mut(|d| d.insert_temp(note_id, format!("Error: {e}")));
                    }
                },
                Err(e) => {
                    ui.data_mut(|d| d.insert_temp(note_id, format!("Could not read file: {e}")));
                }
            }
        }
        if hovering_txt {
            ui.label(
                RichText::new("Drop to load")
                    .size(11.0)
                    .color(theme::ACCENT()),
            );
        } else {
            ui.label(
                RichText::new("or drag a .txt file here")
                    .size(11.0)
                    .color(theme::TEXT_DIM()),
            );
        }
    });
    ui.data_mut(|d| d.insert_temp(path_id, path));

    // Apply a dropped file.
    if let Some(dp) = load_path {
        match std::fs::read_to_string(&dp) {
            Ok(text) => match autoeq::parse(&text) {
                Ok((preamp_db, eq)) => {
                    p.eq = eq;
                    ui.data_mut(|d| {
                        d.insert_temp(
                            note_id,
                            format!(
                                "Applied {}. Preamp: {preamp_db:+.1} dB — adjust the bus fader to match.",
                                dp.file_name().and_then(|n| n.to_str()).unwrap_or("file")
                            ),
                        )
                    });
                }
                Err(e) => {
                    ui.data_mut(|d| d.insert_temp(note_id, format!("Error: {e}")));
                }
            },
            Err(e) => {
                ui.data_mut(|d| d.insert_temp(note_id, format!("Could not read file: {e}")));
            }
        }
    }

    if let Some(note) = ui.data(|d| d.get_temp::<String>(note_id)) {
        ui.label(
            RichText::new(&note)
                .size(11.0)
                .color(if note.starts_with("Error") || note.starts_with("Could") {
                    theme::NOTE_TEXT()
                } else {
                    theme::METER_GREEN()
                }),
        );
    }
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
    /// The editor panel for one output bus.
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
                // Title row.
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.config.bus_title(b))
                            .size(13.0)
                            .strong()
                            .color(accent),
                    );
                    // Which strips are feeding this bus.
                    let ep = self.engine.params();
                    let fed_by: Vec<String> = (0..STRIPS)
                        .filter(|&i| ep.strips[i].sends[b])
                        .map(|i| self.config.strip_title(i))
                        .collect();
                    if !fed_by.is_empty() {
                        let summary = fed_by.join(", ");
                        let label = strip::truncate(ui, &format!("← {summary}"), 320.0, 11.0);
                        ui.label(
                            RichText::new(label)
                                .size(11.0)
                                .color(theme::TEXT_DIM()),
                        )
                        .on_hover_text(format!("Feeds from: {summary}"));
                    } else {
                        ui.label(
                            RichText::new("← nothing feeds this bus")
                                .size(11.0)
                                .color(theme::NOTE_TEXT()),
                        )
                        .on_hover_text("No input strips are sending to this bus");
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.bus_editing = None;
                        }
                    });
                });
                // Tab bar — matches the strip editor's look.
                let tab_id = egui::Id::new(("bus-tab", b));
                let mut tab: usize = ui.data(|d| d.get_temp(tab_id).unwrap_or(0));
                ui.horizontal(|ui| {
                    ui.add_space(2.0);
                    let stereo_on = p.crossfeed > 0.005 || (p.width - 1.0).abs() > 0.05;
                    for (i, (name, active)) in [
                        ("Equalizer", p.eq.enabled),
                        ("AutoEQ", false),
                        ("Compressor", p.comp.enabled),
                        ("Stereo", stereo_on),
                    ]
                    .iter()
                    .enumerate()
                    {
                        let label = if *active {
                            RichText::new(*name).color(if tab == i {
                                theme::ON_ACCENT()
                            } else {
                                theme::ACCENT()
                            })
                        } else {
                            RichText::new(*name)
                        };
                        if ui.selectable_label(tab == i, label).clicked() {
                            tab = i;
                        }
                    }
                });
                ui.data_mut(|d| d.insert_temp(tab_id, tab));
                ui.separator();
                match tab {
                    1 => autoeq_section(ui, b, p),
                    2 => {
                        let gr = self.engine.levels().bus_comp_gain_reduction_db(b);
                        bus_comp_editor(ui, &mut p.comp, gr);
                    }
                    3 => spatial_controls(ui, p),
                    _ => {
                        let mut band = self.eq_band;
                        bus_eq_editor(ui, &mut p.eq, &mut band, accent);
                        self.eq_band = band;
                    }
                }
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

pub(super) fn format_clock(seconds: f64) -> String {
    let s = seconds as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn clock(seconds: f64) -> String {
    format_clock(seconds)
}

impl MixerApp {
    /// Start or stop recording `bus`. `bus: None` uses the saved preference.
    pub(super) fn set_recording(&mut self, on: bool, bus: Option<usize>) {
        if let Some(b) = bus {
            self.config.record_bus = b.min(BUSES - 1);
            self.config.save();
        }
        let bus = self.config.record_bus.min(BUSES - 1);
        if !on {
            if let Some((path, secs)) = self.engine.stop_bus_recording(bus) {
                self.recording_note = Some(format!("Saved {} ({})", path.display(), clock(secs)));
            }
            return;
        }
        if self.engine.recording_for_bus(bus).is_some() {
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
        let bus = self.config.record_bus.min(BUSES - 1);
        let active = self.engine.recording_for_bus(bus);
        let all_active = self.engine.all_recordings();
        let label = match &active {
            Some((_, secs)) => format!("Stop  {}", clock(*secs)),
            None => {
                if !all_active.is_empty() {
                    format!("Record  ({}●)", all_active.len())
                } else {
                    "Record".to_string()
                }
            }
        };
        let red = theme::MUTE();
        let resp = strip::toggle(ui, &label, active.is_some(), vec2(102.0, 30.0), red)
            .on_hover_text(match &active {
                Some((path, _)) => format!("Recording to {}\nClick to stop", path.display()),
                None => "Record one output to a WAV file".to_string(),
            });
        if resp.clicked() {
            self.set_recording(active.is_none(), None);
        }
        if active.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
        // Which output to capture; also shows all active recordings.
        let pick = strip::chip(ui, &self.config.bus_title(bus), 62.0, false)
            .on_hover_text("Which output to record");
        egui::Popup::menu(&pick).show(|ui| {
            ui.set_min_width(200.0);
            strip::caption(ui, "RECORD THIS OUTPUT");
            for b in 0..BUSES {
                let recording_this = self.engine.recording_for_bus(b).is_some();
                let label = if recording_this {
                    format!("● {}", self.config.bus_title(b))
                } else {
                    self.config.bus_title(b)
                };
                if ui.selectable_label(b == bus, label).clicked() {
                    self.config.record_bus = b;
                    self.config.save();
                    ui.close();
                }
            }
            if !all_active.is_empty() {
                ui.separator();
                strip::caption(ui, "ACTIVE RECORDINGS");
                let stops: Vec<usize> = all_active.iter().map(|(b, _, _)| *b).collect();
                for (b, path, secs) in &all_active {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "● {} {} — {}",
                                self.config.bus_title(*b),
                                clock(*secs),
                                path.file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("?")
                            ))
                            .size(11.0)
                            .color(theme::MUTE()),
                        );
                    });
                }
                if ui.small_button("Stop all").clicked() {
                    for b in stops {
                        if let Some((path, secs)) = self.engine.stop_bus_recording(b) {
                            self.recording_note =
                                Some(format!("Saved {} ({})", path.display(), clock(secs)));
                        }
                    }
                    ui.close();
                }
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
