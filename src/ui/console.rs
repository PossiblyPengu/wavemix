//! The mixer console: input strips on the left, output buses on the right, a processing editor
//! underneath. Everything here edits `EngineParams` or the device assignments; the audio thread
//! picks the changes up on its next block.

use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Align, Color32, Layout, RichText, Sense, Stroke, Ui};

use super::strip::{self, CONSOLE_STRIP_W};
use super::{theme, MixerApp};
use crate::engine::dsp::eq_response_db;
use crate::engine::io::{same_cable, DeviceInfo};
use crate::engine::params::{
    bus_name, strip_name, EngineParams, EqParams, StripParams, BUSES, HARDWARE_STRIPS, STRIPS,
};
use crate::engine::runtime::{Devices, SlotStatus, Stats, Status, ENGINE_RATE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorTab {
    Eq,
    Gate,
    Comp,
}

/// Latency presets, in milliseconds per device link.
const LATENCY_CHOICES: [(&str, u32); 4] = [
    ("Low (20 ms)", 20),
    ("Normal (30 ms)", 30),
    ("Safe (60 ms)", 60),
    ("Max (100 ms)", 100),
];

const BUS_COLORS: [Color32; BUSES] = [
    theme::ACCENT_OUT,
    theme::ACCENT_OUT,
    theme::ACCENT_OUT,
    theme::ACCENT_IN,
    theme::ACCENT_IN,
];

/// Height of an input strip's controls (everything except the fader), plus the panel's own
/// title row and padding. The fader takes whatever the window has left.
const STRIP_CHROME_H: f32 = 190.0;
const PANEL_CHROME_H: f32 = 52.0;
const MIN_FADER_H: f32 = 150.0;
const MAX_FADER_H: f32 = 460.0;
/// Room the processing editor needs underneath when it's open.
const EDITOR_H: f32 = 300.0;

/// How long to wait after the last change before writing settings to disk.
const SAVE_DELAY: Duration = Duration::from_millis(600);

/// A titled, rounded region that groups related strips.
fn panel(ui: &mut Ui, title: &str, subtitle: &str, min_h: f32, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::PANEL)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                // Panels share a height so they line up and reach the same bottom edge.
                ui.set_min_height(min_h);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(title)
                            .size(11.0)
                            .strong()
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(RichText::new(subtitle).size(11.0).color(theme::TEXT_DIM));
                });
                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    body(ui);
                });
            });
        });
}

fn db_text(db: f32) -> String {
    if db <= -59.9 {
        "-inf dB".to_string()
    } else {
        format!("{db:+.1} dB")
    }
}

impl MixerApp {
    pub(super) fn console(&mut self, ui: &mut Ui) {
        self.auto_configure();
        let mut params = self.engine.params();
        let before = params.clone();
        let devices = self.engine.devices();
        let status = self.engine.status();
        let stats = self.engine.stats();

        self.engine_bar(ui, &stats, &devices);
        ui.add_space(8.0);

        // Use the whole window: the faders take the height left over once the controls and (if
        // open) the editor have what they need.
        let editor = if self.editing.is_some() {
            EDITOR_H
        } else {
            0.0
        };
        let fader_h = (ui.available_height() - STRIP_CHROME_H - PANEL_CHROME_H - editor)
            .clamp(MIN_FADER_H, MAX_FADER_H);
        let panel_h = STRIP_CHROME_H + fader_h;

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    panel(ui, "INPUTS", "hardware and virtual", panel_h, |ui| {
                        for i in 0..STRIPS {
                            self.input_strip(
                                ui,
                                i,
                                fader_h,
                                &mut params.strips[i],
                                &devices,
                                &status,
                            );
                        }
                    });
                    panel(
                        ui,
                        "OUTPUT BUSES",
                        "A = hardware, B = virtual",
                        panel_h,
                        |ui| {
                            for b in 0..BUSES {
                                self.bus_strip(ui, b, fader_h, &mut params, &devices, &status);
                            }
                        },
                    );
                });
                if let Some((i, tab)) = self.editing {
                    ui.add_space(10.0);
                    self.editor(ui, i, tab, &mut params.strips[i]);
                }
            });

        if params != before {
            self.commit_params(params);
        }
    }

    // --- engine-level controls -----------------------------------------------------------------

    fn engine_bar(&mut self, ui: &mut Ui, stats: &Stats, devices: &Devices) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let running = self.engine.is_running();
                    let (label, color) = if running {
                        ("ENGINE ON", theme::METER_GREEN)
                    } else {
                        ("ENGINE OFF", theme::TEXT_DIM)
                    };
                    if strip::toggle(ui, label, running, vec2(96.0, 28.0), color)
                        .on_hover_text(
                            "Start or stop the mixing engine. While it is off, no audio is processed.",
                        )
                        .clicked()
                    {
                        self.engine.set_running(!running);
                        self.config.engine.io.engine_on = !running;
                        self.mark_engine_dirty();
                    }
                    ui.add_space(8.0);
                    if running {
                        let latency = self.config.engine.latency_ms * 2 + 10;
                        ui.label(
                            RichText::new(format!(
                                "{} kHz   ~{} ms + device buffers   CPU {:.0}%",
                                ENGINE_RATE / 1000,
                                latency,
                                stats.cpu_percent
                            ))
                            .size(12.0)
                            .color(theme::TEXT_DIM),
                        );
                    } else {
                        ui.label(
                            RichText::new("Turn the engine on to start mixing")
                                .size(12.0)
                                .color(theme::TEXT_DIM),
                        );
                    }
                    let drops: u64 = stats.late_blocks
                        + stats.input_underruns.iter().sum::<u64>()
                        + stats.output_underruns.iter().sum::<u64>()
                        + stats.output_overruns.iter().sum::<u64>();
                    if drops > 0 {
                        ui.label(
                            RichText::new(format!("{drops} dropouts"))
                                .size(12.0)
                                .color(theme::METER_YELLOW),
                        )
                        .on_hover_text(
                            "Audio glitched. If the count keeps climbing, pick a safer latency.",
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let current = LATENCY_CHOICES
                            .iter()
                            .find(|(_, ms)| *ms == self.config.engine.latency_ms)
                            .map(|(n, _)| *n)
                            .unwrap_or("Custom");
                        egui::ComboBox::from_id_salt("latency")
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (name, ms) in LATENCY_CHOICES {
                                    if ui
                                        .selectable_label(self.config.engine.latency_ms == ms, name)
                                        .clicked()
                                    {
                                        self.config.engine.latency_ms = ms;
                                        self.engine.set_latency_ms(ms);
                                        self.mark_engine_dirty();
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Lower latency reacts faster but is more likely to glitch");
                        ui.label(RichText::new("Latency").size(12.0).color(theme::TEXT_DIM));
                        if !devices
                            .inputs
                            .iter()
                            .chain(devices.outputs.iter())
                            .any(|d| d.is_virtual)
                        {
                            ui.add_space(12.0);
                            ui.hyperlink_to(
                                "Get a virtual cable (VB-Cable, free)",
                                "https://vb-audio.com/Cable/",
                            )
                            .on_hover_text(
                                "Virtual cables let you feed apps into the mixer and send the mix to Discord, OBS and more",
                            );
                        }
                    });
                });
            });
    }

    /// Mark engine settings as changed; they're written to disk shortly after the last change.
    pub(super) fn mark_engine_dirty(&mut self) {
        self.engine_dirty_at = Some(Instant::now());
    }

    fn commit_params(&mut self, params: EngineParams) {
        self.engine.set_params(params.clone());
        self.config.engine.params = params;
        self.mark_engine_dirty();
    }

    /// Called every frame: write pending settings once things have been quiet for a moment.
    pub(super) fn flush_engine_config(&mut self, ctx: &egui::Context) {
        if let Some(at) = self.engine_dirty_at {
            if at.elapsed() >= SAVE_DELAY {
                self.config.save();
                self.engine_dirty_at = None;
            } else {
                ctx.request_repaint_after(SAVE_DELAY);
            }
        }
    }

    /// First run: pick sensible devices so the mixer works without any setup. A real microphone
    /// goes to Input 1 (and to B1, so chat apps hear it), the default speakers to A1, and if a
    /// virtual cable is installed it becomes Virtual 1 / B1.
    fn auto_configure(&mut self) {
        if self.config.engine.configured {
            return;
        }
        let devices = self.engine.devices();
        if devices.inputs.is_empty() && devices.outputs.is_empty() {
            return; // device list not published yet
        }
        let pick = |list: &[DeviceInfo], virtual_wanted: bool, hint: &str| -> Option<String> {
            list.iter()
                .filter(|d| d.is_virtual == virtual_wanted)
                .find(|d| {
                    (!hint.is_empty() && d.name.contains(hint)) || (hint.is_empty() && d.is_default)
                })
                .or_else(|| list.iter().find(|d| d.is_virtual == virtual_wanted))
                .map(|d| d.name.clone())
        };
        let mic = pick(&devices.inputs, false, "");
        let speakers = pick(&devices.outputs, false, "");
        let cable_out = pick(&devices.inputs, true, "CABLE Output");
        let cable_in = pick(&devices.outputs, true, "CABLE Input");

        let mut params = self.engine.params();
        // Hardware inputs stay off A1 so you don't hear your own mic; they go to B1 for chat apps.
        for s in params.strips.iter_mut().take(HARDWARE_STRIPS) {
            s.sends = [false, false, false, cable_in.is_some(), false];
        }
        for s in params.strips.iter_mut().skip(HARDWARE_STRIPS) {
            s.sends = [true, false, false, false, false];
        }
        self.commit_params(params);

        let io = &mut self.config.engine.io;
        io.inputs[0] = mic.clone();
        io.inputs[HARDWARE_STRIPS] = cable_out.clone();
        io.outputs[0] = speakers.clone();
        io.outputs[3] = cable_in.clone();
        self.engine.set_input(0, mic);
        self.engine.set_input(HARDWARE_STRIPS, cable_out);
        self.engine.set_output(0, speakers);
        self.engine.set_output(3, cable_in);
        self.config.engine.configured = true;
        self.mark_engine_dirty();
    }

    // --- device picking ------------------------------------------------------------------------

    /// A pill showing the assigned device; the popup lists what's available. Returns the new
    /// assignment if the user chose one (`Some(None)` means "no device").
    fn device_picker(
        &self,
        ui: &mut Ui,
        current: &Option<String>,
        list: &[DeviceInfo],
        want_virtual: bool,
    ) -> Option<Option<String>> {
        let text = match current {
            Some(n) => strip::short_device_name(n).to_string(),
            None => "Select device".to_string(),
        };
        let resp = strip::chip(ui, &text, CONSOLE_STRIP_W - 4.0, current.is_some()).on_hover_text(
            current
                .as_deref()
                .unwrap_or("Choose a device for this slot"),
        );
        let mut chosen = None;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(260.0);
            if ui.selectable_label(current.is_none(), "None").clicked() {
                chosen = Some(None);
                ui.close();
            }
            ui.separator();
            // The kind this slot is meant for goes first, so the likely choice is on top.
            for virtual_group in [want_virtual, !want_virtual] {
                let group: Vec<&DeviceInfo> = list
                    .iter()
                    .filter(|d| d.is_virtual == virtual_group)
                    .collect();
                if group.is_empty() {
                    continue;
                }
                ui.label(
                    RichText::new(if virtual_group {
                        "VIRTUAL CABLES"
                    } else {
                        "HARDWARE"
                    })
                    .size(10.0)
                    .color(theme::TEXT_DIM),
                );
                for d in group {
                    let label = format!(
                        "{}{}",
                        d.name,
                        if d.is_default { "  (default)" } else { "" }
                    );
                    if ui
                        .selectable_label(current.as_deref() == Some(d.name.as_str()), label)
                        .clicked()
                    {
                        chosen = Some(Some(d.name.clone()));
                        ui.close();
                    }
                }
            }
            ui.separator();
            if ui.small_button("Rescan devices").clicked() {
                self.engine.refresh_devices();
                ui.close();
            }
            if want_virtual && !list.iter().any(|d| d.is_virtual) {
                ui.separator();
                ui.label(
                    RichText::new("No virtual cable found.")
                        .size(11.0)
                        .color(theme::TEXT_DIM),
                );
                ui.hyperlink_to("Get VB-Cable (free)", "https://vb-audio.com/Cable/");
            }
        });
        chosen
    }

    fn status_dot(&self, ui: &mut Ui, status: &SlotStatus) {
        let (color, tip) = match status {
            SlotStatus::Running { rate } => (theme::METER_GREEN, format!("Running at {rate} Hz")),
            SlotStatus::Missing => (theme::METER_YELLOW, "Device not found".to_string()),
            SlotStatus::Error(e) => (theme::MUTE, format!("Couldn't open: {e}")),
            SlotStatus::Off => (theme::TEXT_DIM, "No device, or engine off".to_string()),
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        resp.on_hover_text(tip);
    }

    // --- strips --------------------------------------------------------------------------------

    fn input_strip(
        &mut self,
        ui: &mut Ui,
        i: usize,
        fader_h: f32,
        p: &mut StripParams,
        devices: &Devices,
        status: &Status,
    ) {
        let peaks = self.engine.levels().strip(i);
        let is_virtual_slot = i >= HARDWARE_STRIPS;
        let accent = if is_virtual_slot {
            theme::ACCENT_IN
        } else {
            theme::ACCENT
        };
        ui.vertical(|ui| {
            ui.set_width(CONSOLE_STRIP_W);
            ui.spacing_mut().item_spacing = vec2(4.0, 5.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(strip_name(i)).size(13.0).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.status_dot(ui, &status.inputs[i]);
                });
            });
            let current = self.config.engine.io.inputs[i].clone();
            if let Some(sel) = self.device_picker(ui, &current, &devices.inputs, is_virtual_slot) {
                self.config.engine.io.inputs[i] = sel.clone();
                self.engine.set_input(i, sel);
                self.mark_engine_dirty();
            }

            if let Some(db) = strip::db_fader(
                ui,
                super::wid(("in-fader", i)),
                fader_h,
                p.gain_db,
                peaks,
                accent,
                p.mute,
            ) {
                p.gain_db = db;
            }
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(db_text(p.gain_db))
                        .size(12.0)
                        .monospace()
                        .color(theme::TEXT),
                );
            });
            if let Some(v) =
                strip::balance(ui, super::wid(("pan", i)), p.pan, CONSOLE_STRIP_W - 8.0)
            {
                p.pan = v;
            }

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if strip::toggle(ui, "M", p.mute, vec2(34.0, 24.0), theme::MUTE)
                    .on_hover_text("Mute")
                    .clicked()
                {
                    p.mute = !p.mute;
                }
                if strip::toggle(ui, "S", p.solo, vec2(34.0, 24.0), theme::METER_YELLOW)
                    .on_hover_text("Solo: hear only soloed strips")
                    .clicked()
                {
                    p.solo = !p.solo;
                }
                if strip::toggle(ui, "Mono", p.mono, vec2(38.0, 24.0), theme::ACCENT)
                    .on_hover_text("Mix left and right into one")
                    .clicked()
                {
                    p.mono = !p.mono;
                }
            });

            // Which buses this strip feeds.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                for (b, bus_color) in BUS_COLORS.iter().copied().enumerate() {
                    // A strip that listens to a cable can't also feed the bus playing into it.
                    let loops = match (
                        &self.config.engine.io.inputs[i],
                        &self.config.engine.io.outputs[b],
                    ) {
                        (Some(inp), Some(out)) => same_cable(inp, out),
                        _ => false,
                    };
                    let (color, tip) = if loops && p.sends[b] {
                        (
                            theme::MUTE,
                            format!(
                                "Feedback loop! This strip listens to the cable {} plays into. Turn this send off.",
                                bus_name(b)
                            ),
                        )
                    } else if loops {
                        (
                            bus_color,
                            format!(
                                "Would cause a feedback loop: this strip listens to the cable {} plays into.",
                                bus_name(b)
                            ),
                        )
                    } else {
                        (bus_color, format!("Send to {}", bus_name(b)))
                    };
                    if strip::toggle(ui, bus_name(b), p.sends[b], vec2(20.0, 22.0), color)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        p.sends[b] = !p.sends[b];
                    }
                }
            });

            // Processing. Clicking opens the editor on that tab; the LED shows whether it's on.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                let mut want: Option<EditorTab> = None;
                for (tab, label, on, width) in [
                    (EditorTab::Eq, "EQ", p.eq.enabled, 31.0),
                    (EditorTab::Gate, "Gate", p.gate.enabled, 40.0),
                    (EditorTab::Comp, "Comp", p.comp.enabled, 42.0),
                ] {
                    let open = self.editing == Some((i, tab));
                    if strip::led_button(ui, label, on, open, vec2(width, 22.0)).clicked() {
                        want = Some(tab);
                    }
                }
                if let Some(tab) = want {
                    self.editing = if self.editing == Some((i, tab)) {
                        None
                    } else {
                        Some((i, tab))
                    };
                }
            });
        });
    }

    fn bus_strip(
        &mut self,
        ui: &mut Ui,
        b: usize,
        fader_h: f32,
        params: &mut EngineParams,
        devices: &Devices,
        status: &Status,
    ) {
        let peaks = self.engine.levels().bus(b);
        let p = &mut params.buses[b];
        ui.vertical(|ui| {
            ui.set_width(CONSOLE_STRIP_W);
            ui.spacing_mut().item_spacing = vec2(4.0, 5.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(bus_name(b))
                        .size(13.0)
                        .strong()
                        .color(BUS_COLORS[b]),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.status_dot(ui, &status.outputs[b]);
                });
            });
            let current = self.config.engine.io.outputs[b].clone();
            if let Some(sel) = self.device_picker(ui, &current, &devices.outputs, b >= 3) {
                self.config.engine.io.outputs[b] = sel.clone();
                self.engine.set_output(b, sel);
                self.mark_engine_dirty();
            }
            if let Some(db) = strip::db_fader(
                ui,
                super::wid(("bus-fader", b)),
                fader_h,
                p.gain_db,
                peaks,
                BUS_COLORS[b],
                p.mute,
            ) {
                p.gain_db = db;
            }
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(db_text(p.gain_db))
                        .size(12.0)
                        .monospace()
                        .color(theme::TEXT),
                );
            });
            ui.add_space(23.0); // lines the buttons up with the input strips' second row
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if strip::toggle(ui, "M", p.mute, vec2(34.0, 24.0), theme::MUTE)
                    .on_hover_text("Mute this bus")
                    .clicked()
                {
                    p.mute = !p.mute;
                }
                if strip::toggle(ui, "Limit", p.limiter, vec2(46.0, 24.0), theme::METER_GREEN)
                    .on_hover_text("Limiter: stops the summed mix from clipping")
                    .clicked()
                {
                    p.limiter = !p.limiter;
                }
            });
        });
    }

    // --- processing editor ---------------------------------------------------------------------

    fn editor(&mut self, ui: &mut Ui, i: usize, tab: EditorTab, p: &mut StripParams) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(strip_name(i)).size(13.0).strong());
                    ui.add_space(8.0);
                    for (t, label) in [
                        (EditorTab::Eq, "Equalizer"),
                        (EditorTab::Gate, "Noise gate"),
                        (EditorTab::Comp, "Compressor"),
                    ] {
                        if ui.selectable_label(tab == t, label).clicked() {
                            self.editing = Some((i, t));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.editing = None;
                        }
                    });
                });
                ui.separator();
                match tab {
                    EditorTab::Eq => eq_editor(ui, &mut p.eq),
                    EditorTab::Gate => {
                        let open = self.engine.levels().gate_open(i);
                        gate_editor(ui, p, open);
                    }
                    EditorTab::Comp => {
                        let gr = self.engine.levels().comp_gain_reduction_db(i);
                        comp_editor(ui, p, gr);
                    }
                }
            });
    }
}

// --- editors -----------------------------------------------------------------------------------

const EQ_DB_RANGE: f32 = 18.0;
const EQ_MIN_HZ: f32 = 20.0;
const EQ_MAX_HZ: f32 = 20_000.0;

fn hz_to_x(rect: egui::Rect, hz: f32) -> f32 {
    let t = (hz.clamp(EQ_MIN_HZ, EQ_MAX_HZ) / EQ_MIN_HZ).ln() / (EQ_MAX_HZ / EQ_MIN_HZ).ln();
    rect.left() + rect.width() * t
}

fn x_to_hz(rect: egui::Rect, x: f32) -> f32 {
    let t = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    EQ_MIN_HZ * (EQ_MAX_HZ / EQ_MIN_HZ).powf(t)
}

fn db_to_y(rect: egui::Rect, db: f32) -> f32 {
    rect.center().y - db.clamp(-EQ_DB_RANGE, EQ_DB_RANGE) / EQ_DB_RANGE * rect.height() / 2.0
}

fn y_to_db(rect: egui::Rect, y: f32) -> f32 {
    ((rect.center().y - y) / (rect.height() / 2.0) * EQ_DB_RANGE).clamp(-EQ_DB_RANGE, EQ_DB_RANGE)
}

fn eq_editor(ui: &mut Ui, p: &mut EqParams) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut p.enabled, "Enable equalizer");
        ui.label(
            RichText::new("Drag the dots: sideways for frequency, up and down for gain.")
                .size(11.0)
                .color(theme::TEXT_DIM),
        );
        if ui.small_button("Reset").clicked() {
            *p = EqParams {
                enabled: p.enabled,
                ..EqParams::default()
            };
        }
    });
    ui.add_space(4.0);

    let width = ui.available_width().min(680.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 170.0), Sense::click_and_drag());
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, theme::METER_BG);
    for hz in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0] {
        let x = hz_to_x(rect, hz);
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.0, theme::BORDER),
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
            theme::TEXT_DIM,
        );
    }
    for db in [-12.0, -6.0, 0.0, 6.0, 12.0] {
        let y = db_to_y(rect, db);
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(if db == 0.0 { 1.5 } else { 1.0 }, theme::BORDER),
        );
        painter.text(
            pos2(rect.left() + 3.0, y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:+.0}"),
            egui::FontId::proportional(9.0),
            theme::TEXT_DIM,
        );
    }

    // The combined response curve.
    let curve_color = if p.enabled {
        theme::ACCENT
    } else {
        theme::TEXT_DIM
    };
    let pts: Vec<egui::Pos2> = (0..=200)
        .map(|k| {
            let hz = x_to_hz(rect, rect.left() + rect.width() * k as f32 / 200.0);
            pos2(
                rect.left() + rect.width() * k as f32 / 200.0,
                db_to_y(rect, eq_response_db(p, ENGINE_RATE as f32, hz)),
            )
        })
        .collect();
    painter.add(egui::Shape::line(pts, Stroke::new(2.0, curve_color)));

    // Draggable handles: (frequency, gain, allowed frequency range).
    let handles = [
        (p.low_freq, p.low_gain_db, 30.0..=600.0),
        (p.mid_freq, p.mid_gain_db, 100.0..=10_000.0),
        (p.high_freq, p.high_gain_db, 1_500.0..=18_000.0),
    ];
    let screen: Vec<egui::Pos2> = handles
        .iter()
        .map(|(f, g, _)| pos2(hz_to_x(rect, *f), db_to_y(rect, *g)))
        .collect();

    let drag_id = resp.id.with("dragging");
    if resp.drag_started() {
        let which = resp.interact_pointer_pos().and_then(|pt| {
            screen
                .iter()
                .enumerate()
                .map(|(k, s)| (k, s.distance(pt)))
                .filter(|(_, d)| *d < 28.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k)
        });
        ui.data_mut(|d| d.insert_temp(drag_id, which));
    }
    if resp.dragged() {
        let which: Option<usize> = ui.data(|d| d.get_temp::<Option<usize>>(drag_id)).flatten();
        if let (Some(k), Some(pt)) = (which, resp.interact_pointer_pos()) {
            let db = (y_to_db(rect, pt.y) * 2.0).round() / 2.0;
            let range = handles[k].2.clone();
            let hz = x_to_hz(rect, pt.x).clamp(*range.start(), *range.end());
            match k {
                0 => {
                    p.low_freq = hz;
                    p.low_gain_db = db;
                }
                1 => {
                    p.mid_freq = hz;
                    p.mid_gain_db = db;
                }
                _ => {
                    p.high_freq = hz;
                    p.high_gain_db = db;
                }
            }
            p.enabled = true;
        }
    }
    // Scrolling over the mid band's dot changes its width.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(pt) = resp.hover_pos() {
                if screen[1].distance(pt) < 28.0 {
                    p.mid_q = (p.mid_q * (1.0 + scroll.signum() * 0.08)).clamp(0.2, 10.0);
                }
            }
        }
    }
    let painter = ui.painter();
    for (k, s) in screen.iter().enumerate() {
        let label = ["Low", "Mid", "High"][k];
        painter.circle_filled(*s, 7.0, curve_color);
        painter.circle_stroke(*s, 7.0, Stroke::new(2.0, theme::BG));
        painter.text(
            *s + vec2(0.0, -12.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            egui::FontId::proportional(10.0),
            theme::TEXT,
        );
    }

    ui.add_space(6.0);
    egui::Grid::new("eq-values")
        .num_columns(4)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("Low shelf");
            ui.add(
                egui::DragValue::new(&mut p.low_freq)
                    .range(30.0..=600.0)
                    .suffix(" Hz")
                    .speed(2.0),
            );
            ui.add(
                egui::DragValue::new(&mut p.low_gain_db)
                    .range(-EQ_DB_RANGE..=EQ_DB_RANGE)
                    .suffix(" dB")
                    .speed(0.1),
            );
            ui.label("");
            ui.end_row();
            ui.label("Mid");
            ui.add(
                egui::DragValue::new(&mut p.mid_freq)
                    .range(100.0..=10_000.0)
                    .suffix(" Hz")
                    .speed(5.0),
            );
            ui.add(
                egui::DragValue::new(&mut p.mid_gain_db)
                    .range(-EQ_DB_RANGE..=EQ_DB_RANGE)
                    .suffix(" dB")
                    .speed(0.1),
            );
            ui.add(
                egui::DragValue::new(&mut p.mid_q)
                    .range(0.2..=10.0)
                    .prefix("Q ")
                    .speed(0.02),
            );
            ui.end_row();
            ui.label("High shelf");
            ui.add(
                egui::DragValue::new(&mut p.high_freq)
                    .range(1_500.0..=18_000.0)
                    .suffix(" Hz")
                    .speed(20.0),
            );
            ui.add(
                egui::DragValue::new(&mut p.high_gain_db)
                    .range(-EQ_DB_RANGE..=EQ_DB_RANGE)
                    .suffix(" dB")
                    .speed(0.1),
            );
            ui.label("");
            ui.end_row();
        });
}

fn gate_editor(ui: &mut Ui, p: &mut StripParams, open: bool) {
    let g = &mut p.gate;
    ui.horizontal(|ui| {
        ui.checkbox(&mut g.enabled, "Enable noise gate");
        if g.enabled {
            let (text, color) = if open {
                ("open", theme::METER_GREEN)
            } else {
                ("closed", theme::TEXT_DIM)
            };
            ui.label(RichText::new(format!("gate {text}")).color(color));
        }
    });
    ui.label(
        RichText::new(
            "Silences the strip when it falls below the threshold, so background noise disappears between words.",
        )
        .size(11.0)
        .color(theme::TEXT_DIM),
    );
    ui.add_space(4.0);
    ui.add(
        egui::Slider::new(&mut g.threshold_db, -80.0..=-10.0)
            .text("Threshold")
            .suffix(" dB"),
    );
    ui.add(
        egui::Slider::new(&mut g.attack_ms, 0.1..=50.0)
            .logarithmic(true)
            .text("Attack")
            .suffix(" ms"),
    );
    ui.add(
        egui::Slider::new(&mut g.release_ms, 10.0..=1000.0)
            .logarithmic(true)
            .text("Release")
            .suffix(" ms"),
    );
}

fn comp_editor(ui: &mut Ui, p: &mut StripParams, gain_reduction_db: f32) {
    let c = &mut p.comp;
    ui.horizontal(|ui| {
        ui.checkbox(&mut c.enabled, "Enable compressor");
        if c.enabled {
            ui.label(
                RichText::new(format!("reducing {:.1} dB", -gain_reduction_db))
                    .size(12.0)
                    .color(theme::TEXT_DIM),
            );
        }
    });
    ui.label(
        RichText::new(
            "Evens out loud and quiet parts: anything above the threshold is turned down by the ratio.",
        )
        .size(11.0)
        .color(theme::TEXT_DIM),
    );
    // Gain-reduction meter: grows to the left from zero.
    let (rect, _) = ui.allocate_exact_size(vec2(260.0, 10.0), Sense::hover());
    ui.painter().rect_filled(rect, 4.0, theme::METER_BG);
    let frac = (-gain_reduction_db / 24.0).clamp(0.0, 1.0);
    let fill = egui::Rect::from_min_max(
        pos2(rect.right() - rect.width() * frac, rect.top()),
        rect.max,
    );
    ui.painter().rect_filled(fill, 4.0, theme::METER_YELLOW);
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
