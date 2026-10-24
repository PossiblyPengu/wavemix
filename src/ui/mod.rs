//! The mixer window. Two views of the same state: the console (apps on the left, outputs and
//! microphones on the right, as in Voicemeeter) and the compact list (EarTrumpet-style).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, vec2, Align, Id, Layout, RichText, TextureHandle, Ui};

use crate::backend::Handle;
use crate::config::{Config, View};
use crate::engine::runtime::Engine;
use crate::model::{group_sessions, AppStrip, Command, Flow, IconData, Snapshot};

mod compact;
mod console;
mod settings;
#[cfg(windows)]
mod shell;
mod strip;
mod switch;
mod theme;

use strip::StripOutput;

/// Pro mode: the full console.
const PRO_SIZE: [f32; 2] = [1360.0, 840.0];
/// Simple mode: a small volume control.
const SIMPLE_SIZE: [f32; 2] = [370.0, 560.0];
/// The flyout is as tall as its content, within these limits (points).
#[cfg_attr(not(windows), allow(dead_code))]
const SIMPLE_MIN_H: f32 = 240.0;
#[cfg_attr(not(windows), allow(dead_code))]
const SIMPLE_MAX_H: f32 = 640.0;
/// Width of the per-app panel beside the Pro console, and the gap before it.
const SIDE_PANEL_W: f32 = 380.0;
const SIDE_PANEL_GAP: f32 = 12.0;

pub struct MixerApp {
    backend: Handle,
    engine: Engine,
    /// Strip whose processing editor (EQ / gate / compressor) is open, if any.
    editing: Option<(usize, console::EditorTab)>,
    /// When settings last changed and haven't been written to disk yet.
    engine_dirty_at: Option<std::time::Instant>,
    /// Startup check for a left-over cable default has run.
    route_checked: bool,
    /// Auto-route has already happened for this engine run.
    auto_routed: bool,
    config: Config,
    demo: bool,
    /// Height Simple mode wants to be, in points, measured from its content each frame. The
    /// taskbar flyout resizes itself to match.
    simple_height: std::cell::Cell<f32>,
    /// Device the compact view's top card controls; `None` follows the default playback device.
    compact_device: Option<String>,
    #[cfg_attr(not(windows), allow(dead_code))]
    mode: Mode,
    /// App icons uploaded as textures, by app key. Filled lazily while drawing.
    icons: RefCell<HashMap<String, TextureHandle>>,
    show_settings: bool,
    /// Voicemeeter is installed, so offer to switch over.
    vm_installed: bool,
    switch: switch::SwitchState,
    preset_name: String,
    /// Hotkey text being edited in settings; applied with the Apply button.
    #[cfg(windows)]
    hk_edit: crate::config::Hotkeys,
    #[cfg(windows)]
    shell: shell::ShellState,
}

/// A normal window, or the borderless popup that appears from the tray icon.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Window,
    #[cfg_attr(not(windows), allow(dead_code))]
    Flyout,
}

impl MixerApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        backend: Handle,
        engine: Engine,
        config: Config,
        demo: bool,
        background: bool,
    ) -> Self {
        theme::apply(&cc.egui_ctx);
        #[cfg(not(windows))]
        let _ = background;
        #[cfg(windows)]
        let hk_edit = config.hotkeys.clone();
        // Simple mode lives in the tray and rises from the taskbar, so its window starts hidden;
        // so does a launch at login.
        #[cfg(windows)]
        let start_hidden = background || config.view == View::Compact;
        #[cfg(windows)]
        let shell = shell::ShellState::new(
            &cc.egui_ctx,
            start_hidden,
            background,
            backend.clone(),
            &config,
        );
        #[cfg(windows)]
        let flyout_capable = config.view == View::Compact && shell.has_tray();
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut app = Self {
            backend,
            engine,
            // `--editor eq|gate|comp` opens Input 1's processing editor on launch.
            editing: std::env::args()
                .skip_while(|a| a != "--editor")
                .nth(1)
                .and_then(|t| match t.as_str() {
                    "eq" => Some((0, console::EditorTab::Eq)),
                    "gate" => Some((0, console::EditorTab::Gate)),
                    "comp" => Some((0, console::EditorTab::Comp)),
                    _ => None,
                }),
            engine_dirty_at: None,
            route_checked: false,
            auto_routed: false,
            config,
            demo,
            simple_height: std::cell::Cell::new(SIMPLE_SIZE[1]),
            compact_device: None,
            mode: Mode::Window,
            icons: RefCell::default(),
            // `--settings` opens the settings window on launch.
            show_settings: std::env::args().any(|a| a == "--settings"),
            // Off Windows (the demo) pretend it is installed, so the flow can be tried.
            #[cfg(windows)]
            vm_installed: crate::voicemeeter::remote::installed(),
            #[cfg(not(windows))]
            vm_installed: true,
            switch: switch::SwitchState {
                // `--switch` opens the switch page on launch.
                open: std::env::args().any(|a| a == "--switch"),
                ..Default::default()
            },
            preset_name: String::new(),
            #[cfg(windows)]
            hk_edit,
            #[cfg(windows)]
            shell,
        };
        #[cfg(windows)]
        {
            if flyout_capable {
                app.mode = Mode::Flyout;
            }
            app.apply_hotkeys();
        }
        app
    }

    /// Window size for the current mode (Pro grows to make room for its Apps panel).
    pub fn size_for(config: &Config) -> [f32; 2] {
        match config.view {
            View::Console if config.pro_apps_panel => {
                [PRO_SIZE[0] + SIDE_PANEL_W + SIDE_PANEL_GAP, PRO_SIZE[1]]
            }
            View::Console => PRO_SIZE,
            View::Compact => SIMPLE_SIZE,
        }
    }

    /// Which mode's interface to draw.
    fn view(&self) -> View {
        self.config.view
    }

    fn set_view(&mut self, ctx: &egui::Context, view: View) {
        if self.config.view != view {
            self.config.view = view;
            self.config.save();
            // On Windows the shell turns the window into the flyout (Simple) or the full window
            // (Pro); elsewhere it is one window that just changes size.
            #[cfg(windows)]
            self.request(ctx, shell::Request::Apply);
            #[cfg(not(windows))]
            {
                let [w, h] = Self::size_for(&self.config);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
            }
        }
    }

    // --- shared helpers ------------------------------------------------------------------------

    /// The texture for an app's icon, uploading it the first time it is seen.
    fn icon(
        &self,
        ctx: &egui::Context,
        key: &str,
        data: &Option<Arc<IconData>>,
    ) -> Option<TextureHandle> {
        let data = data.as_ref()?;
        let mut cache = self.icons.borrow_mut();
        if let Some(t) = cache.get(key) {
            return Some(t.clone());
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([data.width, data.height], &data.rgba);
        let tex = ctx.load_texture(format!("icon-{key}"), image, egui::TextureOptions::LINEAR);
        cache.insert(key.to_string(), tex.clone());
        Some(tex)
    }

    /// Every app that's using any playback device, one entry per (app, device), playing first.
    fn all_apps(&self, snap: &Snapshot, include_idle: bool) -> Vec<AppStrip> {
        let mut apps: Vec<AppStrip> = snap
            .devices
            .iter()
            .filter(|d| d.flow == Flow::Render)
            .flat_map(|d| group_sessions(&snap.sessions, &d.id))
            .filter(|a| include_idle || a.active)
            .collect();
        apps.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        apps
    }

    fn send_app(&self, app: &AppStrip, out: StripOutput) {
        for (device_id, id) in &app.sessions {
            if let Some(volume) = out.volume {
                self.backend.send(Command::SetSessionVolume {
                    device_id: device_id.clone(),
                    id: id.clone(),
                    volume,
                });
            }
            if let Some(muted) = out.mute {
                self.backend.send(Command::SetSessionMute {
                    device_id: device_id.clone(),
                    id: id.clone(),
                    muted,
                });
            }
        }
    }

    /// The pill showing which device an app plays through. Clicking it opens the output picker;
    /// if routing hasn't been turned on yet, it explains and offers to.
    fn device_chip(&mut self, ui: &mut Ui, snap: &Snapshot, app: &AppStrip, width: f32) {
        let name = snap
            .devices
            .iter()
            .find(|d| d.id == app.device_id)
            .map(|d| strip::short_device_name(&d.name))
            .unwrap_or("Output");
        let resp = strip::chip(ui, name, width, self.config.experimental_routing)
            .on_hover_text("Where this app plays. Click to change.");
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(210.0);
            if !self.config.experimental_routing {
                ui.label(RichText::new("Send this app to another device").strong());
                ui.label(
                    RichText::new(
                        "Per-app routing is experimental: it uses an undocumented Windows\n\
                         interface that may not work on every Windows version.",
                    )
                    .size(11.0)
                    .color(theme::TEXT_DIM),
                );
                ui.add_space(4.0);
                if ui.button("Turn on app routing").clicked() {
                    self.config.experimental_routing = true;
                    self.config.save();
                }
                return;
            }
            ui.label(
                RichText::new("PLAY THROUGH")
                    .size(10.0)
                    .color(theme::TEXT_DIM),
            );
            for d in snap.devices.iter().filter(|d| d.flow == Flow::Render) {
                let label = format!(
                    "{}{}",
                    d.name,
                    if d.is_default {
                        "  (default)"
                    } else if crate::engine::io::looks_virtual(&d.name) {
                        "  - virtual cable, feeds the mixer"
                    } else {
                        ""
                    }
                );
                if ui.selectable_label(d.id == app.device_id, label).clicked() {
                    self.backend.send(Command::SetAppDevice {
                        pids: app.pids.clone(),
                        device_id: Some(d.id.clone()),
                    });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Use Windows default").clicked() {
                self.backend.send(Command::SetAppDevice {
                    pids: app.pids.clone(),
                    device_id: None,
                });
                ui.close();
            }
        });
    }

    // --- chrome --------------------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let pro = self.view() == View::Console;
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("wavemix").size(16.0).strong());
            if self.demo && pro {
                ui.label(RichText::new("demo").size(11.0).color(theme::TEXT_DIM))
                    .on_hover_text("Showing fake devices and apps");
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ctx = ui.ctx().clone();
                if ui
                    .selectable_label(self.show_settings, "Settings")
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                if pro {
                    self.presets_menu(ui, snap);
                }
                ui.add_space(6.0);
                // Reads left to right as: Simple | Pro.
                for (view, label, tip) in [
                    (
                        View::Console,
                        "Pro",
                        "Mixing console: inputs, buses, EQ, gate and compressor",
                    ),
                    (
                        View::Compact,
                        "Simple",
                        "A small volume control, like the Windows volume popup",
                    ),
                ] {
                    let selected = self.config.view == view;
                    if ui
                        .selectable_label(selected, label)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.set_view(&ctx, view);
                    }
                }
            });
        });
    }

    /// Pro mode's per-app list: send a program to a cable here to bring it into the mixer.
    fn apps_side_panel(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui.label(
            RichText::new("APPS")
                .size(11.0)
                .strong()
                .color(theme::TEXT_DIM),
        );
        ui.label(
            RichText::new(
                "Use an app's pill to play it through a virtual cable; that cable's input strip then carries it.",
            )
            .size(11.0)
            .color(theme::TEXT_DIM),
        );
        ui.add_space(6.0);
        self.app_list(ui, snap, true);
    }
}

impl eframe::App for MixerApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.route_housekeeping();
        #[cfg(windows)]
        self.handle_shell(ctx, frame);
        #[cfg(not(windows))]
        let _ = (ctx, frame);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Leave Windows playing to the speakers it had, not to a cable nobody is listening to.
        if self.config.restore_playback.is_some() {
            self.unroute_windows_audio();
            self.config.save();
            // The backend thread applies the change; give it a moment before the process ends.
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let snap = self.backend.snapshot();
        egui::Frame::new()
            .fill(theme::BG)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.top_bar(ui, &snap);
                ui.add_space(8.0);
                if self.offer_switch() {
                    self.switch_banner(ui);
                }
                match self.view() {
                    View::Console if self.config.pro_apps_panel => {
                        let avail = ui.available_size();
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = SIDE_PANEL_GAP;
                            let main = vec2(avail.x - SIDE_PANEL_W - SIDE_PANEL_GAP, avail.y);
                            ui.allocate_ui_with_layout(main, Layout::top_down(Align::Min), |ui| {
                                self.console(ui)
                            });
                            let side = vec2(SIDE_PANEL_W, avail.y);
                            ui.allocate_ui_with_layout(side, Layout::top_down(Align::Min), |ui| {
                                self.apps_side_panel(ui, &snap)
                            });
                        });
                    }
                    View::Console => self.console(ui),
                    View::Compact if self.switch.open => self.switch_inline(ui),
                    View::Compact if self.show_settings => self.settings_inline(ui),
                    View::Compact => self.compact(ui, &snap),
                }
            });
        // Esc closes the taskbar flyout, as it does any popup.
        #[cfg(windows)]
        if self.mode == Mode::Flyout && ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.request(ui.ctx(), shell::Request::Hide);
        }
        self.settings_window(ui.ctx());
        self.switch_window(ui.ctx());
        self.flush_engine_config(ui.ctx());
        if self.view() == View::Console {
            // Meters move constantly.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
}

/// Unique id for a widget from a few identifying values.
fn wid(parts: impl std::hash::Hash + std::fmt::Debug) -> Id {
    Id::unique(egui::IdSalt::new(parts))
}
