//! Windows shell integration: the tray icon, Simple mode as a flyout that rises out of the
//! taskbar, Pro mode as a normal window, hotkeys, and the on-screen volume display.
//!
//! One native window serves both modes. As the flyout it is borderless, always on top, has no
//! taskbar button and slides in from the taskbar edge next to the tray icon; as Pro it is a
//! normal decorated window.

use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::platform::windows::{CornerPreference, WindowExtWindows};
use winit::window::{Window, WindowLevel};

use super::{theme, MixerApp, Mode, SIMPLE_SIZE};
use crate::backend::Handle;
use crate::config::{Config, OsdPosition, View};
use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::model::{Command, Flow};
use crate::osd::layout::{flyout_plan, taskbar_edge, Area, Edge};
use crate::osd::{MonitorInfo, Osd, OsdEvent, OsdSettings};
use crate::tray::{Tray, TrayAction};

/// A click on the tray icon steals focus, which dismisses the flyout, and the click's release then
/// arrives as a "toggle". Releases this soon after a dismissal are that same click.
const DISMISS_GRACE: Duration = Duration::from_millis(350);
/// Focus can take a moment to arrive after showing; don't dismiss before it has.
const FOCUS_GRACE: Duration = Duration::from_millis(400);
/// How the flyout sits against the taskbar and how far it travels while sliding in (96-dpi px).
const FLYOUT_MARGIN: f32 = 12.0;
const SLIDE_DISTANCE: f32 = 56.0;
const SLIDE_SECS: f32 = 0.20;
/// How often the tray icon's position is handed to the on-screen display.
const TRAY_SYNC: Duration = Duration::from_millis(500);
/// If the shell hasn't reported where the tray icon is by now, open the flyout in the corner.
const STARTUP_WAIT: Duration = Duration::from_millis(1500);

/// Things the UI asks the shell to do. They run from `logic`, which is where the native window is
/// reachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Make the window match the current mode (flyout for Simple, window for Pro) and show it.
    Apply,
    Hide,
}

struct Slide {
    started: Instant,
    from: (i32, i32),
    to: (i32, i32),
}

/// Where the flyout is, so it can resize in place when its content grows or shrinks.
struct FlyoutGeom {
    work: Area,
    edge: Edge,
    anchor: Option<(i32, i32)>,
    scale: f32,
    w_px: i32,
    h_px: i32,
}

pub struct ShellState {
    tray: Option<Tray>,
    visible: bool,
    pub(super) quitting: bool,
    shown_at: Instant,
    hidden_at: Option<Instant>,
    focused_once: bool,
    hotkeys: Hotkeys,
    osd: Option<Osd>,
    osd_events: Receiver<OsdEvent>,
    /// Problems from the last hotkey or startup change, shown in settings.
    pub hk_status: Vec<String>,
    pending: Option<Request>,
    slide: Option<Slide>,
    flyout: Option<FlyoutGeom>,
    /// Open the flyout once the tray icon's position is known (first launch in Simple mode).
    startup_flyout: Option<Instant>,
    last_tray_sync: Instant,
}

impl ShellState {
    /// `start_hidden`: the native window was created hidden (it is the flyout, or a login launch).
    pub fn new(
        ctx: &egui::Context,
        start_hidden: bool,
        background: bool,
        backend: Handle,
        config: &Config,
    ) -> Self {
        let tray = Tray::new(ctx);
        // Without a tray there'd be no way back to a hidden window, so show it.
        let visible = !start_hidden || tray.is_none();
        if visible && start_hidden {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
        let startup_flyout =
            (start_hidden && !background && tray.is_some() && config.view == View::Compact)
                .then(Instant::now);
        let (tx, osd_events) = mpsc::channel();
        let c = ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || c.request_repaint());
        Self {
            tray,
            visible,
            quitting: false,
            shown_at: Instant::now(),
            hidden_at: None,
            focused_once: false,
            hotkeys: Hotkeys::new(ctx),
            osd: Osd::new(backend, osd_settings(config), tx, repaint),
            osd_events,
            hk_status: Vec::new(),
            pending: None,
            slide: None,
            flyout: None,
            startup_flyout,
            last_tray_sync: Instant::now() - TRAY_SYNC,
        }
    }

    pub fn has_tray(&self) -> bool {
        self.tray.is_some()
    }
}

pub fn osd_settings(config: &Config) -> OsdSettings {
    OsdSettings {
        enabled: config.osd,
        vertical: config.osd_vertical,
        opacity_pct: config.osd_opacity,
        position: config.osd_position,
        custom: (config.osd_x, config.osd_y),
        timeout_ms: config.osd_timeout_ms,
        accent_system: config.osd_accent_system,
        volume_step: config.volume_step,
        hide_windows_popup: config.hide_windows_osd,
        dark: theme::is_dark(),
        device_names: config
            .device_names
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

impl MixerApp {
    /// Push changed OSD settings to the display thread.
    pub(super) fn sync_osd(&self) {
        if let Some(osd) = &self.shell.osd {
            osd.set_settings(osd_settings(&self.config));
        }
    }

    /// Ask the shell to do something on the next `logic` pass.
    pub(super) fn request(&mut self, ctx: &egui::Context, request: Request) {
        self.shell.pending = Some(request);
        ctx.request_repaint();
    }

    pub(super) fn handle_shell(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let win: Option<Arc<Window>> = frame.winit_window().cloned();

        // Keep the on-screen display informed: where the tray icon is, and whether our own
        // window is up (it shows volume changes itself, so the display stays out of the way).
        if let Some(osd) = &self.shell.osd {
            osd.set_suppressed(self.shell.visible);
            if self.shell.last_tray_sync.elapsed() >= TRAY_SYNC {
                osd.set_tray_point(self.shell.tray.as_ref().and_then(|t| t.center()));
                self.shell.last_tray_sync = Instant::now();
            }
        }

        // First launch in Simple mode: come up from the taskbar once we know where the icon is.
        if let Some(since) = self.shell.startup_flyout {
            let located = self.shell.tray.as_ref().and_then(|t| t.center()).is_some();
            if located || since.elapsed() >= STARTUP_WAIT {
                self.shell.startup_flyout = None;
                self.apply_mode(ctx, win.as_deref());
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }

        let actions = self
            .shell
            .tray
            .as_ref()
            .map(|t| t.poll())
            .unwrap_or_default();
        // A second copy of the app was started: show this one.
        if crate::instance::take_open_request() {
            self.apply_mode(ctx, win.as_deref());
        }
        for action in actions {
            match action {
                TrayAction::Toggle => {
                    let just_dismissed = self
                        .shell
                        .hidden_at
                        .is_some_and(|t| t.elapsed() < DISMISS_GRACE);
                    if self.shell.visible {
                        self.hide(ctx);
                    } else if !just_dismissed {
                        self.apply_mode(ctx, win.as_deref());
                    }
                }
                TrayAction::Open => self.apply_mode(ctx, win.as_deref()),
                TrayAction::SetView(view) => {
                    self.config.view = view;
                    self.config.save();
                    self.apply_mode(ctx, win.as_deref());
                }
                TrayAction::Quit => {
                    self.shell.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }

        while let Ok(event) = self.shell.osd_events.try_recv() {
            match event {
                // The bar was clicked: bring the app up, as the original brought up Voicemeeter.
                OsdEvent::Open => self.apply_mode(ctx, win.as_deref()),
                OsdEvent::Moved(x, y) => {
                    self.config.osd_position = OsdPosition::Custom;
                    self.config.osd_x = x;
                    self.config.osd_y = y;
                    self.config.save();
                    self.sync_osd();
                }
            }
        }

        for action in self.shell.hotkeys.poll() {
            self.run_hotkey(action);
        }

        match self.shell.pending.take() {
            Some(Request::Apply) => self.apply_mode(ctx, win.as_deref()),
            Some(Request::Hide) => self.hide(ctx),
            None => {}
        }

        // The close button hides to the tray, but only if there is a tray to come back from.
        let close = ctx.input(|i| i.viewport().close_requested());
        if close && !self.shell.quitting && self.shell.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide(ctx);
        }

        if self.mode == Mode::Flyout && self.shell.visible {
            // Dismiss the flyout when the user clicks elsewhere.
            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
            if focused {
                self.shell.focused_once = true;
            } else if self.shell.focused_once && self.shell.shown_at.elapsed() > FOCUS_GRACE {
                self.hide(ctx);
            }
            if let Some(win) = &win {
                self.animate_flyout(ctx, win);
            }
        }
    }

    /// Make the window match the current mode and show it: the flyout for Simple, a normal window
    /// for Pro. Without a tray there's nothing to rise from, so Simple is just a small window.
    fn apply_mode(&mut self, ctx: &egui::Context, win: Option<&Window>) {
        let flyout = self.config.view == View::Compact && self.shell.tray.is_some();
        match (flyout, win) {
            (true, Some(win)) => self.show_flyout(ctx, win),
            (_, win) => self.show_window(ctx, win),
        }
    }

    fn run_hotkey(&self, action: HotkeyAction) {
        let snap = self.backend.snapshot();
        let Some(d) = snap
            .devices
            .iter()
            .find(|d| d.flow == Flow::Render && d.is_default)
        else {
            return;
        };
        let step = self.config.volume_step as f32 / 100.0;
        let id = d.id.clone();
        // Even at maximum volume, where nothing changes, a keypress should show where you are.
        if let Some(osd) = &self.shell.osd {
            osd.poke();
        }
        self.backend.send(match action {
            HotkeyAction::VolumeUp => Command::SetDeviceVolume {
                id,
                volume: (d.volume + step).min(1.0),
            },
            HotkeyAction::VolumeDown => Command::SetDeviceVolume {
                id,
                volume: (d.volume - step).max(0.0),
            },
            HotkeyAction::ToggleMute => Command::SetDeviceMute {
                id,
                muted: !d.muted,
            },
        });
    }

    /// Register the hotkeys in `config`.
    pub(super) fn apply_hotkeys(&mut self) {
        self.shell.hk_status = self.shell.hotkeys.apply(&self.config.hotkeys);
    }

    /// Settings' Apply button: take the edited text, save it, and register.
    pub(super) fn apply_hotkeys_from_edit(&mut self) {
        self.config.hotkeys = self.hk_edit.clone();
        self.config.save();
        self.apply_hotkeys();
    }

    fn hide(&mut self, ctx: &egui::Context) {
        self.shell.visible = false;
        self.shell.hidden_at = Some(Instant::now());
        self.shell.slide = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn reveal(&mut self, ctx: &egui::Context) {
        self.shell.visible = true;
        self.shell.shown_at = Instant::now();
        self.shell.focused_once = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Pro mode (or Simple without a tray): a normal, decorated window with a taskbar button.
    fn show_window(&mut self, ctx: &egui::Context, win: Option<&Window>) {
        let was_flyout = self.mode == Mode::Flyout;
        self.mode = Mode::Window;
        self.shell.flyout = None;
        self.shell.slide = None;
        let [w, h] = Self::size_for(&self.config);
        if let Some(win) = win {
            win.set_skip_taskbar(false);
            win.set_corner_preference(CornerPreference::Default);
            win.set_undecorated_shadow(false);
            win.set_decorations(true);
            win.set_window_level(WindowLevel::Normal);
            let _ = win.request_inner_size(winit::dpi::LogicalSize::new(w as f64, h as f64));
            // Coming from the flyout the window is parked by the taskbar; bring it to the middle
            // of the screen. Otherwise leave it where the user put it.
            if was_flyout || !self.shell.visible {
                if let Some(m) = win.current_monitor().or_else(|| win.primary_monitor()) {
                    let scale = m.scale_factor();
                    let (mx, my) = (m.position().x, m.position().y);
                    let (mw, mh) = (m.size().width as f64, m.size().height as f64);
                    let x = mx as f64 + ((mw - w as f64 * scale) / 2.0).max(0.0);
                    let y = my as f64 + ((mh - h as f64 * scale) / 2.0).max(0.0);
                    win.set_outer_position(PhysicalPosition::new(x as i32, y as i32));
                }
            }
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
        }
        self.reveal(ctx);
    }

    /// Simple mode: borderless, no taskbar button, rising out of the taskbar by the tray icon.
    fn show_flyout(&mut self, ctx: &egui::Context, win: &Window) {
        self.mode = Mode::Flyout;
        // Replay the content's fade-in each time the flyout rises.
        self.page_since = Instant::now();
        let anchor = self.shell.tray.as_ref().and_then(|t| t.center());
        let mon = match anchor {
            Some((x, y)) => MonitorInfo::at(x, y),
            None => MonitorInfo::primary(),
        };
        let edge = taskbar_edge(mon.full, mon.work, anchor);
        let scale = mon.dpi as f32 / 96.0;
        let w_px = (SIMPLE_SIZE[0] * scale).round() as i32;
        let h_px = (self.flyout_height_points(&mon) * scale).round() as i32;
        let margin = (FLYOUT_MARGIN * scale).round() as i32;
        let slide = (SLIDE_DISTANCE * scale).round() as i32;
        let plan = flyout_plan(edge, mon.work, anchor, (w_px, h_px), margin, slide);

        win.set_skip_taskbar(true);
        win.set_corner_preference(CornerPreference::Round);
        win.set_undecorated_shadow(true);
        win.set_decorations(false);
        win.set_window_level(WindowLevel::AlwaysOnTop);
        let _ = win.request_inner_size(PhysicalSize::new(w_px as u32, h_px as u32));
        let from = (plan.x + plan.slide.0, plan.y + plan.slide.1);
        win.set_outer_position(PhysicalPosition::new(from.0, from.1));
        self.shell.slide = Some(Slide {
            started: Instant::now(),
            from,
            to: (plan.x, plan.y),
        });
        self.shell.flyout = Some(FlyoutGeom {
            work: mon.work,
            edge,
            anchor,
            scale,
            w_px,
            h_px,
        });
        self.reveal(ctx);
    }

    /// The flyout's height in points: as tall as its content, within sensible limits.
    fn flyout_height_points(&self, mon: &MonitorInfo) -> f32 {
        let scale = mon.dpi as f32 / 96.0;
        let room = (mon.work.bottom - mon.work.top) as f32 / scale - 2.0 * FLYOUT_MARGIN;
        self.simple_height
            .get()
            .clamp(super::SIMPLE_MIN_H, super::SIMPLE_MAX_H.min(room))
    }

    /// Slide the flyout in, then keep its height matched to its content.
    fn animate_flyout(&mut self, ctx: &egui::Context, win: &Window) {
        if let Some(s) = &self.shell.slide {
            let t = s.started.elapsed().as_secs_f32() / SLIDE_SECS;
            let e = ease_out_cubic(t);
            let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * e).round() as i32;
            win.set_outer_position(PhysicalPosition::new(
                lerp(s.from.0, s.to.0),
                lerp(s.from.1, s.to.1),
            ));
            if t >= 1.0 {
                self.shell.slide = None;
            }
            ctx.request_repaint();
            return;
        }
        let Some(g) = &self.shell.flyout else { return };
        let mon_dpi = (g.scale * 96.0) as i32;
        let want = (self.flyout_height_points(&MonitorInfo {
            full: g.work,
            work: g.work,
            dpi: mon_dpi,
        }) * g.scale)
            .round() as i32;
        if (want - g.h_px).abs() > 4 {
            let margin = (FLYOUT_MARGIN * g.scale).round() as i32;
            let plan = flyout_plan(g.edge, g.work, g.anchor, (g.w_px, want), margin, 0);
            let _ = win.request_inner_size(PhysicalSize::new(g.w_px as u32, want as u32));
            win.set_outer_position(PhysicalPosition::new(plan.x, plan.y));
            if let Some(g) = &mut self.shell.flyout {
                g.h_px = want;
            }
        }
    }
}
