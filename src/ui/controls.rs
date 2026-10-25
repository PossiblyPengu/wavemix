//! Ways to drive the mixer without clicking it: macros, MIDI controllers, and the settings for
//! sending audio to and from other computers.

use eframe::egui::{self, RichText, Ui};
use serde_json::json;

use super::settings::{hint, section};
use super::{strip, theme, MixerApp};
use crate::config::Macro;
use crate::engine::params::{BUSES, STRIPS};
use crate::engine::vban::{VbanIn, VbanOut};
use crate::midi::{Binding, Target, TARGET_KINDS};
use crate::remote;

/// How often to look for a controller that was plugged in or removed.
const MIDI_SCAN_EVERY: std::time::Duration = std::time::Duration::from_secs(3);

impl MixerApp {
    // --- macros --------------------------------------------------------------------------------

    /// Run every line of the macro called `name`, as if each were a remote request.
    pub(super) fn run_macro(&mut self, name: &str) -> (u16, String) {
        let Some(commands) = self
            .config
            .macros
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .map(|m| m.commands.clone())
        else {
            return (
                404,
                json!({ "ok": false, "error": "no macro with that name" }).to_string(),
            );
        };
        let (mut ran, mut problems) = (0, Vec::new());
        for (n, line) in commands.lines().enumerate() {
            let Some(request) = remote::request_from_line(line) else {
                continue;
            };
            // A macro that runs macros could run forever.
            if request.path.get(1).map(String::as_str) == Some("macro") {
                problems.push(format!("line {}: a macro can't run another macro", n + 1));
                continue;
            }
            let (code, body) = self.handle_remote(&request);
            if code < 400 {
                ran += 1;
            } else {
                let why = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v["error"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "failed".into());
                problems.push(format!("line {}: {why}", n + 1));
            }
        }
        self.macro_note = (!problems.is_empty()).then(|| problems.join("; "));
        if problems.is_empty() {
            (200, json!({ "ok": true, "ran": ran }).to_string())
        } else {
            (
                400,
                json!({ "ok": false, "ran": ran, "error": problems.join("; ") }).to_string(),
            )
        }
    }

    /// The "Macros" button in the engine bar: one click per macro.
    pub(super) fn macros_menu(&mut self, ui: &mut Ui) {
        if self.config.macros.is_empty() {
            return;
        }
        let resp =
            strip::chip(ui, "Macros", 90.0, false).on_hover_text("Run a saved list of actions");
        let mut run = None;
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(180.0);
            for m in &self.config.macros {
                if !m.name.trim().is_empty() && ui.button(&m.name).clicked() {
                    run = Some(m.name.clone());
                    ui.close();
                }
            }
            if let Some(note) = &self.macro_note {
                ui.separator();
                ui.label(RichText::new(note).size(11.0).color(theme::NOTE_TEXT()));
            }
        });
        if let Some(name) = run {
            self.run_macro(&name);
        }
    }

    pub(super) fn macros_section(&mut self, ui: &mut Ui) {
        section(ui, "Macros");
        hint(
            ui,
            "A macro does several things with one click, one command per line. Run it from the \
             Macros button in Pro mode, from a MIDI pad, or from the remote address \
             /api/macro?name=...",
        );
        let mut changed = false;
        let mut remove = None;
        let mut run = None;
        for (n, m) in self.config.macros.iter_mut().enumerate() {
            egui::Frame::new()
                .fill(theme::PANEL_HI())
                .corner_radius(theme::R_CARD)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut m.name)
                                    .hint_text("Name")
                                    .desired_width(150.0),
                            )
                            .changed();
                        if ui.button("Run").clicked() {
                            run = Some(m.name.clone());
                        }
                        if ui.button("Remove").clicked() {
                            remove = Some(n);
                        }
                    });
                    changed |= ui
                        .add(
                            egui::TextEdit::multiline(&mut m.commands)
                                .font(egui::TextStyle::Monospace)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY)
                                .hint_text(
                                    "strip/0?mute=1\nbus/1?gain_db=-6\nchatmix?balance=-0.5",
                                ),
                        )
                        .changed();
                });
        }
        if let Some(n) = remove {
            self.config.macros.remove(n);
            changed = true;
        }
        if ui.button("Add a macro").clicked() {
            self.config.macros.push(Macro {
                name: format!("Macro {}", self.config.macros.len() + 1),
                commands: String::new(),
            });
            changed = true;
        }
        if changed {
            self.config.save();
        }
        if let Some(name) = run {
            self.run_macro(&name);
        }
        if let Some(note) = &self.macro_note {
            hint(ui, note);
        }
        hint(
            ui,
            "Commands: strip/0?gain_db=-6&mute=toggle (also solo, pan, mono, denoise, send_a1...) \u{b7} \
             bus/0?gain_db=0&mute=1&limiter=1&delay_ms=20&crossfeed=0.4&width=1 \u{b7} \
             chatmix?balance=-1..1 \u{b7} engine?on=toggle \u{b7} duck?on=toggle \u{b7} \
             bypass?on=toggle \u{b7} record?on=toggle \u{b7} preset?name=...",
        );
    }

    // --- MIDI ----------------------------------------------------------------------------------

    /// Every frame: keep the MIDI inputs open, and act on what they sent.
    pub(super) fn drive_midi(&mut self, ctx: &egui::Context) {
        if !self.config.midi.enabled {
            self.midi = None;
            self.midi_learning = None;
            return;
        }
        let stale = self
            .midi
            .as_ref()
            .is_none_or(|m| m.device != self.config.midi.device);
        if stale || self.midi_scanned.elapsed() >= MIDI_SCAN_EVERY {
            self.midi_scanned = std::time::Instant::now();
            let ports = crate::midi_device::port_names();
            if stale || ports != self.midi_ports {
                self.midi_ports = ports;
                let c = ctx.clone();
                self.midi = Some(crate::midi_device::open(
                    &self.config.midi.device,
                    move || c.request_repaint(),
                ));
            }
        }
        // Wake up now and then even when nothing is happening, to notice a controller arriving.
        ctx.request_repaint_after(MIDI_SCAN_EVERY);

        let mut events = Vec::new();
        if let Some(m) = &self.midi {
            while let Ok(e) = m.events.try_recv() {
                events.push(e);
            }
        }
        let Some(last) = events.last() else {
            return;
        };
        self.midi_last = Some((*last, std::time::Instant::now()));

        if let Some(n) = self.midi_learning {
            // The next control that moves is the one being taught.
            let learned = self
                .config
                .midi
                .bindings
                .get_mut(n)
                .is_some_and(|b| events.iter().any(|e| b.learn(e)));
            if learned {
                self.midi_learning = None;
                self.config.save();
            }
            return;
        }
        let lines = self
            .midi_mapper
            .commands(&self.config.midi.bindings, &events);
        for line in lines {
            if let Some(request) = remote::request_from_line(&line) {
                self.handle_remote(&request);
            }
        }
    }

    pub(super) fn midi_section(&mut self, ui: &mut Ui) {
        section(ui, "MIDI control");
        hint(
            ui,
            "Use the faders, knobs and pads of a MIDI controller to run the mixer. Add a binding, \
             then move the control you want to use.",
        );
        let mut on = self.config.midi.enabled;
        if ui.checkbox(&mut on, "Use MIDI controllers").changed() {
            self.config.midi.enabled = on;
            self.config.save();
        }
        if !self.config.midi.enabled {
            return;
        }

        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Listen to");
            let shown = if self.config.midi.device.is_empty() {
                "Every controller".to_string()
            } else {
                self.config.midi.device.clone()
            };
            egui::ComboBox::from_id_salt("midi-device")
                .selected_text(shown)
                .width(220.0)
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(
                            &mut self.config.midi.device,
                            String::new(),
                            "Every controller",
                        )
                        .changed();
                    for name in &self.midi_ports {
                        changed |= ui
                            .selectable_value(&mut self.config.midi.device, name.clone(), name)
                            .changed();
                    }
                });
        });
        if self.midi_ports.is_empty() {
            hint(
                ui,
                "No MIDI controller found. Plug one in; it is picked up in a few seconds.",
            );
        } else if let Some((e, at)) = &self.midi_last {
            let source = crate::midi::Source::of(e).label();
            hint(
                ui,
                &format!(
                    "Last message: {source}, value {} ({} s ago)",
                    e.value,
                    at.elapsed().as_secs()
                ),
            );
        } else {
            hint(ui, "Move a control to check it is heard.");
        }

        if let Some(m) = &self.midi {
            if !m.open.is_empty() {
                hint(ui, &format!("Connected: {}", m.open.join(", ")));
            }
        }

        let strips: Vec<String> = (0..STRIPS).map(|i| self.config.strip_title(i)).collect();
        let buses: Vec<String> = (0..BUSES).map(|b| self.config.bus_title(b)).collect();
        let macros: Vec<String> = self
            .config
            .macros
            .iter()
            .map(|m| m.name.clone())
            .filter(|n| !n.trim().is_empty())
            .collect();
        let presets: Vec<String> = self.config.presets.iter().map(|p| p.name.clone()).collect();
        let learning = self.midi_learning;
        let (mut remove, mut toggle_learn) = (None, None);
        for (n, b) in self.config.midi.bindings.iter_mut().enumerate() {
            egui::Frame::new()
                .fill(theme::PANEL_HI())
                .corner_radius(theme::R_CARD)
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let text = if learning == Some(n) {
                            "Move a control...".to_string()
                        } else {
                            b.source
                                .map_or("Not set \u{2013} click to learn".into(), |s| s.label())
                        };
                        if ui
                            .add(egui::Button::new(text).selected(learning == Some(n)))
                            .on_hover_text("Click, then move the fader or knob or press the pad")
                            .clicked()
                        {
                            toggle_learn = Some(n);
                        }
                        if ui.button("Remove").clicked() {
                            remove = Some(n);
                        }
                    });
                    ui.horizontal(|ui| {
                        changed |=
                            target_editor(ui, n, &mut b.target, &strips, &buses, &macros, &presets);
                    });
                });
        }
        if let Some(n) = remove {
            self.config.midi.bindings.remove(n);
            self.midi_learning = None;
            changed = true;
        }
        if let Some(n) = toggle_learn {
            self.midi_learning = if self.midi_learning == Some(n) {
                None
            } else {
                Some(n)
            };
        }
        if ui.button("Add a binding").clicked() {
            self.config.midi.bindings.push(Binding {
                source: None,
                target: Target::StripGain(0),
            });
            self.midi_learning = Some(self.config.midi.bindings.len() - 1);
            changed = true;
        }
        if changed {
            self.config.save();
        }
        hint(
            ui,
            "Faders and knobs follow the control; switches and pads act when pressed. The game / \
             chat dial and pan use the middle position as centre.",
        );
    }

    // --- network audio -------------------------------------------------------------------------

    pub(super) fn vban_section(&mut self, ui: &mut Ui) {
        section(ui, "Network audio (VBAN)");
        hint(
            ui,
            "Send an output to another PC, or take sound from one, over your network. It works with \
             Voicemeeter, VBAN Receptor and other VBAN programs. Each entry then appears in the \
             input and output device menus. Windows may ask to let Wavemix through its firewall.",
        );
        let mut changed = false;
        let (mut drop_in, mut drop_out) = (None, None);

        ui.label(RichText::new("Receive").strong());
        for (n, r) in self.config.vban.receivers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut r.stream)
                            .hint_text("any stream")
                            .desired_width(130.0),
                    )
                    .on_hover_text("Only take this stream name; leave empty to take any")
                    .changed();
                ui.label("on port");
                changed |= ui
                    .add(egui::DragValue::new(&mut r.port).range(1024..=65535))
                    .changed();
                if ui.button("Remove").clicked() {
                    drop_in = Some(n);
                }
            });
        }
        if ui.button("Add a receiver").clicked() {
            self.config.vban.receivers.push(VbanIn::default());
            changed = true;
        }

        ui.add_space(8.0);
        ui.label(RichText::new("Send").strong());
        for (n, s) in self.config.vban.senders.iter_mut().enumerate() {
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut s.stream).desired_width(100.0))
                    .on_hover_text("The stream name the other PC listens for")
                    .changed();
                ui.label("to");
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut s.host)
                            .hint_text("PC name or address")
                            .desired_width(150.0),
                    )
                    .changed();
                ui.label("port");
                changed |= ui
                    .add(egui::DragValue::new(&mut s.port).range(1024..=65535))
                    .changed();
                if ui.button("Remove").clicked() {
                    drop_out = Some(n);
                }
            });
        }
        if ui.button("Add a sender").clicked() {
            self.config.vban.senders.push(VbanOut::default());
            changed = true;
        }

        if let Some(n) = drop_in {
            self.config.vban.receivers.remove(n);
            changed = true;
        }
        if let Some(n) = drop_out {
            self.config.vban.senders.remove(n);
            changed = true;
        }
        if changed {
            *self.vban.lock().unwrap_or_else(|e| e.into_inner()) = self.config.vban.clone();
            self.config.save();
            self.engine.refresh_devices();
        }
    }
}

/// The two drop-downs that say what a binding does. Returns true when either changed.
fn target_editor(
    ui: &mut Ui,
    n: usize,
    target: &mut Target,
    strips: &[String],
    buses: &[String],
    macros: &[String],
    presets: &[String],
) -> bool {
    let mut changed = false;
    let kind = target.kind_index();
    egui::ComboBox::from_id_salt(("midi-kind", n))
        .selected_text(TARGET_KINDS[kind])
        .width(150.0)
        .show_ui(ui, |ui| {
            for (k, label) in TARGET_KINDS.iter().enumerate() {
                if ui.selectable_label(k == kind, *label).clicked() && k != kind {
                    let first = match k {
                        11 => macros.first(),
                        12 => presets.first(),
                        _ => None,
                    };
                    *target = Target::of_kind(k, first.map_or("", String::as_str));
                    changed = true;
                }
            }
        });
    match target {
        Target::StripGain(i)
        | Target::StripPan(i)
        | Target::StripMute(i)
        | Target::StripSolo(i) => changed |= index_picker(ui, ("midi-strip", n), i, strips),
        Target::BusGain(i) | Target::BusMute(i) => {
            changed |= index_picker(ui, ("midi-bus", n), i, buses)
        }
        Target::Macro(name) => {
            changed |= name_picker(ui, ("midi-macro", n), name, macros, "No macros yet")
        }
        Target::Preset(name) => {
            changed |= name_picker(ui, ("midi-preset", n), name, presets, "No presets yet")
        }
        _ => {}
    }
    changed
}

fn index_picker(ui: &mut Ui, id: (&str, usize), value: &mut usize, labels: &[String]) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(labels.get(*value).map_or("?", String::as_str))
        .show_ui(ui, |ui| {
            for (i, label) in labels.iter().enumerate() {
                changed |= ui.selectable_value(value, i, label).changed();
            }
        });
    changed
}

fn name_picker(
    ui: &mut Ui,
    id: (&str, usize),
    value: &mut String,
    choices: &[String],
    none: &str,
) -> bool {
    let mut changed = false;
    let shown = if value.is_empty() {
        none
    } else {
        value.as_str()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for name in choices {
                changed |= ui.selectable_value(value, name.clone(), name).changed();
            }
        });
    changed
}
