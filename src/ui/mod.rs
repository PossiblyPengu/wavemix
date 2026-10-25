//! The mixer window. Two views of the same state: the console (apps on the left, outputs and
//! microphones on the right, as in Voicemeeter) and the compact list (EarTrumpet-style).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, vec2, Align, Id, Layout, RichText, TextureHandle, Ui};

use crate::backend::Handle;
use crate::config::{Config, View};
use crate::engine::runtime::Engine;
use crate::model::{group_apps, AppStrip, Command, Flow, IconData, Snapshot};

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
const PRO_SIZE: [f32; 2] = [1424.0, 860.0];
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
    /// Names saved under Windows device names have been moved to device ids.
    names_migrated: bool,
    /// The theme setting changed; apply it on the next frame.
    theme_dirty: bool,
    /// The window frame (title bar) theme last sent to the OS.
    frame_dark: Option<bool>,
    /// When the theme was last compared with the setting (and Windows).
    theme_checked: std::time::Instant,
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
    /// Strip or bus whose name is being typed over, and the text so far.
    renaming: Option<(Rename, usize)>,
    rename_buf: String,
    /// Hotkey text being edited in settings; applied with the Apply button.
    #[cfg(windows)]
    hk_edit: crate::config::Hotkeys,
    #[cfg(windows)]
    shell: shell::ShellState,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rename {
    Strip,
    Bus,
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
        theme::set_dark(theme::wants_dark(config.theme));
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
            names_migrated: false,
            theme_dirty: false,
            frame_dark: None,
            theme_checked: std::time::Instant::now(),
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
            renaming: None,
            rename_buf: String::new(),
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
        let default = snap
            .devices
            .iter()
            .find(|d| d.flow == Flow::Render && d.is_default)
            .map(|d| d.id.as_str());
        let mut apps: Vec<AppStrip> = group_apps(&snap.sessions, default)
            .into_iter()
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
    /// A device's name for display: yours if you gave it one, else Windows' name made short (and
    /// kept distinct from similar devices in `all`).
    fn device_label<'a>(&self, name: &str, all: impl IntoIterator<Item = &'a str>) -> String {
        match self.alias_for_name(name) {
            Some(alias) => alias.to_string(),
            None => strip::label_among(name, all),
        }
    }

    /// The name you gave the device Windows calls `name`. The engine only knows devices by name,
    /// so this finds the matching device (and its id) in Windows' own list.
    fn alias_for_name(&self, name: &str) -> Option<String> {
        let snap = self.backend.snapshot();
        snap.devices
            .iter()
            .find(|d| d.name == name)
            .and_then(|d| self.config.device_alias(&d.id))
            .map(str::to_string)
    }

    /// Keep the palette in step with the setting, and with Windows when set to follow it.
    pub(super) fn follow_theme(&mut self, ctx: &egui::Context) {
        let due = self.theme_checked.elapsed() > std::time::Duration::from_secs(2);
        if !due && !self.theme_dirty {
            return;
        }
        self.theme_checked = std::time::Instant::now();
        self.theme_dirty = false;
        let dark = theme::wants_dark(self.config.theme);
        if self.frame_dark != Some(dark) {
            // Tints the title bar and window frame to match.
            self.frame_dark = Some(dark);
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(if dark {
                egui::SystemTheme::Dark
            } else {
                egui::SystemTheme::Light
            }));
        }
        if dark != theme::is_dark() {
            theme::set_dark(dark);
            theme::apply(ctx);
            self.sync_osd_names();
            ctx.request_repaint();
        }
    }

    /// Tell the volume bar about changed names (Windows only).
    fn sync_osd_names(&self) {
        #[cfg(windows)]
        self.sync_osd();
    }

    /// Re-key names saved by older versions (which used the Windows name) to device ids.
    pub(super) fn migrate_device_names(&mut self) {
        let snap = self.backend.snapshot();
        if snap.devices.is_empty() {
            return;
        }
        let old: Vec<(String, String)> = self
            .config
            .device_names
            .iter()
            .filter(|(k, _)| !snap.devices.iter().any(|d| &d.id == *k))
            .filter(|(k, _)| snap.devices.iter().any(|d| &d.name == *k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if old.is_empty() {
            return;
        }
        for (name, text) in old {
            self.config.device_names.remove(&name);
            for d in snap.devices.iter().filter(|d| d.name == name) {
                self.config.device_names.insert(d.id.clone(), text.clone());
            }
        }
        self.config.save();
        self.sync_osd_names();
    }

    /// A strip's or bus's title. Double-click to type a new name; Enter or clicking away keeps it,
    /// Esc cancels, and an empty name goes back to the default.
    fn title_label(&mut self, ui: &mut Ui, kind: Rename, i: usize, color: Option<egui::Color32>) {
        if self.renaming == Some((kind, i)) {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut self.rename_buf)
                    .desired_width(70.0)
                    .char_limit(24),
            );
            let cancelled = ui.input(|inp| inp.key_pressed(egui::Key::Escape));
            if resp.lost_focus() || cancelled {
                if !cancelled {
                    let names = match kind {
                        Rename::Strip => &mut self.config.strip_names,
                        Rename::Bus => &mut self.config.bus_names,
                    };
                    if names.len() <= i {
                        names.resize(i + 1, String::new());
                    }
                    names[i] = self.rename_buf.trim().to_string();
                    self.config.save();
                }
                self.renaming = None;
            } else if !resp.has_focus() {
                resp.request_focus();
            }
            return;
        }
        let title = match kind {
            Rename::Strip => self.config.strip_title(i),
            Rename::Bus => self.config.bus_title(i),
        };
        let shown = strip::truncate(ui, &title, 78.0, 13.0);
        let mut text = RichText::new(shown).size(13.0).strong();
        if let Some(c) = color {
            text = text.color(c);
        }
        let resp = ui
            .add(egui::Label::new(text).sense(egui::Sense::click()))
            .on_hover_text(format!("{title}\nDouble-click to rename"));
        if resp.double_clicked() {
            self.rename_buf = title;
            self.renaming = Some((kind, i));
        }
    }

    fn device_chip(&mut self, ui: &mut Ui, snap: &Snapshot, app: &AppStrip, width: f32) {
        let name = snap
            .devices
            .iter()
            .find(|d| d.id == app.device_id)
            .map(|d| {
                self.device_label(
                    &d.name,
                    snap.devices
                        .iter()
                        .filter(|o| o.flow == Flow::Render)
                        .map(|o| o.name.as_str()),
                )
            })
            .unwrap_or_else(|| "Output".to_string());
        let resp = strip::chip(ui, &name, width, self.config.experimental_routing)
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
                    .color(theme::TEXT_DIM()),
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
                    .color(theme::TEXT_DIM()),
            );
            for d in snap.devices.iter().filter(|d| d.flow == Flow::Render) {
                let label = format!(
                    "{}{}",
                    match self.config.device_alias(&d.id) {
                        Some(alias) => format!("{alias}  ({})", d.name),
                        None => d.name.clone(),
                    },
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
            let (mark, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
            ui.painter().circle_filled(mark.center(), 8.0, theme::ACCENT());
            ui.painter().circle_filled(mark.center(), 3.0, theme::CARD());
            ui.label(RichText::new("wavemix").size(17.0).strong().color(theme::TEXT()));
            if self.demo && pro {
                ui.label(RichText::new("demo").size(11.0).color(theme::TEXT_DIM()))
                    .on_hover_text("Showing fake devices and apps");
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ctx = ui.ctx().clone();
                if strip::ghost_button(ui, "Settings", self.show_settings).clicked() {
                    self.show_settings = !self.show_settings;
                }
                if pro {
                    self.presets_menu(ui, snap);
                }
                ui.add_space(8.0);
                // Reads left to right as: Simple | Pro.
                let selected = usize::from(pro);
                if let Some(i) = strip::segmented(
                    ui,
                    wid("view-switch"),
                    &[
                        ("Simple", "A small volume control, like the Windows volume popup"),
                        ("Pro", "Mixing console: inputs, buses, EQ, gate and compressor"),
                    ],
                    selected,
                ) {
                    self.set_view(&ctx, if i == 1 { View::Console } else { View::Compact });
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
                .color(theme::TEXT_DIM()),
        );
        ui.label(
            RichText::new(
                "Use an app's pill to play it through a virtual cable; that cable's input strip then carries it.",
            )
            .size(11.0)
            .color(theme::TEXT_DIM()),
        );
        ui.add_space(6.0);
        self.app_list(ui, snap, true);
    }
}

impl eframe::App for MixerApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.route_housekeeping();
        self.follow_theme(ctx);
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
            .fill(theme::BG())
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
