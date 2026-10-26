//! The mixer console: input strips on the left, output buses on the right, a processing editor
//! underneath. Everything here edits `EngineParams` or the device assignments; the audio thread
//! picks the changes up on its next block.

use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Align, Color32, Layout, RichText, Sense, Stroke, Ui};

use super::strip::{self, CONSOLE_STRIP_W};
use super::{theme, MixerApp};
use crate::engine::dsp::eq_response_db;
use crate::engine::io::{feeds_itself, DeviceInfo};
use crate::engine::params::{
    bus_name, EngineParams, EqParams, StripParams, BUSES, HARDWARE_STRIPS, STRIPS,
};
use crate::engine::runtime::{Devices, SlotStatus, Stats, Status, ENGINE_RATE};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorTab {
    Eq,
    Gate,
    Comp,
    Denoise,
}

/// Latency presets, in milliseconds per device link.
const LATENCY_CHOICES: [(&str, u32); 5] = [
    ("Ultra (10 ms)", 10),
    ("Low (20 ms)", 20),
    ("Normal (30 ms)", 30),
    ("Safe (60 ms)", 60),
    ("Max (100 ms)", 100),
];

/// Bus colours: hardware outputs (A) in terracotta, virtual ones (B) in green.
pub(super) fn bus_colors() -> [Color32; BUSES] {
    [
        theme::ACCENT_OUT(),
        theme::ACCENT_OUT(),
        theme::ACCENT_OUT(),
        theme::ACCENT_IN(),
        theme::ACCENT_IN(),
    ]
}

/// Height of an input strip's controls (everything except the fader), plus the panel's own
/// title row and padding. The fader takes whatever the window has left.
const STRIP_CHROME_H: f32 = 262.0;
const PANEL_CHROME_H: f32 = 64.0;
const MIN_FADER_H: f32 = 150.0;
const MIN_FADER_H_EDITING: f32 = 90.0;
const MAX_FADER_H: f32 = 460.0;
/// Room the processing editor needs underneath when it's open.
const EDITOR_H: f32 = 300.0;

/// How long to wait after the last change before writing settings to disk.
const SAVE_DELAY: Duration = Duration::from_millis(600);

/// One mixer strip's card.
fn strip_card(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    theme::card_frame()
        .inner_margin(egui::Margin::symmetric(5, 10))
        .show(ui, |ui| {
            ui.vertical(body);
        });
}

/// Gap between the two console panels, and between strips inside a panel.
const PANEL_GAP: f32 = 12.0;
const STRIP_GAP: f32 = 6.0;
/// Width of one strip card: the strip itself plus the card's side padding.
const STRIP_CARD_W: f32 = CONSOLE_STRIP_W + 10.0;

/// A titled, rounded region that groups related strips. It is exactly `width` wide, with the
/// strips centred in it, so two panels side by side mirror each other.
fn panel(
    ui: &mut Ui,
    title: &str,
    subtitle: &str,
    width: f32,
    min_h: f32,
    body: impl FnOnce(&mut Ui),
) {
    theme::panel_frame().show(ui, |ui| {
        // Frame margins are 18 a side.
        let inner_w = (width - 36.0 - 2.0).max(STRIP_CARD_W);
        ui.set_width(inner_w);
        ui.vertical(|ui| {
            // Panels share a height so they line up and reach the same bottom edge.
            ui.set_min_height(min_h);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .size(11.0)
                        .strong()
                        .color(theme::TEXT_DIM()),
                );
                ui.label(RichText::new(subtitle).size(11.0).color(theme::TEXT_DIM()));
            });
            ui.add_space(10.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = STRIP_GAP;
                let used = 5.0 * STRIP_CARD_W + 4.0 * STRIP_GAP;
                ui.add_space(((inner_w - used) / 2.0).max(0.0));
                body(ui);
            });
        });
    });
}

/// The index after (or before) `current` in a ring of `len`; with nothing current, the first (or
/// last).
#[cfg_attr(not(windows), allow(dead_code))]
fn cycle_index(current: Option<usize>, len: usize, forward: bool) -> usize {
    match (current, forward) {
        (None, true) => 0,
        (None, false) => len - 1,
        (Some(i), true) => (i + 1) % len,
        (Some(i), false) => (i + len - 1) % len,
    }
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
        // The test click turns itself off after a while, so it can't be left on by accident.
        if self.click_until.is_some_and(|t| Instant::now() > t) {
            self.click_until = None;
            self.engine.set_test_click(false);
        }
        self.auto_configure();
        if self.editing.is_some()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.editing = None;
        }
        let mut params = self.engine.params();
        let before = params.clone();
        let devices = self.engine.devices();
        let status = self.engine.status();
        let stats = self.engine.stats();

        self.engine_bar(ui, &stats, &devices);
        // The bar can change this flag; don't let the copy taken above write the old value back.
        let bypass = self.engine.params().bypass_fx;
        params.bypass_fx = bypass;
        let before = EngineParams {
            bypass_fx: bypass,
            ..before
        };
        // Signal-flow routing matrix (shown when toggled on).
        let flow_id = egui::Id::new("show-signal-flow");
        if ui.data(|d| d.get_temp::<bool>(flow_id).unwrap_or(false)) {
            ui.add_space(6.0);
            self.signal_flow(ui, &mut params);
        }
        ui.add_space(8.0);

        // Use the whole window: the faders take the height left over once the controls and (if
        // open) the editor have what they need.
        let editing = self.editing.is_some() || self.bus_editing.is_some();
        let editor = if editing { EDITOR_H } else { 0.0 };
        // With an editor open the faders give way, so the editor fits without scrolling.
        let min_fader = if editing {
            MIN_FADER_H_EDITING
        } else {
            MIN_FADER_H
        };
        let fader_h = (ui.available_height() - STRIP_CHROME_H - PANEL_CHROME_H - editor)
            .clamp(min_fader, MAX_FADER_H);
        let panel_h = STRIP_CHROME_H + fader_h;

        // Measured out here: inside a scroll area the width is whatever the content asks for.
        let avail_w = ui.available_width();
        egui::ScrollArea::both()
            .id_salt("pro-console")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = PANEL_GAP;
                    // Two panels of exactly equal width; the strips sit centred in each.
                    let panel_w = ((avail_w - PANEL_GAP) / 2.0).floor();
                    panel(
                        ui,
                        "INPUTS",
                        "hardware and virtual",
                        panel_w,
                        panel_h,
                        |ui| {
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
                        },
                    );
                    panel(
                        ui,
                        "OUTPUT BUSES",
                        "A = hardware, B = virtual",
                        panel_w,
                        panel_h,
                        |ui| {
                            for b in 0..BUSES {
                                self.bus_strip(ui, b, fader_h, &mut params, &devices, &status);
                            }
                        },
                    );
                });
                if let Some((i, tab)) = self.editing {
                    // The editor eases in when it opens or moves to another strip.
                    if self.editor_strip != Some(i) {
                        self.editor_strip = Some(i);
                        self.editor_since = Instant::now();
                    }
                    let t = (self.editor_since.elapsed().as_secs_f32() / 0.2).min(1.0);
                    let t = t * t * (3.0 - 2.0 * t);
                    if t < 1.0 {
                        ui.ctx().request_repaint();
                    }
                    ui.add_space(10.0 + (1.0 - t) * 12.0);
                    ui.set_opacity(t);
                    self.editor(ui, i, tab, &mut params.strips[i]);
                } else {
                    self.editor_strip = None;
                }
                if let Some(b) = self.bus_editing {
                    ui.add_space(10.0);
                    self.bus_editor(ui, b, &mut params.buses[b]);
                }
            });

        if params != before {
            self.commit_params(params);
        }
    }

    // --- engine-level controls -----------------------------------------------------------------

    fn engine_bar(&mut self, ui: &mut Ui, stats: &Stats, devices: &Devices) {
        egui::Frame::new()
            .fill(theme::PANEL())
            .stroke(Stroke::new(1.0, theme::BORDER()))
            .corner_radius(theme::R_PANEL)
            .shadow(theme::shadow(14, 50))
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let running = self.engine.is_running();
                    let (label, color) = if running {
                        ("ENGINE ON", theme::METER_GREEN())
                    } else {
                        ("ENGINE OFF", theme::TEXT_DIM())
                    };
                    if strip::toggle(ui, label, running, vec2(104.0, 30.0), color)
                        .on_hover_text(
                            "Start or stop the mixing engine. While it is off, no audio is processed.",
                        )
                        .clicked()
                    {
                        self.engine.set_running(!running);
                        self.config.engine.io.engine_on = !running;
                        if running {
                            self.unroute_windows_audio();
                        }
                        self.mark_engine_dirty();
                    }
                    ui.add_space(6.0);
                    let drops: u64 = stats.late_blocks
                        + stats.input_underruns.iter().sum::<u64>()
                        + stats.output_underruns.iter().sum::<u64>()
                        + stats.output_overruns.iter().sum::<u64>();
                    if running {
                        let latency = self.config.engine.latency_ms * 2 + 10;
                        for text in [
                            format!("{} kHz", ENGINE_RATE / 1000),
                            format!("~{latency} ms"),
                            format!("CPU {:.0}%", stats.cpu_percent),
                        ] {
                            strip::stat_chip(ui, &text, theme::TEXT_DIM());
                        }
                        if drops > 0 {
                            strip::stat_chip(
                                ui,
                                &format!("{drops} dropouts"),
                                theme::NOTE_TEXT(),
                            )
                            .on_hover_text(format!(
                                "Audio glitched. Links buffer a little more after each dropout, so \
                                 it should settle; if the count keeps climbing, pick a safer latency.\n\n\
                                 mixer ran late: {}\ninput underruns: {:?}\noutput underruns: {:?}\noutput overruns: {:?}",
                                stats.late_blocks,
                                stats.input_underruns,
                                stats.output_underruns,
                                stats.output_overruns,
                            ));
                        }
                    } else {
                        ui.label(
                            RichText::new("Turn the engine on to start mixing")
                                .size(12.0)
                                .color(theme::TEXT_DIM()),
                        );
                    }
                    if running {
                        ui.add_space(8.0);
                        self.route_control(ui);
                    }
                    if running {
                        self.more_menu(ui);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Signal-flow toggle: a routing matrix below the bar.
                        let flow_id = egui::Id::new("show-signal-flow");
                        let flow_on: bool = ui.data(|d| d.get_temp(flow_id).unwrap_or(false));
                        if strip::toggle(ui, "Signal flow", flow_on, vec2(88.0, 26.0), theme::ACCENT())
                            .on_hover_text("Show a routing matrix: which inputs feed which outputs")
                            .clicked()
                        {
                            ui.data_mut(|d| d.insert_temp(flow_id, !flow_on));
                        }
                        ui.add_space(8.0);
                        // Show or hide the per-app list beside the console, growing or shrinking
                        // the window by its width so the console keeps its room.
                        let on = self.config.pro_apps_panel;
                        if strip::toggle(ui, "Apps panel", on, vec2(84.0, 26.0), theme::ACCENT())
                            .on_hover_text("List running apps beside the mixer, to send them into virtual cables")
                            .clicked()
                        {
                            self.config.pro_apps_panel = !on;
                            let grow = super::SIDE_PANEL_W + super::SIDE_PANEL_GAP;
                            let width = ui.ctx().input(|i| i.viewport().inner_rect.map(|r| r.width()));
                            if let Some(w) = width {
                                let w = if on { (w - grow).max(900.0) } else { w + grow };
                                let h = ui.ctx().input(|i| i.viewport().inner_rect.map(|r| r.height())).unwrap_or(840.0);
                                ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
                            }
                            self.mark_engine_dirty();
                        }
                        ui.add_space(8.0);
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
                        ui.label(RichText::new("Latency").size(12.0).color(theme::TEXT_DIM()));
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

    /// Whether Windows already plays into the mixer, and the playback device that would do it:
    /// the other end of the cable that Virtual 1 listens to. Other virtual devices (Voicemeeter's,
    /// say) don't count, since nothing in wavemix listens to them.
    pub(super) fn route_state(&self) -> (bool, Option<crate::model::Device>) {
        let snap = self.backend.snapshot();
        // Before the mixer has been set up, guess the same cable it will choose.
        let listening = self.config.engine.io.inputs[HARDWARE_STRIPS]
            .clone()
            .or_else(|| {
                self.engine
                    .devices()
                    .inputs
                    .iter()
                    .find(|d| d.is_virtual && d.name.contains("CABLE Output"))
                    .map(|d| d.name.clone())
            });
        let cable = listening.and_then(|input| {
            snap.devices
                .iter()
                .find(|d| {
                    d.flow == crate::model::Flow::Render
                        && crate::engine::io::same_cable(&input, &d.name)
                })
                .cloned()
        });
        let routed = cable.as_ref().is_some_and(|c| c.is_default);
        (routed, cable)
    }

    /// One button that sends everything Windows plays through the mixer (by making the virtual
    /// cable the default playback device), and puts the old device back when switched off.
    /// "More ▾" chip: Ducking, ChatMix, Record, Macros and Bypass FX in one drop-down.
    /// Keeps the engine bar from overflowing on a normal 1360-wide window.
    fn more_menu(&mut self, ui: &mut Ui) {
        let recording = self.engine.recording().is_some();
        let p = self.engine.params();
        let any_on = recording || p.duck.enabled || p.chatmix.enabled || p.bypass_fx;
        let label = if any_on { "More •" } else { "More" };
        let resp = strip::chip(ui, label, 72.0, any_on)
            .on_hover_text("Ducking, ChatMix, Record, Macros, Bypass FX");
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(220.0);
            // --- Bypass FX ---
            let mut p = self.engine.params();
            if ui
                .selectable_label(p.bypass_fx, "Bypass FX")
                .on_hover_text("Skip every strip's gate, EQ and compressor")
                .clicked()
            {
                p.bypass_fx = !p.bypass_fx;
                self.commit_params(p);
            }
            ui.separator();
            // Ducking and ChatMix open their own inner menus via the existing buttons.
            self.duck_button(ui);
            self.chatmix_button(ui);
            ui.separator();
            // Record controls inline.
            self.record_button(ui);
            // Macros.
            if !self.config.macros.is_empty() {
                ui.separator();
                self.macros_menu(ui);
            }
        });
    }

    fn route_control(&mut self, ui: &mut Ui) {
        let (routed, cable) = self.route_state();
        let Some(cable) = cable else {
            if self.config.engine.io.inputs[HARDWARE_STRIPS].is_some() {
                strip::stat_chip(ui, "CABLE Input is disabled", theme::NOTE_TEXT())
                .on_hover_text("Windows sound settings > Playback > right-click > Show disabled devices > enable CABLE Input");
            }
            return;
        };
        let label = if routed {
            "Windows audio: routed"
        } else {
            "Route Windows audio"
        };
        let tip = if routed {
            "Everything Windows plays goes through Wavemix. Click to give Windows its normal speakers back."
        } else {
            "Windows is playing straight to your speakers, so the mixer can't see it. Click to send it through Wavemix."
        };
        if strip::toggle(ui, label, routed, vec2(150.0, 28.0), theme::ACCENT())
            .on_hover_text(tip)
            .clicked()
        {
            if routed {
                self.unroute_windows_audio();
            } else {
                self.route_windows_audio(&cable);
            }
        }
        if !routed {
            strip::stat_chip(ui, "Windows sound bypasses the mixer", theme::NOTE_TEXT());
        }
    }

    /// Simple mode's "play through" row: tick the outputs you want (speakers, headphones, or both)
    /// and the mixer does the rest: engine on, Windows routed through it, system audio sent to
    /// exactly those outputs. Unticking the last one gives Windows back its own default.
    pub(super) fn output_switch(&mut self, ui: &mut Ui) {
        let devices = self.engine.devices();
        let hardware: Vec<&DeviceInfo> = devices
            .outputs
            .iter()
            .filter(|d| !d.is_virtual && !d.is_loopback)
            .collect();
        let (routed, cable) = self.route_state();
        if hardware.len() < 2 {
            return;
        }
        if cable.is_none() {
            if self.config.engine.io.inputs[HARDWARE_STRIPS].is_some()
                || devices
                    .inputs
                    .iter()
                    .any(|d| d.name.contains("CABLE Output"))
            {
                egui::Frame::new()
                    .fill(theme::TINT_NOTE())
                    .corner_radius(theme::R_CARD)
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new("Switching outputs needs CABLE Input enabled in Windows sound settings (Playback > Show disabled devices).")
                                .size(12.0)
                                .color(theme::NOTE_TEXT()),
                        );
                    });
            }
            return;
        }
        let running = self.engine.is_running();
        let active: Vec<String> = if running && routed {
            self.config.engine.io.outputs[..3]
                .iter()
                .flatten()
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        let mut next = active.clone();
        ui.label(
            RichText::new("PLAY THROUGH")
                .size(11.0)
                .strong()
                .color(theme::TEXT_DIM()),
        );
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            for d in &hardware {
                let on = active.contains(&d.name);
                let label = self.device_label(&d.name, hardware.iter().map(|h| h.name.as_str()));
                let label = strip::truncate(ui, &label, 150.0, 11.5);
                if strip::toggle(ui, &label, on, vec2(170.0, 28.0), theme::ACCENT())
                    .on_hover_text(format!(
                        "{}\nTick more than one to play on all of them at once",
                        d.name
                    ))
                    .clicked()
                {
                    if on {
                        next.retain(|n| n != &d.name);
                    } else if next.len() < 3 {
                        next.push(d.name.clone());
                    }
                }
            }
        });
        self.signal_hint(ui, routed && running);
        if next == active {
            return;
        }
        if next.is_empty() {
            self.unroute_windows_audio();
            return;
        }
        self.set_play_through(next);
    }

    /// Send system audio out of exactly these devices (Windows names), turning the engine on and
    /// routing Windows through the mixer if that isn't already so.
    pub(super) fn set_play_through(&mut self, next: Vec<String>) {
        let (routed, cable) = self.route_state();
        self.auto_configure();
        for b in 0..3 {
            let dev = next.get(b).cloned();
            self.config.engine.io.outputs[b] = dev.clone();
            self.engine.set_output(b, dev);
        }
        let mut params = self.engine.params();
        for (b, send) in params.strips[HARDWARE_STRIPS]
            .sends
            .iter_mut()
            .take(3)
            .enumerate()
        {
            *send = b < next.len();
        }
        // A bus or strip left far down from earlier tinkering would make this look broken.
        for b in 0..next.len() {
            params.buses[b].mute = false;
            if params.buses[b].gain_db < -12.0 {
                params.buses[b].gain_db = 0.0;
            }
        }
        let cable_strip = &mut params.strips[HARDWARE_STRIPS];
        cable_strip.mute = false;
        if cable_strip.gain_db < -12.0 {
            cable_strip.gain_db = 0.0;
        }
        self.commit_params(params);
        self.config.engine.io.engine_on = true;
        self.engine.set_running(true);
        if !routed {
            if let Some(cable) = cable {
                self.route_windows_audio(&cable);
            }
        }
    }

    /// Hardware playback devices (no virtual cables), by Windows name.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn hardware_outputs(&self) -> Vec<String> {
        self.engine
            .devices()
            .outputs
            .iter()
            .filter(|d| !d.is_virtual && !d.is_loopback)
            .map(|d| d.name.clone())
            .collect()
    }

    /// Hotkey: move to the next (or previous) output. While Windows is routed through the mixer
    /// that is the mixer's output; otherwise it is Windows' default playback device. Returns the
    /// name of the device switched to.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn cycle_output(&mut self, forward: bool) -> Option<String> {
        let (routed, _) = self.route_state();
        if routed && self.engine.is_running() {
            let hardware = self.hardware_outputs();
            if hardware.len() < 2 {
                return None;
            }
            let current = self.config.engine.io.outputs[..3]
                .iter()
                .flatten()
                .next()
                .and_then(|c| hardware.iter().position(|h| h == c));
            let name = hardware[cycle_index(current, hardware.len(), forward)].clone();
            self.set_play_through(vec![name.clone()]);
            return Some(self.device_label(&name, hardware.iter().map(String::as_str)));
        }
        let snap = self.backend.snapshot();
        let render: Vec<&crate::model::Device> = snap
            .devices
            .iter()
            .filter(|d| {
                d.flow == crate::model::Flow::Render && !crate::engine::io::looks_virtual(&d.name)
            })
            .collect();
        if render.len() < 2 {
            return None;
        }
        let current = render.iter().position(|d| d.is_default);
        let target = render[cycle_index(current, render.len(), forward)];
        self.backend.send(crate::model::Command::SetDefaultDevice {
            id: target.id.clone(),
            flow: crate::model::Flow::Render,
        });
        Some(self.device_label(&target.name, render.iter().map(|d| d.name.as_str())))
    }

    /// Say where the sound stops when something is playing but nothing is heard.
    fn signal_hint(&mut self, ui: &mut Ui, live: bool) {
        if !live {
            return;
        }
        let snap = self.backend.snapshot();
        let playing = snap.sessions.iter().any(|s| s.active && s.peak > 0.02);
        if !playing {
            return;
        }
        let levels = self.engine.levels();
        let strip = levels
            .strip(HARDWARE_STRIPS)
            .into_iter()
            .fold(0.0, f32::max);
        let bus = (0..3).flat_map(|b| levels.bus(b)).fold(0.0, f32::max);
        let msg = if strip < 1e-4 {
            "Sound isn't reaching the mixer. In Pro, check Virtual 1 is set to CABLE Output."
        } else if bus < 1e-4 {
            "The mixer has the sound but its outputs are silent. In Pro, check A1-A3 aren't muted or turned down."
        } else {
            return;
        };
        ui.add_space(4.0);
        ui.label(RichText::new(msg).size(12.0).color(theme::NOTE_TEXT()));
        if strip::ghost_button(ui, "Give Windows its audio back", false)
            .on_hover_text("Stop routing and play straight to your speakers again")
            .clicked()
        {
            self.unroute_windows_audio();
        }
    }

    pub(super) fn route_windows_audio(&mut self, cable: &crate::model::Device) {
        let snap = self.backend.snapshot();
        self.config.restore_playback = snap
            .devices
            .iter()
            .find(|d| d.flow == crate::model::Flow::Render && d.is_default)
            .map(|d| d.id.clone());
        self.backend.send(crate::model::Command::SetDefaultDevice {
            id: cable.id.clone(),
            flow: crate::model::Flow::Render,
        });
        self.mark_engine_dirty();
    }

    /// Runs every frame. Once Windows' devices are known: after a crash or forced close, put the
    /// speakers back if we had moved them and the engine isn't running; and, if asked to, route
    /// when the engine starts.
    pub(super) fn route_housekeeping(&mut self) {
        if self.backend.snapshot().devices.is_empty() {
            return;
        }
        if !self.names_migrated {
            self.names_migrated = true;
            self.migrate_device_names();
        }
        let running = self.engine.is_running();
        if !self.route_checked {
            self.route_checked = true;
            if !running && self.config.restore_playback.is_some() {
                self.unroute_windows_audio();
            }
        }
        if !running {
            self.auto_routed = false;
        } else if self.config.auto_route && !self.auto_routed {
            self.auto_routed = true;
            let (routed, cable) = self.route_state();
            if let (false, Some(cable)) = (routed, cable) {
                self.route_windows_audio(&cable);
            }
        }
    }

    /// Put Windows' default playback device back where it was before routing.
    pub(super) fn unroute_windows_audio(&mut self) {
        let snap = self.backend.snapshot();
        let render = |d: &&crate::model::Device| d.flow == crate::model::Flow::Render;
        let had_restore = self.config.restore_playback.is_some();
        let target = self
            .config
            .restore_playback
            .take()
            .filter(|id| snap.devices.iter().any(|d| &d.id == id))
            .or_else(|| {
                snap.devices
                    .iter()
                    .filter(render)
                    .find(|d| !crate::engine::io::looks_virtual(&d.name))
                    .map(|d| d.id.clone())
            });
        // Also covers a default we moved to some other virtual device in an earlier version.
        let on_cable = self.route_state().0
            || (had_restore
                && snap
                    .devices
                    .iter()
                    .filter(render)
                    .any(|d| d.is_default && crate::engine::io::looks_virtual(&d.name)));
        if let (Some(id), true) = (target, on_cable) {
            self.backend.send(crate::model::Command::SetDefaultDevice {
                id,
                flow: crate::model::Flow::Render,
            });
        }
        self.mark_engine_dirty();
    }

    /// Mark engine settings as changed; they're written to disk shortly after the last change.
    pub(super) fn mark_engine_dirty(&mut self) {
        self.engine_dirty_at = Some(Instant::now());
    }

    pub(super) fn commit_params(&mut self, params: EngineParams) {
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
    pub(super) fn auto_configure(&mut self) {
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
        // "What you hear": a strip that shows the real level of everything playing, with no
        // sends, so it can't feed back into the speakers it is listening to.
        let system = devices
            .inputs
            .iter()
            .filter(|d| d.is_loopback)
            .find(|d| d.is_default)
            .or_else(|| devices.inputs.iter().find(|d| d.is_loopback))
            .map(|d| d.name.clone());

        let mut params = self.engine.params();
        // Hardware inputs stay off A1 so you don't hear your own mic; they go to B1 for chat apps.
        for s in params.strips.iter_mut().take(HARDWARE_STRIPS) {
            s.sends = [false, false, false, cable_in.is_some(), false];
        }
        // Virtual 1 (the cable) plays on A1; Virtual 2 (system audio) is for metering only.
        params.strips[HARDWARE_STRIPS].sends = [true, false, false, false, false];
        params.strips[STRIPS - 1].sends = [false; BUSES];
        self.commit_params(params);

        let io = &mut self.config.engine.io;
        io.inputs[0] = mic.clone();
        io.inputs[HARDWARE_STRIPS] = cable_out.clone();
        io.inputs[STRIPS - 1] = system.clone();
        io.outputs[0] = speakers.clone();
        io.outputs[3] = cable_in.clone();
        self.engine.set_input(0, mic);
        self.engine.set_input(HARDWARE_STRIPS, cable_out);
        self.engine.set_input(STRIPS - 1, system);
        self.engine.set_output(0, speakers);
        self.engine.set_output(3, cable_in);
        self.config.engine.configured = true;
        // Opening Pro for the first time is what starts the engine.
        self.config.engine.io.engine_on = true;
        self.engine.set_running(true);
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
            Some(n) => self.device_label(n, list.iter().map(|d| d.name.as_str())),
            None => "Select device".to_string(),
        };
        let resp = strip::chip(ui, &text, CONSOLE_STRIP_W, current.is_some()).on_hover_text(
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
            const LABELS: [&str; 3] = [
                "SYSTEM AUDIO (what you hear, for level meters and desktop capture)",
                "VIRTUAL CABLES",
                "HARDWARE",
            ];
            let kind = |d: &DeviceInfo| {
                if d.is_loopback {
                    0
                } else if d.is_virtual {
                    1
                } else {
                    2
                }
            };
            let order = if want_virtual { [0, 1, 2] } else { [2, 1, 0] };
            for k in order {
                let group: Vec<&DeviceInfo> = list.iter().filter(|d| kind(d) == k).collect();
                if group.is_empty() {
                    continue;
                }
                ui.label(RichText::new(LABELS[k]).size(10.0).color(theme::TEXT_DIM()));
                for d in group {
                    let label = format!(
                        "{}{}",
                        match self.alias_for_name(&d.name) {
                            Some(alias) => format!("{alias}  ({})", d.name),
                            None => d.name.clone(),
                        },
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
                        .color(theme::TEXT_DIM()),
                );
                ui.hyperlink_to("Get VB-Cable (free)", "https://vb-audio.com/Cable/");
            }
        });
        chosen
    }

    fn status_dot(&self, ui: &mut Ui, status: &SlotStatus) {
        let (color, tip) = match status {
            SlotStatus::Running { rate } => (theme::METER_GREEN(), format!("Running at {rate} Hz")),
            SlotStatus::Missing => (theme::METER_YELLOW(), "Device not found".to_string()),
            SlotStatus::Error(e) => (theme::MUTE(), format!("Couldn't open: {e}")),
            SlotStatus::Off => (theme::TEXT_DIM(), "No device, or engine off".to_string()),
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
            theme::ACCENT_IN()
        } else {
            theme::ACCENT()
        };
        strip_card(ui, |ui| {
            ui.set_width(CONSOLE_STRIP_W);
            ui.spacing_mut().item_spacing = vec2(6.0, 8.0);
            ui.horizontal(|ui| {
                self.title_label(ui, super::Rename::Strip, i, None);
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

            let fader_id = super::wid(("in-fader", i));
            if strip::fader_focused(ui, fader_id) {
                let key = |ui: &mut Ui, k: egui::Key| {
                    ui.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, k))
                };
                if key(ui, egui::Key::M) {
                    p.mute = !p.mute;
                }
                if key(ui, egui::Key::S) {
                    p.solo = !p.solo;
                }
                if key(ui, egui::Key::E) {
                    self.editing = if matches!(self.editing, Some((n, _)) if n == i) {
                        None
                    } else {
                        Some((i, EditorTab::Eq))
                    };
                }
            }
            if let Some(db) =
                strip::db_fader(ui, fader_id, fader_h, p.gain_db, peaks, accent, p.mute)
            {
                p.gain_db = db;
            }
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(db_text(p.gain_db))
                        .size(12.0)
                        .monospace()
                        .color(theme::TEXT()),
                );
            });
            ui.add_space(4.0);
            // Face of the strip: only what you reach for constantly. Everything else is one
            // click away, under "Route" and "Tune".
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if strip::toggle(
                    ui,
                    "Mute",
                    p.mute,
                    vec2((CONSOLE_STRIP_W - 6.0) / 2.0, 28.0),
                    theme::MUTE(),
                )
                .on_hover_text("Mute")
                .clicked()
                {
                    p.mute = !p.mute;
                }
                if strip::toggle(
                    ui,
                    "Solo",
                    p.solo,
                    vec2((CONSOLE_STRIP_W - 6.0) / 2.0, 28.0),
                    theme::METER_YELLOW(),
                )
                .on_hover_text("Solo: hear only soloed strips")
                .clicked()
                {
                    p.solo = !p.solo;
                }
            });

            // Where this strip goes (and its pan and mono), in a menu.
            let sends: Vec<String> = (0..BUSES)
                .filter(|b| p.sends[*b])
                .map(|b| self.config.bus_title(b))
                .collect();
            let summary = match sends.len() {
                0 => "Sends nowhere".to_string(),
                1..=3 => format!("To {}", sends.join(" ")),
                n => format!("To {n} buses"),
            };
            let route = strip::chip(ui, &summary, CONSOLE_STRIP_W, false)
                .on_hover_text("Choose which buses this strip feeds, plus pan and mono");
            egui::Popup::menu(&route).show(|ui| {
                ui.set_min_width(210.0);
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                strip::caption(ui, "SEND TO");
                ui.horizontal_wrapped(|ui| {
                    for (b, bus_color) in bus_colors().into_iter().enumerate() {
                        // A strip that listens to a cable can't also feed the bus playing into it.
                        let loops = match (
                            &self.config.engine.io.inputs[i],
                            &self.config.engine.io.outputs[b],
                        ) {
                            (Some(inp), Some(out)) => feeds_itself(inp, out),
                            _ => false,
                        };
                        let (color, tip) = if loops && p.sends[b] {
                            (
                                theme::MUTE(),
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
                        if strip::toggle(ui, bus_name(b), p.sends[b], vec2(38.0, 28.0), color)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            p.sends[b] = !p.sends[b];
                        }
                    }
                });
                ui.add_space(4.0);
                strip::caption(ui, "PAN");
                if let Some(v) = strip::balance(ui, super::wid(("pan", i)), p.pan, 200.0) {
                    p.pan = v;
                }
                ui.add_space(2.0);
                if strip::toggle(ui, "Mono", p.mono, vec2(64.0, 28.0), theme::ACCENT())
                    .on_hover_text("Mix left and right into one")
                    .clicked()
                {
                    p.mono = !p.mono;
                }
            });

            // EQ, gate and compressor share one editor with tabs.
            let working = p.eq.enabled || p.gate.enabled || p.comp.enabled || p.denoise;
            let open = matches!(self.editing, Some((n, _)) if n == i);
            if strip::process_button(ui, "Tune", working, open, vec2(CONSOLE_STRIP_W, 28.0))
                .on_hover_text("EQ, noise gate and compressor")
                .clicked()
            {
                self.editing = if open { None } else { Some((i, EditorTab::Eq)) };
                self.bus_editing = None;
            }
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
        strip_card(ui, |ui| {
            ui.set_width(CONSOLE_STRIP_W);
            ui.spacing_mut().item_spacing = vec2(6.0, 8.0);
            ui.horizontal(|ui| {
                self.title_label(ui, super::Rename::Bus, b, Some(bus_colors()[b]));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.status_dot(ui, &status.outputs[b]);
                });
            });
            let current = self.config.engine.io.outputs[b].clone();
            if let Some(sel) = self.device_picker(ui, &current, &devices.outputs, b >= 3) {
                // Two buses on one device would fight over it (and glitch), so the device moves.
                if sel.is_some() {
                    for other in 0..BUSES {
                        if other != b && self.config.engine.io.outputs[other] == sel {
                            self.config.engine.io.outputs[other] = None;
                            self.engine.set_output(other, None);
                        }
                    }
                }
                self.config.engine.io.outputs[b] = sel.clone();
                self.engine.set_output(b, sel);
                self.mark_engine_dirty();
            }
            let fader_id = super::wid(("bus-fader", b));
            if strip::fader_focused(ui, fader_id) {
                let key = |ui: &mut Ui, k: egui::Key| {
                    ui.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, k))
                };
                if key(ui, egui::Key::M) {
                    p.mute = !p.mute;
                }
                if key(ui, egui::Key::L) {
                    p.limiter = !p.limiter;
                }
            }
            if let Some(db) = strip::db_fader(
                ui,
                fader_id,
                fader_h,
                p.gain_db,
                peaks,
                bus_colors()[b],
                p.mute,
            ) {
                p.gain_db = db;
            }
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(db_text(p.gain_db))
                        .size(12.0)
                        .monospace()
                        .color(theme::TEXT()),
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if strip::toggle(
                    ui,
                    "Mute",
                    p.mute,
                    vec2((CONSOLE_STRIP_W - 6.0) / 2.0, 28.0),
                    theme::MUTE(),
                )
                .on_hover_text("Mute this bus")
                .clicked()
                {
                    p.mute = !p.mute;
                }
                if strip::toggle(
                    ui,
                    "Limit",
                    p.limiter,
                    vec2((CONSOLE_STRIP_W - 6.0) / 2.0, 28.0),
                    theme::METER_GREEN(),
                )
                .on_hover_text("Limiter: stops the summed mix from clipping")
                .clicked()
                {
                    p.limiter = !p.limiter;
                }
            });
            let delay_text = if p.delay_ms > 0.0 {
                format!("Delay {:.0} ms", p.delay_ms)
            } else {
                "No delay".to_string()
            };
            let delay = strip::chip(ui, &delay_text, CONSOLE_STRIP_W, p.delay_ms > 0.0)
                .on_hover_text(
                    "Hold this output back so it lines up with a slower one, for example wired speakers waiting for Bluetooth headphones",
                );
            egui::Popup::menu(&delay).show(|ui| {
                ui.set_min_width(190.0);
                strip::caption(ui, "TEST CLICK");
                let mut click = self.engine.test_click();
                if ui
                    .checkbox(&mut click, "Click on every output")
                    .on_hover_text(
                        "A short click twice a second on each output, to hear whether they line up. Add delay to the one that's early.",
                    )
                    .changed()
                {
                    self.engine.set_test_click(click);
                    self.click_until = click.then(|| Instant::now() + Duration::from_secs(20));
                }
                ui.add_space(4.0);
                strip::caption(ui, "DELAY");
                ui.add(
                    egui::DragValue::new(&mut p.delay_ms)
                        .range(0.0..=crate::engine::mixer::MAX_BUS_DELAY_MS)
                        .speed(1.0)
                        .suffix(" ms"),
                );
            });
            // Per-bus record chip: shows recording state, starts/stops this bus.
            let rec = self.engine.recording_for_bus(b);
            let rec_label = match &rec {
                Some((_, secs)) => {
                    // Refresh every quarter second while recording.
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(250));
                    format!("● {}", super::tools::format_clock(*secs))
                }
                None => String::new(),
            };
            if !rec_label.is_empty() {
                if strip::toggle(
                    ui,
                    &rec_label,
                    true,
                    vec2(CONSOLE_STRIP_W, 26.0),
                    theme::MUTE(),
                )
                .on_hover_text("Recording — click to stop")
                .clicked()
                {
                    self.set_recording(false, Some(b));
                }
            } else if ui
                .add_enabled(
                    self.engine.is_running(),
                    egui::Button::new(RichText::new("Record").size(11.5))
                        .min_size(vec2(CONSOLE_STRIP_W, 26.0)),
                )
                .on_hover_text("Record this bus to a WAV file")
                .clicked()
            {
                self.set_recording(true, Some(b));
            }
            // The bus equaliser; also keeps the bottoms of both panels level.
            let open = self.bus_editing == Some(b);
            if strip::process_button(ui, "EQ + Comp", p.eq.enabled || p.comp.enabled, open, vec2(CONSOLE_STRIP_W, 28.0))
                .on_hover_text("Six-band EQ, bus compressor, AutoEQ loader and stereo controls for this output")
                .clicked()
            {
                self.bus_editing = if open { None } else { Some(b) };
                self.editing = None;
            }
        });
    }

    // --- signal flow routing matrix ------------------------------------------------------------

    fn signal_flow(&mut self, ui: &mut Ui, params: &mut EngineParams) {
        let bus_colors = bus_colors();
        let levels = self.engine.levels();
        egui::Frame::new()
            .fill(theme::PANEL())
            .stroke(egui::Stroke::new(1.0, theme::BORDER()))
            .corner_radius(theme::R_PANEL)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                let cell = 36.0;
                let label_w = 90.0;
                let total_w = label_w + (BUSES as f32) * cell + 80.0;
                ui.set_min_width(total_w);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("SIGNAL FLOW")
                            .size(11.0)
                            .strong()
                            .color(theme::TEXT_DIM()),
                    );
                    ui.add_space(12.0);
                    ui.label(
                        egui::RichText::new("● = sends to bus   click to toggle   size = signal level   dim = muted")
                            .size(10.0)
                            .color(theme::TEXT_DIM()),
                    );
                });
                ui.add_space(4.0);
                // Header row: bus names.
                ui.horizontal(|ui| {
                    ui.add_space(label_w);
                    for b in 0..BUSES {
                        let col = bus_colors[b];
                        ui.add_sized(
                            vec2(cell, 18.0),
                            egui::Label::new(
                                egui::RichText::new(bus_name(b))
                                    .size(10.0)
                                    .strong()
                                    .color(col),
                            ),
                        );
                    }
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Level").size(10.0).color(theme::TEXT_DIM()));
                });
                // One row per strip. Collect clicks to apply after the layout loop to avoid
                // aliasing the params borrow inside the closure.
                let mut toggle: Option<(usize, usize)> = None;
                for i in 0..STRIPS {
                    let is_virtual = i >= HARDWARE_STRIPS;
                    let strip_color = if is_virtual {
                        theme::ACCENT_IN()
                    } else {
                        theme::ACCENT()
                    };
                    let any_solo = params.strips.iter().any(|s| s.solo);
                    let muted = params.strips[i].mute;
                    let solo = params.strips[i].solo;
                    let audible = !muted && !(any_solo && !solo);
                    let dim = if audible { 1.0 } else { 0.35 };
                    ui.horizontal(|ui| {
                        // Strip name, left-aligned and truncated.
                        let name = self.config.strip_title(i);
                        let name = strip::truncate(ui, &name, label_w - 8.0, 11.5);
                        ui.add_sized(
                            vec2(label_w, 22.0),
                            egui::Label::new(
                                egui::RichText::new(name)
                                    .size(11.5)
                                    .color(strip_color.gamma_multiply(dim)),
                            ),
                        );
                        // Send dots for each bus — click to toggle routing.
                        for b in 0..BUSES {
                            let sends = params.strips[i].sends[b];
                            // Check if enabling this send would create a feedback loop.
                            let loops = match (
                                &self.config.engine.io.inputs[i],
                                &self.config.engine.io.outputs[b],
                            ) {
                                (Some(inp), Some(out)) => feeds_itself(inp, out),
                                _ => false,
                            };
                            let bus_peak = if sends {
                                let lv = levels.strip(i);
                                lv[0].max(lv[1])
                            } else {
                                0.0
                            };
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(cell, 22.0), Sense::click());
                            let center = rect.center();
                            let dot_color = if loops && sends {
                                theme::MUTE() // red: active loop
                            } else {
                                bus_colors[b]
                            };
                            if sends {
                                let r = (4.0 + bus_peak * 8.0).min(10.0) * dim;
                                ui.painter().circle_filled(
                                    center,
                                    r,
                                    dot_color.gamma_multiply(dim),
                                );
                            } else {
                                let ring_color = if loops {
                                    theme::NOTE_TEXT().gamma_multiply(0.7)
                                } else {
                                    theme::BORDER()
                                };
                                ui.painter().circle_stroke(
                                    center,
                                    3.5,
                                    egui::Stroke::new(1.0, ring_color),
                                );
                            }
                            let tip = if loops && sends {
                                format!(
                                    "⚠ Feedback loop! {} listens to the cable {} plays into — click to remove",
                                    self.config.strip_title(i),
                                    bus_name(b)
                                )
                            } else if loops {
                                format!(
                                    "Would create a feedback loop: {} listens to the cable {} plays into",
                                    self.config.strip_title(i),
                                    bus_name(b)
                                )
                            } else if sends {
                                format!(
                                    "{} → {} — click to remove",
                                    self.config.strip_title(i),
                                    bus_name(b)
                                )
                            } else {
                                format!(
                                    "Click to send {} to {}",
                                    self.config.strip_title(i),
                                    bus_name(b)
                                )
                            };
                            // Don't toggle ON a send that would create a loop.
                            if resp.on_hover_text(tip).clicked() && !(loops && !sends) {
                                toggle = Some((i, b));
                            }
                        }
                        // Strip level mini-meter.
                        ui.add_space(8.0);
                        let peaks = levels.strip(i);
                        let peak = peaks[0].max(peaks[1]);
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(64.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(rect, 2.0, theme::METER_BG());
                        let fill_w = (peak.min(1.0) * rect.width() * dim) as f32;
                        if fill_w > 0.0 {
                            let fill = egui::Rect::from_min_size(rect.min, vec2(fill_w, rect.height()));
                            let color = if peak > 0.9 {
                                theme::MUTE()
                            } else {
                                strip_color.gamma_multiply(dim)
                            };
                            ui.painter().rect_filled(fill, 2.0, color);
                        }
                    });
                }
                if let Some((i, b)) = toggle {
                    params.strips[i].sends[b] = !params.strips[i].sends[b];
                }
                // Bus output row — click to open that bus's editor.
                let mut open_bus: Option<usize> = None;
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_sized(
                        vec2(label_w, 22.0),
                        egui::Label::new(
                            egui::RichText::new("Bus out")
                                .size(10.0)
                                .color(theme::TEXT_DIM()),
                        ),
                    );
                    for b in 0..BUSES {
                        let muted = params.buses[b].mute;
                        let gain_db = params.buses[b].gain_db;
                        let peak = {
                            let l = levels.bus(b);
                            l[0].max(l[1])
                        };
                        let dim = if muted { 0.3 } else { 1.0 };
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(cell, 22.0), Sense::click());
                        let bar_h = (rect.height() * (peak * dim).min(1.0)) as f32;
                        let fill = egui::Rect::from_min_size(
                            egui::pos2(rect.center().x - 6.0, rect.bottom() - bar_h),
                            vec2(12.0, bar_h),
                        );
                        ui.painter().rect_filled(
                            fill,
                            2.0,
                            bus_colors[b].gamma_multiply(dim),
                        );
                        let tip = format!(
                            "{}: {:.1} dB{} — click to open EQ",
                            bus_name(b),
                            gain_db,
                            if muted { " (muted)" } else { "" }
                        );
                        if resp.on_hover_text(tip).clicked() {
                            open_bus = Some(b);
                        }
                    }
                });
                if let Some(b) = open_bus {
                    self.bus_editing = if self.bus_editing == Some(b) { None } else { Some(b) };
                    self.editing = None;
                }
            });
    }

    // --- processing editor ---------------------------------------------------------------------

    fn editor(&mut self, ui: &mut Ui, i: usize, tab: EditorTab, p: &mut StripParams) {
        egui::Frame::new()
            .fill(theme::PANEL())
            .stroke(Stroke::new(1.0, theme::BORDER()))
            .corner_radius(theme::R_CARD)
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.config.strip_title(i))
                            .size(13.0)
                            .strong(),
                    );
                    ui.add_space(8.0);
                    for (t, label) in [
                        (EditorTab::Eq, "Equalizer"),
                        (EditorTab::Gate, "Noise gate"),
                        (EditorTab::Comp, "Compressor"),
                        (EditorTab::Denoise, "Noise removal"),
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
                    EditorTab::Denoise => denoise_editor(ui, p),
                }
            });
    }
}

// --- editors -----------------------------------------------------------------------------------

pub(super) const EQ_DB_RANGE: f32 = 18.0;
const EQ_MIN_HZ: f32 = 20.0;
const EQ_MAX_HZ: f32 = 20_000.0;

pub(super) fn hz_to_x(rect: egui::Rect, hz: f32) -> f32 {
    let t = (hz.clamp(EQ_MIN_HZ, EQ_MAX_HZ) / EQ_MIN_HZ).ln() / (EQ_MAX_HZ / EQ_MIN_HZ).ln();
    rect.left() + rect.width() * t
}

pub(super) fn x_to_hz(rect: egui::Rect, x: f32) -> f32 {
    let t = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    EQ_MIN_HZ * (EQ_MAX_HZ / EQ_MIN_HZ).powf(t)
}

pub(super) fn db_to_y(rect: egui::Rect, db: f32) -> f32 {
    rect.center().y - db.clamp(-EQ_DB_RANGE, EQ_DB_RANGE) / EQ_DB_RANGE * rect.height() / 2.0
}

pub(super) fn y_to_db(rect: egui::Rect, y: f32) -> f32 {
    ((rect.center().y - y) / (rect.height() / 2.0) * EQ_DB_RANGE).clamp(-EQ_DB_RANGE, EQ_DB_RANGE)
}

fn eq_editor(ui: &mut Ui, p: &mut EqParams) {
    ui.horizontal(|ui| {
        ui.checkbox(&mut p.enabled, "Enable equalizer");
        ui.label(
            RichText::new("Drag the dots: sideways for frequency, up and down for gain.")
                .size(11.0)
                .color(theme::TEXT_DIM()),
        );
        if ui.small_button("Reset").clicked() {
            *p = EqParams {
                enabled: p.enabled,
                ..EqParams::default()
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

    // The combined response curve.
    let curve_color = if p.enabled {
        theme::ACCENT()
    } else {
        theme::TEXT_DIM()
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
        painter.circle_stroke(*s, 7.0, Stroke::new(2.0, theme::BG()));
        painter.text(
            *s + vec2(0.0, -12.0),
            egui::Align2::CENTER_BOTTOM,
            label,
            egui::FontId::proportional(10.0),
            theme::TEXT(),
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

fn denoise_editor(ui: &mut Ui, p: &mut StripParams) {
    ui.checkbox(&mut p.denoise, "Remove background noise");
    ui.label(
        RichText::new(
            "A small neural network trained on speech takes out hiss, fans, keyboard clatter and \
             room noise and keeps the voice. Made for microphones; it works on one channel (a \
             stereo input is mixed to mono first) and adds about 10 ms of delay.",
        )
        .size(12.0)
        .color(theme::TEXT_DIM()),
    );
}

fn gate_editor(ui: &mut Ui, p: &mut StripParams, open: bool) {
    let g = &mut p.gate;
    ui.horizontal(|ui| {
        ui.checkbox(&mut g.enabled, "Enable noise gate");
        if g.enabled {
            let (text, color) = if open {
                ("open", theme::METER_GREEN())
            } else {
                ("closed", theme::TEXT_DIM())
            };
            ui.label(RichText::new(format!("gate {text}")).color(color));
        }
    });
    ui.label(
        RichText::new(
            "Silences the strip when it falls below the threshold, so background noise disappears between words.",
        )
        .size(11.0)
        .color(theme::TEXT_DIM()),
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
                    .color(theme::TEXT_DIM()),
            );
        }
    });
    ui.label(
        RichText::new(
            "Evens out loud and quiet parts: anything above the threshold is turned down by the ratio.",
        )
        .size(11.0)
        .color(theme::TEXT_DIM()),
    );
    // Gain-reduction meter: grows to the left from zero.
    let (rect, _) = ui.allocate_exact_size(vec2(260.0, 10.0), Sense::hover());
    ui.painter().rect_filled(rect, 4.0, theme::METER_BG());
    let frac = (-gain_reduction_db / 24.0).clamp(0.0, 1.0);
    let fill = egui::Rect::from_min_max(
        pos2(rect.right() - rect.width() * frac, rect.top()),
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

#[cfg(test)]
mod cycle_tests {
    use super::cycle_index;

    #[test]
    fn cycling_wraps_both_ways() {
        assert_eq!(cycle_index(Some(0), 3, true), 1);
        assert_eq!(cycle_index(Some(2), 3, true), 0);
        assert_eq!(cycle_index(Some(0), 3, false), 2);
        assert_eq!(cycle_index(None, 3, true), 0);
        assert_eq!(cycle_index(None, 3, false), 2);
    }
}
