//! "Switch from Voicemeeter": bring a Voicemeeter setup over to wavemix and take Voicemeeter out of
//! the way, in one go. Everything is opt-in per step, and the order is chosen so that sound never
//! has nowhere to go: Windows' default devices move off Voicemeeter's virtual ones *before*
//! Voicemeeter is closed.

use eframe::egui::{self, RichText, Ui};

use super::{theme, MixerApp};
use crate::model::{Command, Device, Flow};
use crate::voicemeeter::{plan_import, VmState};

/// Which steps to carry out.
#[derive(Clone, Copy)]
pub struct SwitchOptions {
    pub import: bool,
    pub swap_defaults: bool,
    pub close_voicemeeter: bool,
    pub stop_voicemeeter_startup: bool,
    pub start_with_windows: bool,
}

impl Default for SwitchOptions {
    fn default() -> Self {
        Self {
            import: true,
            swap_defaults: true,
            close_voicemeeter: true,
            stop_voicemeeter_startup: true,
            start_with_windows: true,
        }
    }
}

#[derive(Default)]
pub struct SwitchState {
    /// The switch page is showing.
    pub open: bool,
    pub opts: SwitchOptions,
    /// What happened, once it has run.
    pub report: Option<Vec<String>>,
}

/// The Windows device a Voicemeeter-style name refers to.
fn find_by_name<'a>(devices: &'a [Device], name: &str, flow: Flow) -> Option<&'a Device> {
    let wanted = name.to_lowercase();
    let mut of_flow = devices.iter().filter(|d| d.flow == flow);
    of_flow
        .clone()
        .find(|d| d.name.to_lowercase() == wanted)
        .or_else(|| of_flow.find(|d| d.name.to_lowercase().starts_with(&wanted)))
}

impl MixerApp {
    /// Whether to nudge the user toward switching over.
    pub(super) fn offer_switch(&self) -> bool {
        self.vm_installed && self.config.voicemeeter_offer && !self.switch.open
    }

    /// The small card offered at the top of the window when Voicemeeter is installed.
    pub(super) fn switch_banner(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0, theme::ACCENT))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Using Voicemeeter?").strong());
                ui.label(
                    RichText::new("Bring your setup over to wavemix in one click.")
                        .size(12.0)
                        .color(theme::TEXT_DIM),
                );
                ui.horizontal(|ui| {
                    if ui.button("Switch over").clicked() {
                        self.switch.open = true;
                        self.show_settings = false;
                    }
                    if ui.small_button("Not now").clicked() {
                        self.config.voicemeeter_offer = false;
                        self.config.save();
                    }
                });
            });
        ui.add_space(8.0);
    }

    /// The switch page inside the small Simple window.
    pub(super) fn switch_inline(&mut self, ui: &mut Ui) {
        self.simple_height.set(super::SIMPLE_MAX_H);
        ui.horizontal(|ui| {
            if ui.button("Back").clicked() {
                self.switch.open = false;
            }
            ui.label(RichText::new("Switch from Voicemeeter").size(15.0).strong());
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("switch-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| self.switch_body(ui));
    }

    /// The switch page as a window, for Pro mode.
    pub(super) fn switch_window(&mut self, ctx: &egui::Context) {
        if self.view() == crate::config::View::Compact {
            return;
        }
        let mut open = self.switch.open;
        egui::Window::new("Switch from Voicemeeter")
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(460.0)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| self.switch_body(ui));
        self.switch.open = open;
    }

    pub(super) fn switch_body(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.0;
        if let Some(report) = self.switch.report.clone() {
            ui.label(
                RichText::new("Done")
                    .size(15.0)
                    .strong()
                    .color(theme::METER_GREEN),
            );
            for line in &report {
                ui.label(RichText::new(line).size(12.0));
            }
            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                self.switch.open = false;
                self.switch.report = None;
            }
            return;
        }

        ui.label(
            "Copies your Voicemeeter strips, buses and routing into wavemix, then moves Windows \
             over so nothing goes quiet when Voicemeeter closes.",
        );
        if !self.vm_installed {
            ui.label(
                RichText::new("Voicemeeter doesn't seem to be installed on this PC.")
                    .color(theme::METER_YELLOW),
            );
        } else {
            ui.label(
                RichText::new(
                    "Voicemeeter needs to be running so its current settings can be read.",
                )
                .size(12.0)
                .color(theme::TEXT_DIM),
            );
        }
        ui.add_space(6.0);
        let o = &mut self.switch.opts;
        ui.checkbox(
            &mut o.import,
            "Import my strips, buses, routing, EQ and devices",
        )
        .on_hover_text(
            "Gain, mute, solo, mono, balance, A/B routing, bass/mid/treble, compressor and gate. \
                 Bus EQ, delays and macros aren't imported.",
        );
        ui.checkbox(
            &mut o.swap_defaults,
            "Point Windows at my real speakers and microphone instead of Voicemeeter's",
        )
        .on_hover_text(
            "If Windows' default devices are Voicemeeter's virtual ones, they're changed to your real \
             hardware (or your virtual cable) so audio keeps working once Voicemeeter closes.",
        );
        ui.checkbox(&mut o.close_voicemeeter, "Close Voicemeeter");
        ui.checkbox(
            &mut o.stop_voicemeeter_startup,
            "Stop Voicemeeter starting with Windows",
        );
        ui.checkbox(&mut o.start_with_windows, "Start wavemix with Windows");
        ui.add_space(6.0);
        ui.label(
            RichText::new(
                "Voicemeeter's own virtual inputs can't be captured by other programs. For apps you \
                 sent into Voicemeeter, install a virtual cable (VB-Cable, free) and wavemix will \
                 use it in their place.",
            )
            .size(11.0)
            .color(theme::TEXT_DIM),
        );
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let enabled = self.vm_installed;
            if ui
                .add_enabled(
                    enabled,
                    egui::Button::new(RichText::new("Switch now").strong()),
                )
                .clicked()
            {
                let report = self.run_switch();
                self.switch.report = Some(report);
            }
            if ui.button("Cancel").clicked() {
                self.switch.open = false;
            }
        });
    }

    /// Carry out the chosen steps and describe what happened.
    fn run_switch(&mut self) -> Vec<String> {
        let opts = self.switch.opts;

        // 1. Read Voicemeeter.
        #[cfg(windows)]
        let (state, remote): (VmState, Option<crate::voicemeeter::remote::Remote>) =
            match crate::voicemeeter::remote::Remote::connect()
                .and_then(|r| r.read_state().map(|s| (s, r)))
            {
                Ok((s, r)) => (s, Some(r)),
                Err(e) => return vec![format!("Couldn't read Voicemeeter: {e}")],
            };
        #[cfg(not(windows))]
        let (state, remote): (VmState, Option<()>) =
            (crate::voicemeeter::import::demo_state(), None);

        // 2. Work out the plan.
        let devices = self.engine.devices();
        let snap = self.backend.snapshot();
        let default_of = |flow| {
            snap.devices
                .iter()
                .find(|d| d.flow == flow && d.is_default)
                .map(|d| d.name.clone())
        };
        let plan = plan_import(
            &state,
            &devices.inputs,
            &devices.outputs,
            &self.config.engine.io,
            default_of(Flow::Render).as_deref(),
            default_of(Flow::Capture).as_deref(),
        );
        let mut report = Vec::new();

        // 3. Import.
        if opts.import {
            for (slot, dev) in plan.io.inputs.iter().enumerate() {
                self.engine.set_input(slot, dev.clone());
            }
            for (bus, dev) in plan.io.outputs.iter().enumerate() {
                self.engine.set_output(bus, dev.clone());
            }
            self.engine.set_params(plan.params.clone());
            self.config.engine.params = plan.params.clone();
            self.config.engine.io = plan.io.clone();
            self.config.engine.io.engine_on = true;
            self.config.engine.configured = true;
            self.engine.set_running(true);
            self.mark_engine_dirty();
            report.extend(plan.notes.iter().cloned());
        }

        // 4. Move Windows off Voicemeeter's devices first, so closing it can't silence anything.
        if opts.swap_defaults {
            for (name, flow, what) in [
                (&plan.default_playback, Flow::Render, "playback"),
                (&plan.default_recording, Flow::Capture, "recording"),
            ] {
                if let Some(name) = name {
                    match find_by_name(&snap.devices, name, flow) {
                        Some(d) => {
                            self.backend.send(Command::SetDefaultDevice {
                                id: d.id.clone(),
                                flow,
                            });
                            report.push(format!("Windows' default {what} device is now \"{}\".", d.name));
                        }
                        None => report.push(format!(
                            "Couldn't switch the default {what} device to \"{name}\"; change it in Windows' sound settings."
                        )),
                    }
                }
            }
        }

        // 5. Close Voicemeeter and stop it launching.
        #[cfg(windows)]
        {
            if opts.close_voicemeeter {
                match &remote {
                    Some(r) if r.shutdown() => report.push("Closed Voicemeeter.".into()),
                    _ => {
                        report.push("Couldn't close Voicemeeter; quit it from its own menu.".into())
                    }
                }
            }
            if opts.stop_voicemeeter_startup {
                let removed = crate::voicemeeter::remote::disable_autostart();
                if removed.is_empty() {
                    report.push("Voicemeeter wasn't set to start with Windows.".into());
                } else {
                    report.push("Voicemeeter will no longer start with Windows.".into());
                }
            }
            if opts.start_with_windows {
                match crate::startup::set_enabled(true) {
                    Ok(()) => {
                        self.config.run_at_startup = true;
                        report.push("wavemix will start with Windows.".into());
                    }
                    Err(e) => {
                        report.push(format!("Couldn't set wavemix to start with Windows: {e}"))
                    }
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = remote;
            report
                .push("(Closing Voicemeeter and the startup steps only happen on Windows.)".into());
        }

        // Done: stop nudging.
        self.config.voicemeeter_offer = false;
        self.config.save();
        report
    }
}
