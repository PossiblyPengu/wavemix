//! The presets menu and the settings window.

use eframe::egui::{self, RichText, Ui};

use super::{theme, MixerApp, Mode};
use crate::model::Snapshot;
use crate::preset;

impl MixerApp {
    pub(super) fn presets_menu(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui.menu_button("Presets", |ui| {
            if self.config.presets.is_empty() {
                ui.label(RichText::new("No presets yet").color(theme::TEXT_DIM));
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
                    Some(existing) => *existing = p,
                    None => self.config.presets.push(p),
                }
                self.config.save();
                self.preset_name.clear();
                ui.close();
            }
        });
    }

    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if self.mode == Mode::Flyout {
            return;
        }
        let mut open = self.show_settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| self.settings_body(ui));
        self.show_settings = open;
    }

    fn settings_body(&mut self, ui: &mut Ui) {
        #[cfg(windows)]
        {
            let mut on = self.config.run_at_startup;
            if ui.checkbox(&mut on, "Start with Windows").changed() {
                match crate::startup::set_enabled(on) {
                    Ok(()) => {
                        self.config.run_at_startup = on;
                        self.config.save();
                    }
                    Err(e) => self.shell.hk_status = vec![format!("Couldn't change startup: {e}")],
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Global hotkeys").strong());
            ui.label(
                RichText::new("Act on the default playback device. Examples: ctrl+alt+up, win+f9")
                    .size(11.0)
                    .color(theme::TEXT_DIM),
            );
            egui::Grid::new("hotkeys").num_columns(2).show(ui, |ui| {
                ui.label("Volume up");
                ui.text_edit_singleline(&mut self.hk_edit.volume_up);
                ui.end_row();
                ui.label("Volume down");
                ui.text_edit_singleline(&mut self.hk_edit.volume_down);
                ui.end_row();
                ui.label("Mute");
                ui.text_edit_singleline(&mut self.hk_edit.mute);
                ui.end_row();
            });
            if ui.button("Apply hotkeys").clicked() {
                self.apply_hotkeys_from_edit();
            }
            for msg in &self.shell.hk_status {
                ui.label(RichText::new(msg).size(11.0).color(theme::MUTE));
            }
            ui.add_space(8.0);
        }

        let mut step = self.config.volume_step;
        if ui
            .add(egui::Slider::new(&mut step, 1..=20).text("Hotkey step %"))
            .changed()
        {
            self.config.volume_step = step;
            self.config.save();
        }

        ui.add_space(8.0);
        let mut routing = self.config.experimental_routing;
        if ui
            .checkbox(&mut routing, "Per-app output routing (experimental)")
            .changed()
        {
            self.config.experimental_routing = routing;
            self.config.save();
        }
        ui.label(
            RichText::new(
                "Adds an Output menu to each app. Uses an undocumented Windows interface that may \
                 not work on every Windows version.",
            )
            .size(11.0)
            .color(theme::TEXT_DIM),
        );

        if !self.config.presets.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Presets").strong());
            let mut delete = None;
            for (i, p) in self.config.presets.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&p.name);
                    if ui.small_button("Delete").clicked() {
                        delete = Some(i);
                    }
                });
            }
            if let Some(i) = delete {
                self.config.presets.remove(i);
                self.config.save();
            }
        }
    }
}
