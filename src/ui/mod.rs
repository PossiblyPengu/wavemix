//! The mixer window. Two views of the same state: the console (every device and every app as a
//! fader strip, Voicemeeter-style) and the compact list (one device at a time, EarTrumpet-style).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui::{self, vec2, Align, Id, Layout, RichText, Sense, TextureHandle, Ui};

use crate::backend::Handle;
use crate::config::{Config, View};
use crate::model::{group_sessions, AppStrip, Command, Device, Flow, IconData, Snapshot};

mod settings;
#[cfg(windows)]
mod shell;
mod strip;
mod theme;

use strip::{Strip, StripOutput};

const CONSOLE_SIZE: [f32; 2] = [1100.0, 480.0];
const COMPACT_SIZE: [f32; 2] = [380.0, 560.0];

pub struct MixerApp {
    backend: Handle,
    config: Config,
    demo: bool,
    /// Device whose apps the compact view shows; `None` follows the default playback device.
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
            config,
            demo,
            compact_device: None,
            mode: Mode::Window,
            icons: RefCell::default(),
            show_settings: false,
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

    pub fn initial_size(view: View) -> [f32; 2] {
        match view {
            View::Console => CONSOLE_SIZE,
            View::Compact => COMPACT_SIZE,
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

    fn top_bar(&mut self, ui: &mut Ui, snap: &Snapshot) {
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("wavemix").size(16.0).strong());
            if self.demo {
                ui.label(RichText::new("demo data").size(11.0).color(theme::TEXT_DIM));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ctx = ui.ctx().clone();
                if self.mode == Mode::Flyout {
                    if ui.button("Open mixer").clicked() {
                        #[cfg(windows)]
                        self.show_window(&ctx);
                    }
                    return;
                }
                for (view, label) in [(View::Compact, "Compact"), (View::Console, "Console")] {
                    let selected = self.config.view == view;
                    if ui.selectable_label(selected, label).clicked() {
                        self.set_view(&ctx, view);
                    }
                }
                ui.add_space(8.0);
                if ui
                    .selectable_label(self.show_settings, "Settings")
                    .clicked()
                {
                    self.show_settings = !self.show_settings;
                }
                self.presets_menu(ui, snap);
            });
        });
    }

    fn console(&self, ui: &mut Ui, snap: &Snapshot) {
        egui::ScrollArea::horizontal()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for d in snap.devices.iter().filter(|d| d.flow == Flow::Render) {
                        self.device_group(ui, snap, d);
                    }
                    for d in snap.devices.iter().filter(|d| d.flow == Flow::Capture) {
                        self.device_group(ui, snap, d);
                    }
                });
            });
    }

    /// One device: its apps' faders, then the device master on the right.
    fn device_group(&self, ui: &mut Ui, snap: &Snapshot, d: &Device) {
        let apps = if d.flow == Flow::Render {
            group_sessions(&snap.sessions, &d.id)
        } else {
            Vec::new()
        };
        let accent = match d.flow {
            Flow::Render => theme::ACCENT_OUT,
            Flow::Capture => theme::ACCENT_IN,
        };
        egui::Frame::new()
            .fill(theme::PANEL)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(8)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    self.group_header(ui, d);
                    ui.add_space(6.0);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        for app in &apps {
                            self.app_strip(ui, snap, app);
                        }
                        if !apps.is_empty() {
                            ui.separator();
                        }
                        self.device_strip(ui, d, accent);
                    });
                });
            });
        ui.add_space(4.0);
    }

    fn group_header(&self, ui: &mut Ui, d: &Device) {
        let kind = match d.flow {
            Flow::Render => "OUT",
            Flow::Capture => "IN",
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(kind)
                    .size(10.0)
                    .strong()
                    .color(theme::TEXT_DIM),
            );
            let name = strip::truncate(ui, &d.name, 190.0, 13.0);
            ui.label(RichText::new(name).size(13.0).strong())
                .on_hover_text(&d.name);
        });
        // Fixed-height row so strips line up across groups whichever variant shows.
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 22.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                if d.is_default {
                    ui.label(
                        RichText::new("Default device")
                            .size(10.0)
                            .color(theme::ACCENT),
                    );
                } else if ui
                    .small_button("Make default")
                    .on_hover_text("Use this as the default device in Windows")
                    .clicked()
                {
                    self.backend.send(Command::SetDefaultDevice {
                        id: d.id.clone(),
                        flow: d.flow,
                    });
                }
            },
        );
    }

    fn app_strip(&self, ui: &mut Ui, snap: &Snapshot, app: &AppStrip) {
        let icon = self.icon(ui.ctx(), &app.key, &app.icon);
        ui.vertical(|ui| {
            let out = Strip {
                id: Id::unique(egui::IdSalt::new(("app", &app.device_id, &app.key))),
                title: &app.name,
                subtitle: if app.active { "playing" } else { "idle" },
                volume: app.volume,
                muted: app.muted,
                peak: app.peak,
                accent: theme::ACCENT,
                icon: icon.as_ref(),
                idle: !app.active,
            }
            .show(ui);
            self.send_app(app, out);
            if self.config.experimental_routing {
                self.route_menu(ui, snap, app);
            }
        });
    }

    /// "Output" menu: pin this app to a playback device, or hand it back to the Windows default.
    fn route_menu(&self, ui: &mut Ui, snap: &Snapshot, app: &AppStrip) {
        ui.menu_button("Output", |ui| {
            for d in snap.devices.iter().filter(|d| d.flow == Flow::Render) {
                if ui
                    .selectable_label(d.id == app.device_id, &d.name)
                    .clicked()
                {
                    self.backend.send(Command::SetAppDevice {
                        pids: app.pids.clone(),
                        device_id: Some(d.id.clone()),
                    });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Windows default").clicked() {
                self.backend.send(Command::SetAppDevice {
                    pids: app.pids.clone(),
                    device_id: None,
                });
                ui.close();
            }
        });
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

    fn device_strip(&self, ui: &mut Ui, d: &Device, accent: egui::Color32) {
        let out = Strip {
            id: Id::unique(egui::IdSalt::new(("device", &d.id))),
            title: "Master",
            subtitle: if d.flow == Flow::Render {
                "output"
            } else {
                "input"
            },
            volume: d.volume,
            muted: d.muted,
            peak: d.peak,
            accent,
            icon: None,
            idle: false,
        }
        .show(ui);
        if let Some(volume) = out.volume {
            self.backend.send(Command::SetDeviceVolume {
                id: d.id.clone(),
                volume,
            });
        }
        if let Some(muted) = out.mute {
            self.backend.send(Command::SetDeviceMute {
                id: d.id.clone(),
                muted,
            });
        }
    }

    fn compact(&mut self, ui: &mut Ui, snap: &Snapshot) {
        let outputs: Vec<&Device> = snap
            .devices
            .iter()
            .filter(|d| d.flow == Flow::Render)
            .collect();
        let selected = self
            .compact_device
            .as_ref()
            .and_then(|id| outputs.iter().find(|d| &d.id == id))
            .or_else(|| outputs.iter().find(|d| d.is_default))
            .or(outputs.first())
            .copied();
        let Some(dev) = selected else {
            ui.add_space(20.0);
            ui.label(RichText::new("No playback devices found").color(theme::TEXT_DIM));
            return;
        };

        ui.add_space(4.0);
        egui::ComboBox::from_id_salt("compact-device")
            .width(ui.available_width())
            .selected_text(strip::truncate(
                ui,
                &dev.name,
                ui.available_width() - 40.0,
                13.0,
            ))
            .show_ui(ui, |ui| {
                for d in &outputs {
                    let label = if d.is_default {
                        format!("{}  (default)", d.name)
                    } else {
                        d.name.clone()
                    };
                    if ui.selectable_label(d.id == dev.id, label).clicked() {
                        self.compact_device = Some(d.id.clone());
                    }
                }
            });
        if !dev.is_default && ui.small_button("Make this the default device").clicked() {
            self.backend.send(Command::SetDefaultDevice {
                id: dev.id.clone(),
                flow: dev.flow,
            });
        }
        ui.add_space(8.0);
        self.compact_row(
            ui,
            snap,
            Row {
                id: Id::unique(egui::IdSalt::new(("cdev", &dev.id))),
                name: "Device volume",
                icon: None,
                volume: dev.volume,
                muted: dev.muted,
                peak: dev.peak,
                accent: theme::ACCENT_OUT,
                lit: true,
                route: None,
            },
            |s, v| match v {
                RowEdit::Volume(volume) => s.backend.send(Command::SetDeviceVolume {
                    id: dev.id.clone(),
                    volume,
                }),
                RowEdit::Mute(muted) => s.backend.send(Command::SetDeviceMute {
                    id: dev.id.clone(),
                    muted,
                }),
            },
        );
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);
        let apps = group_sessions(&snap.sessions, &dev.id);
        if apps.is_empty() {
            ui.label(RichText::new("No apps are using this device").color(theme::TEXT_DIM));
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for app in &apps {
                    let icon = self.icon(ui.ctx(), &app.key, &app.icon);
                    self.compact_row(
                        ui,
                        snap,
                        Row {
                            id: Id::unique(egui::IdSalt::new(("capp", &app.device_id, &app.key))),
                            name: &app.name,
                            icon,
                            volume: app.volume,
                            muted: app.muted,
                            peak: app.peak,
                            accent: theme::ACCENT,
                            lit: app.active,
                            route: Some(app),
                        },
                        |s, v| {
                            let out = match v {
                                RowEdit::Volume(x) => StripOutput {
                                    volume: Some(x),
                                    mute: None,
                                },
                                RowEdit::Mute(m) => StripOutput {
                                    volume: None,
                                    mute: Some(m),
                                },
                            };
                            s.send_app(app, out);
                        },
                    );
                    ui.add_space(4.0);
                }
            });
    }

    /// A single EarTrumpet-style row: name, mute button, horizontal slider, percent.
    fn compact_row(
        &self,
        ui: &mut Ui,
        snap: &Snapshot,
        row: Row,
        mut edit: impl FnMut(&Self, RowEdit),
    ) {
        let Row {
            id,
            name,
            icon,
            volume,
            muted,
            peak,
            accent,
            lit,
            route,
        } = row;
        egui::Frame::new()
            .fill(theme::PANEL)
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let color = if lit { theme::TEXT } else { theme::TEXT_DIM };
                ui.horizontal(|ui| {
                    if let Some(tex) = &icon {
                        ui.add(egui::Image::new(tex).fit_to_exact_size(vec2(18.0, 18.0)));
                    }
                    ui.label(RichText::new(name).size(13.0).color(color));
                    if let (Some(app), true) = (route, self.config.experimental_routing) {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            self.route_menu(ui, snap, app);
                        });
                    }
                });
                ui.horizontal(|ui| {
                    let (mute_txt, mute_col) = if muted {
                        ("M", theme::MUTE)
                    } else {
                        ("M", theme::TEXT_DIM)
                    };
                    let mute_btn = egui::Button::new(RichText::new(mute_txt).color(mute_col))
                        .frame(false)
                        .min_size(vec2(24.0, 24.0));
                    if ui.add(mute_btn).on_hover_text("Mute").clicked() {
                        edit(self, RowEdit::Mute(!muted));
                    }
                    let pct_w = 36.0;
                    let slider_w = (ui.available_width() - pct_w - 8.0).max(60.0);
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(slider_w, 24.0), Sense::click_and_drag());
                    let mut v = volume;
                    if let Some(p) = resp.interact_pointer_pos() {
                        if resp.dragged() || resp.is_pointer_button_down_on() {
                            v = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        }
                    }
                    if resp.hovered() {
                        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                        if scroll != 0.0 {
                            v = (v + scroll.signum() * 0.02).clamp(0.0, 1.0);
                        }
                    }
                    if (v - volume).abs() > f32::EPSILON {
                        edit(self, RowEdit::Volume(v));
                    }
                    paint_h_slider(ui, rect, id, v, muted, peak, accent);
                    ui.label(
                        RichText::new(format!("{}", crate::model::percent(v)))
                            .size(13.0)
                            .monospace()
                            .color(theme::TEXT),
                    );
                });
            });
    }
}

/// What a compact row shows.
struct Row<'a> {
    id: Id,
    name: &'a str,
    icon: Option<TextureHandle>,
    volume: f32,
    muted: bool,
    peak: f32,
    accent: egui::Color32,
    lit: bool,
    /// Present for app rows, to offer the output-routing menu.
    route: Option<&'a AppStrip>,
}

enum RowEdit {
    Volume(f32),
    Mute(bool),
}

/// Horizontal slider with a thin live meter underneath, drawn by hand so the meter can share the
/// track.
fn paint_h_slider(
    ui: &Ui,
    rect: egui::Rect,
    id: Id,
    v: f32,
    muted: bool,
    peak: f32,
    accent: egui::Color32,
) {
    use egui::{pos2, CornerRadius, Rect};
    let p = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 6.0));
    p.rect_filled(track, CornerRadius::same(3), theme::METER_BG);
    let color = if muted { theme::TEXT_DIM } else { accent };
    let fill = Rect::from_min_max(
        track.min,
        pos2(track.left() + track.width() * v, track.bottom()),
    );
    p.rect_filled(fill, CornerRadius::same(3), color);

    // Live level: a thin bar under the track, smoothed a little so it doesn't flicker.
    let level = if muted {
        0.0
    } else {
        crate::model::db_to_meter(crate::model::amp_to_db(peak))
    };
    let smooth_id = id.with("lvl");
    let prev = ui.data(|d| d.get_temp::<f32>(smooth_id)).unwrap_or(0.0);
    let shown = if level > prev {
        level
    } else {
        prev - (prev - level) * 0.25
    };
    ui.data_mut(|d| d.insert_temp(smooth_id, shown));
    let lvl_rect = Rect::from_min_size(
        pos2(track.left(), track.bottom() + 3.0),
        vec2(track.width() * shown, 2.0),
    );
    p.rect_filled(
        lvl_rect,
        CornerRadius::ZERO,
        theme::METER_GREEN.linear_multiply(0.8),
    );

    let knob = pos2(track.left() + track.width() * v, track.center().y);
    p.circle_filled(knob, 8.0, theme::TEXT);
    p.circle_stroke(knob, 8.0, egui::Stroke::new(2.0, color));
}

impl MixerApp {
    /// What to draw: the flyout is always the compact list, whatever the window view is.
    fn view(&self) -> View {
        match self.mode {
            Mode::Flyout => View::Compact,
            Mode::Window => self.config.view,
        }
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
                    View::Console => self.console(ui, &snap),
                    View::Compact => self.compact(ui, &snap),
                }
            });
        self.settings_window(ui.ctx());
    }
}
