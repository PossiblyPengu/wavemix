//! The presets menu and the settings window.

use eframe::egui::{self, RichText, Ui};

use super::{strip, theme, MixerApp};
#[cfg(windows)]
use crate::config::OsdPosition;
use crate::config::ThemeMode;
use crate::model::Snapshot;
use crate::preset;

impl MixerApp {
    pub(super) fn presets_menu(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui.menu_button("Presets", |ui| {
            if self.config.presets.is_empty() {
                ui.label(RichText::new("No presets yet").color(theme::TEXT_DIM()));
            }
            let mut apply = None;
            for (i, p) in self.config.presets.iter().enumerate() {
                if ui.button(&p.name).clicked() {
                    apply = Some(i);
                    ui.close();
                }
            }
            if let Some(i) = apply {
                for c in preset::commands(&self.config.presets[i], snap) {
                    self.backend.send(c);
                }
            }
            ui.separator();
            ui.label(RichText::new("Save current volumes as").size(11.0));
            ui.text_edit_singleline(&mut self.preset_name);
            let name = self.preset_name.trim().to_string();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("Save"))
                .clicked()
            {
                let p = preset::capture(&name, snap);
                match self.config.presets.iter_mut().find(|e| e.name == name) {
                    Some(existing) => {
                        // Saving over a preset keeps what triggers it.
                        let trigger = existing.trigger.clone();
                        *existing = preset::Preset { trigger, ..p };
                    }
                    None => self.config.presets.push(p),
                }
                self.config.save();
                self.preset_name.clear();
                ui.close();
            }
        });
    }

    /// Settings inside the small Simple window: they replace the app list until you go back.
    pub(super) fn settings_inline(&mut self, ui: &mut Ui) {
        // Settings are taller than the app list usually is; let the flyout use its full height.
        self.simple_height.set(super::SIMPLE_MAX_H);
        ui.horizontal(|ui| {
            if ui.button("Back").clicked() {
                self.show_settings = false;
            }
            ui.label(RichText::new("Settings").size(15.0).strong());
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("settings-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| self.settings_body(ui));
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        // Simple mode shows settings inline instead.
        if self.view() == crate::config::View::Compact {
            return;
        }
        let mut open = self.show_settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(400.0)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| self.settings_body(ui));
        self.show_settings = open;
    }

    fn settings_body(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.0;

        section(ui, "General");
        #[cfg(windows)]
        {
            let mut on = self.config.run_at_startup;
            if ui
                .checkbox(&mut on, "Start with Windows")
                .on_hover_text("Launches quietly into the tray when you sign in")
                .changed()
            {
                match crate::startup::set_enabled(on) {
                    Ok(()) => {
                        self.config.run_at_startup = on;
                        self.config.save();
                    }
                    Err(e) => self.shell.hk_status = vec![format!("Couldn't change startup: {e}")],
                }
            }
        }
        ui.horizontal(|ui| {
            ui.label("Theme");
            let mut mode = self.config.theme;
            egui::ComboBox::from_id_salt("theme-mode")
                .selected_text(match mode {
                    ThemeMode::System => "Match Windows",
                    ThemeMode::Light => "Light",
                    ThemeMode::Dark => "Dark",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut mode, ThemeMode::System, "Match Windows");
                    ui.selectable_value(&mut mode, ThemeMode::Light, "Light");
                    ui.selectable_value(&mut mode, ThemeMode::Dark, "Dark");
                });
            if mode != self.config.theme {
                self.config.theme = mode;
                self.config.save();
                self.theme_dirty = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("Interface size");
            let mut scale = self.config.ui_scale;
            egui::ComboBox::from_id_salt("ui-scale")
                .selected_text(format!("{:.0}%", scale * 100.0))
                .show_ui(ui, |ui| {
                    for pct in [90, 100, 115, 130, 150] {
                        ui.selectable_value(&mut scale, pct as f32 / 100.0, format!("{pct}%"));
                    }
                });
            if (scale - self.config.ui_scale).abs() > f32::EPSILON {
                self.config.ui_scale = scale;
                self.config.save();
                ui.ctx().set_zoom_factor(scale);
                // The Simple flyout sizes itself; only the Pro window needs resizing.
                if self.config.view == crate::config::View::Console {
                    let [w, h] = Self::size_for(&self.config);
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
                }
            }
        })
        .response
        .on_hover_text("Makes everything bigger or smaller. Taskbar popup sizes follow.");
        self.update_settings(ui);
        let mut auto = self.config.auto_route;
        if ui
            .checkbox(
                &mut auto,
                "Route Windows audio through the mixer when the engine is on",
            )
            .changed()
        {
            self.config.auto_route = auto;
            self.config.save();
        }
        hint(
            ui,
            "Pro mode only sees sound that goes through the virtual cable. Your normal speakers \
             are put back when the engine stops or Wavemix quits.",
        );
        let mut routing = self.config.experimental_routing;
        if ui
            .checkbox(&mut routing, "Let me send each app to a different output")
            .changed()
        {
            self.config.experimental_routing = routing;
            self.config.save();
        }
        hint(
            ui,
            "Experimental. Click the device pill under any app to choose where it plays. Uses an \
             undocumented Windows interface that may not work on every Windows version.",
        );

        #[cfg(windows)]
        {
            section(ui, "Keyboard shortcuts");
            hint(
                ui,
                "They work even when Wavemix is hidden. Click Record, then press the keys you want. \
                 For your keyboard's own volume keys, use the button below; Windows' volume popup \
                 then stays away, since Wavemix has taken the keys.",
            );
            if ui
                .button("Use my keyboard's volume keys")
                .on_hover_text(
                    "Volume keys change the main volume; Ctrl + volume keys change the app in front",
                )
                .clicked()
            {
                self.hk_edit = self.hk_edit.volume_keys();
                self.apply_hotkeys_from_edit();
            }
            // Recording: the next key combination pressed is written into the box being recorded.
            let recorded = self.recording.and_then(|_| {
                ui.ctx().input(|i| {
                    i.events.iter().find_map(|e| match e {
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => Some((*key, *modifiers)),
                        _ => None,
                    })
                })
            });
            let mut finished = false;
            egui::Grid::new("hotkeys")
                .num_columns(3)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for (i, (label, text)) in self.hk_edit.entries_mut().into_iter().enumerate() {
                        ui.label(label);
                        let recording = self.recording == Some(i);
                        if recording {
                            if let Some((key, mods)) = recorded {
                                if key == egui::Key::Escape {
                                    finished = true;
                                } else if let Some(combo) = key_combo(key, mods) {
                                    *text = combo;
                                    finished = true;
                                } else {
                                    self.shell.hk_status =
                                        vec![format!("{} can't be used as a shortcut", key.name())];
                                }
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut String::from("press the keys…"))
                                    .interactive(false)
                                    .desired_width(150.0),
                            );
                        } else {
                            ui.add(egui::TextEdit::singleline(text).desired_width(150.0));
                        }
                        let button = if recording { "Cancel" } else { "Record" };
                        if ui.small_button(button).clicked() {
                            self.recording = if recording { None } else { Some(i) };
                        }
                        ui.end_row();
                    }
                });
            if finished {
                self.recording = None;
            }
            let mut step = self.config.volume_step;
            if ui
                .add(egui::Slider::new(&mut step, 1..=20).text("Step per press, %"))
                .changed()
            {
                self.config.volume_step = step;
                self.config.save();
            }
            if ui.button("Apply hotkeys").clicked() {
                self.apply_hotkeys_from_edit();
            }
            for msg in &self.shell.hk_status {
                ui.label(RichText::new(msg).size(11.0).color(theme::MUTE()));
            }

            section(ui, "On-screen display");
            hint(
                ui,
                "A bar that pops out of the tray icon whenever the volume changes, from a hotkey, a \
                 keyboard knob or anything else. Click it to open wavemix, scroll it to change the \
                 volume, middle-click it to mute, or drag it where you like.",
            );
            let mut osd = self.config.osd;
            let mut vertical = self.config.osd_vertical;
            let mut opacity = self.config.osd_opacity;
            let mut timeout = self.config.osd_timeout_ms;
            let mut accent = self.config.osd_accent_system;
            let mut hide_win = self.config.hide_windows_osd;
            let mut position = self.config.osd_position;
            let mut changed = ui.checkbox(&mut osd, "Show the volume bar").changed();
            ui.horizontal(|ui| {
                ui.label("Position");
                egui::ComboBox::from_id_salt("osd-position")
                    .selected_text(match position {
                        OsdPosition::AboveTray => "By the tray icon",
                        OsdPosition::Centre => "Bottom centre",
                        OsdPosition::Custom => "Where I dragged it",
                    })
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut position,
                                OsdPosition::AboveTray,
                                "By the tray icon",
                            )
                            .changed();
                        changed |= ui
                            .selectable_value(&mut position, OsdPosition::Centre, "Bottom centre")
                            .changed();
                        changed |= ui
                            .selectable_value(
                                &mut position,
                                OsdPosition::Custom,
                                "Where I dragged it",
                            )
                            .changed();
                    });
            });
            changed |= ui.checkbox(&mut vertical, "Vertical bar").changed();
            changed |= ui
                .checkbox(&mut accent, "Match the Windows accent colour")
                .changed();
            changed |= ui
                .checkbox(&mut hide_win, "Hide Windows' own volume popup")
                .on_hover_text(
                    "Stops the Windows volume overlay appearing on top of this bar. Best effort: \
                     it hides the overlay if Windows opens it. If it still shows, tell me, and \
                     send %TEMP%\\wavemix-winosd.txt.",
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut opacity, 40..=100).text("Opacity %"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut timeout, 500..=5000)
                        .text("Stays up (ms)")
                        .step_by(100.0),
                )
                .changed();
            if changed {
                self.config.osd = osd;
                self.config.osd_vertical = vertical;
                self.config.osd_opacity = opacity;
                self.config.osd_timeout_ms = timeout;
                self.config.osd_accent_system = accent;
                self.config.hide_windows_osd = hide_win;
                self.config.osd_position = position;
                self.config.save();
                self.sync_osd();
            }
        }

        section(ui, "Setup check");
        self.setup_check(ui);

        section(ui, "Names");
        hint(
            ui,
            "Call your devices whatever you like; the new name shows everywhere. Double-click a \
             strip or bus name in Pro mode to rename it. Leave a box empty for the default.",
        );
        {
            let snap = self.backend.snapshot();
            let all: Vec<(String, String)> = snap
                .devices
                .iter()
                .map(|d| (d.id.clone(), d.name.clone()))
                .collect();
            let mut changed = false;
            for (id, name) in all {
                let mut text = self
                    .config
                    .device_names
                    .get(&id)
                    .cloned()
                    .unwrap_or_default();
                ui.vertical(|ui| {
                    let hint_text = strip::short_device_name(&name).to_string();
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .hint_text(hint_text)
                            .desired_width(ui.available_width().min(260.0))
                            .char_limit(32),
                    );
                    ui.label(RichText::new(&name).size(11.0).color(theme::TEXT_DIM()));
                    if edit.changed() {
                        changed = true;
                    }
                });
                if changed {
                    if text.trim().is_empty() {
                        self.config.device_names.remove(&id);
                    } else {
                        self.config.device_names.insert(id, text);
                    }
                    break;
                }
            }
            if changed {
                self.config.save();
                self.sync_osd_names();
            }
        }

        section(ui, "Switching from Voicemeeter");
        hint(
            ui,
            "Bring your Voicemeeter strips, buses and devices over, and point Windows at your real \
             devices so nothing goes quiet when Voicemeeter closes.",
        );
        if ui.button("Switch from Voicemeeter...").clicked() {
            self.switch.open = true;
            self.show_settings = false;
        }

        section(ui, "Presets");
        hint(
            ui,
            "A preset remembers every app and device volume and mute, so you can switch between, say, \
             a gaming and a calling setup in one click.",
        );
        let snap = self.backend.snapshot();
        let mut apply = None;
        let mut delete = None;
        // What a trigger can be set to: apps seen on this PC and playback devices.
        let apps: Vec<(String, String)> = self
            .all_apps(&snap, true)
            .into_iter()
            .filter(|a| !a.key.starts_with("pid:"))
            .map(|a| (a.key, a.name))
            .collect();
        let outputs: Vec<(String, String)> = snap
            .devices
            .iter()
            .filter(|d| d.flow == crate::model::Flow::Render)
            .map(|d| {
                (
                    d.id.clone(),
                    self.config.device_alias(&d.id).map_or_else(
                        || strip::short_device_name(&d.name).to_string(),
                        str::to_string,
                    ),
                )
            })
            .collect();
        let mut triggers: Vec<(usize, preset::Trigger)> = Vec::new();
        for (i, p) in self.config.presets.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(&p.name);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Delete").clicked() {
                        delete = Some(i);
                    }
                    if ui.small_button("Apply").clicked() {
                        apply = Some(i);
                    }
                });
            });
            // When it should apply by itself.
            let describe = |t: &preset::Trigger| match t {
                preset::Trigger::Never => "Apply only when I choose".to_string(),
                preset::Trigger::AppPlays(k) => format!(
                    "When {} starts playing",
                    apps.iter()
                        .find(|a| &a.0 == k)
                        .map_or(k.as_str(), |a| a.1.as_str())
                ),
                preset::Trigger::DeviceDefault(id) => format!(
                    "When {} becomes the default",
                    outputs
                        .iter()
                        .find(|o| &o.0 == id)
                        .map_or("a device", |o| o.1.as_str())
                ),
            };
            let mut trigger = p.trigger.clone();
            egui::ComboBox::from_id_salt(("trigger", i))
                .selected_text(describe(&trigger))
                .width(260.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut trigger,
                        preset::Trigger::Never,
                        "Apply only when I choose",
                    );
                    ui.separator();
                    for (key, name) in &apps {
                        ui.selectable_value(
                            &mut trigger,
                            preset::Trigger::AppPlays(key.clone()),
                            format!("When {name} starts playing"),
                        );
                    }
                    ui.separator();
                    for (id, name) in &outputs {
                        ui.selectable_value(
                            &mut trigger,
                            preset::Trigger::DeviceDefault(id.clone()),
                            format!("When {name} becomes the default"),
                        );
                    }
                });
            if trigger != p.trigger {
                triggers.push((i, trigger));
            }
            ui.add_space(4.0);
        }
        for (i, t) in triggers {
            self.config.presets[i].trigger = t;
            self.config.save();
        }
        if let Some(i) = apply {
            for c in preset::commands(&self.config.presets[i], &snap) {
                self.backend.send(c);
            }
        }
        if let Some(i) = delete {
            self.config.presets.remove(i);
            self.config.save();
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.preset_name)
                    .hint_text("Name this setup")
                    .desired_width(180.0),
            );
            let name = self.preset_name.trim().to_string();
            if ui
                .add_enabled(!name.is_empty(), egui::Button::new("Save current"))
                .clicked()
            {
                let p = preset::capture(&name, &snap);
                match self.config.presets.iter_mut().find(|e| e.name == name) {
                    Some(existing) => {
                        // Saving over a preset keeps what triggers it.
                        let trigger = existing.trigger.clone();
                        *existing = preset::Preset { trigger, ..p };
                    }
                    None => self.config.presets.push(p),
                }
                self.config.save();
                self.preset_name.clear();
            }
        });
    }
}

impl MixerApp {
    /// A short checklist for the parts of the setup that most often go wrong, each with its fix.
    fn setup_check(&mut self, ui: &mut Ui) {
        let devices = self.engine.devices();
        let cable_installed = devices
            .inputs
            .iter()
            .any(|d| d.name.contains("CABLE Output"));
        let (routed, cable_in) = self.route_state();
        let running = self.engine.is_running();

        let row = |ui: &mut Ui, ok: bool, text: &str| {
            ui.horizontal(|ui| {
                let (mark, colour) = if ok {
                    ("✔", theme::METER_GREEN())
                } else {
                    ("✖", theme::NOTE_TEXT())
                };
                ui.label(RichText::new(mark).color(colour));
                ui.label(text);
            });
        };

        row(
            ui,
            cable_installed,
            "A virtual cable (VB-Cable) is installed",
        );
        if !cable_installed {
            ui.hyperlink_to("Get VB-Cable (free)", "https://vb-audio.com/Cable/");
        }
        row(
            ui,
            cable_installed && cable_in.is_some(),
            "The cable's playback side (CABLE Input) is enabled",
        );
        if cable_installed && cable_in.is_none() {
            hint(
                ui,
                "In Windows sound settings, Playback tab: right-click, tick Show Disabled Devices, then enable CABLE Input.",
            );
            if ui.button("Open Windows sound settings").clicked() {
                open_sound_settings();
            }
        }
        row(ui, running, "The mixing engine is on");
        if !running && ui.button("Turn the engine on").clicked() {
            self.engine.set_running(true);
            self.config.engine.io.engine_on = true;
            self.mark_engine_dirty();
        }
        row(ui, routed, "Windows sound goes through Wavemix");
        if running && !routed {
            if let Some(cable) = cable_in {
                if ui.button("Route Windows audio").clicked() {
                    self.route_windows_audio(&cable);
                }
            }
        }
    }
}

/// Windows' classic Sound control panel.
fn open_sound_settings() {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("control")
            .arg("mmsys.cpl")
            .spawn();
    }
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(16.0);
    ui.label(RichText::new(title).size(15.0).strong());
    ui.add_space(2.0);
    ui.separator();
    ui.add_space(4.0);
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(theme::TEXT_DIM()));
}

/// An egui key plus modifiers as a shortcut string like `ctrl+alt+Up`, or `None` for keys that
/// can't be bound on their own (modifiers) or aren't named in a way the shortcut parser knows.
#[cfg(windows)]
fn key_combo(key: egui::Key, mods: egui::Modifiers) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if mods.ctrl {
        parts.push("ctrl".into());
    }
    if mods.alt {
        parts.push("alt".into());
    }
    if mods.shift {
        parts.push("shift".into());
    }
    if mods.mac_cmd || mods.command && !mods.ctrl {
        parts.push("super".into());
    }
    parts.push(key.name().replace(' ', ""));
    let text = parts.join("+");
    std::str::FromStr::from_str(&text)
        .ok()
        .map(|_: global_hotkey::hotkey::HotKey| text)
}
