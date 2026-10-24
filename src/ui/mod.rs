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
mod theme;

use strip::StripOutput;

const CONSOLE_SIZE: [f32; 2] = [1360.0, 840.0];
const COMPACT_SIZE: [f32; 2] = [390.0, 660.0];

pub struct MixerApp {
    backend: Handle,
    engine: Engine,
    /// Strip whose processing editor (EQ / gate / compressor) is open, if any.
    editing: Option<(usize, console::EditorTab)>,
    /// When settings last changed and haven't been written to disk yet.
    engine_dirty_at: Option<std::time::Instant>,
    config: Config,
    demo: bool,
    /// Device the compact view's top card controls; `None` follows the default playback device.
    compact_device: Option<String>,
    mode: Mode,
    /// App icons uploaded as textures, by app key. Filled lazily while drawing.
    icons: RefCell<HashMap<String, TextureHandle>>,
    show_settings: bool,
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
        #[cfg(windows)]
        let shell = shell::ShellState::new(&cc.egui_ctx, background, backend.clone(), &config);
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
            config,
            demo,
            compact_device: None,
            mode: Mode::Window,
            icons: RefCell::default(),
            // `--settings` opens the settings window on launch.
            show_settings: std::env::args().any(|a| a == "--settings"),
            preset_name: String::new(),
            #[cfg(windows)]
            hk_edit,
            #[cfg(windows)]
            shell,
        };
        #[cfg(windows)]
        app.apply_hotkeys();
        app
    }

    pub fn initial_size(view: View) -> [f32; 2] {
        match view {
            View::Console => CONSOLE_SIZE,
            View::Compact => COMPACT_SIZE,
        }
    }

    /// What to draw: the flyout is always the compact list, whatever the window view is.
    fn view(&self) -> View {
        match self.mode {
            Mode::Flyout => View::Compact,
            Mode::Window => self.config.view,
        }
    }

    fn set_view(&mut self, ctx: &egui::Context, view: View) {
        if self.config.view != view {
            self.config.view = view;
            self.config.save();
            let [w, h] = Self::initial_size(view);
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(w, h)));
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
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("wavemix").size(16.0).strong());
            if self.demo && self.view() == View::Console {
                ui.label(RichText::new("demo").size(11.0).color(theme::TEXT_DIM))
                    .on_hover_text("Showing fake devices and apps");
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ctx = ui.ctx().clone();
                if self.mode == Mode::Flyout {
                    if ui.button("Open full mixer").clicked() {
                        #[cfg(windows)]
                        self.show_window(&ctx);
                    }
                    return;
                }
                if ui
                    .selectable_label(self.show_settings, "Settings")
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                self.presets_menu(ui, snap);
                ui.add_space(6.0);
                // View switch: reads left-to-right as Mixer | Compact.
                for (view, label, tip) in [
                    (
                        View::Compact,
                        "Apps",
                        "Per-app volume and output, like the Windows volume popup",
                    ),
                    (
                        View::Console,
                        "Mixer",
                        "The mixing console: inputs, buses, EQ and dynamics",
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
}

impl eframe::App for MixerApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(windows)]
        self.handle_shell(ctx);
        #[cfg(not(windows))]
        let _ = ctx;
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
                match self.view() {
                    View::Console => self.console(ui),
                    View::Compact => self.compact(ui, &snap),
                }
            });
        self.settings_window(ui.ctx());
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
