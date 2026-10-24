//! The on-screen volume display, ported from voicemeeter-osd: a never-activating, always-on-top
//! layered window drawn with GDI+. It runs on its own thread with its own message loop, so it keeps
//! animating whether or not the mixer window exists.
//!
//! Behaviour, as in the original:
//!
//! * it pops out of the tray icon with a callout tail pointing back at it (or floats low and
//!   centred, or sits where you dragged it);
//! * it appears whenever any playback device's volume or mute changes, from any cause, and shows
//!   that device;
//! * hover keeps it up, click opens the app, scroll adjusts the volume, middle-click mutes, and
//!   dragging moves it for good;
//! * it can draw in the Windows accent colour, and stays away from fullscreen games.

use std::collections::HashMap;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::Graphics::GdiPlus::{
    GdipGraphicsClear, GdipResetWorldTransform, GdipScaleWorldTransform,
    GdipTranslateWorldTransform, MatrixOrderPrepend,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::gfx::{argb, shade, tint, Canvas, Gdiplus, LayeredSurface, ALIGN_CENTER, ALIGN_NEAR};
use super::layout::{self, has_tail, taskbar_edge, Area, Edge, Place, SHADOW, TAIL_H, TAIL_W};
use super::wheel::WheelSteps;
use crate::backend::Handle;
use crate::config::OsdPosition;
use crate::model::{amp_to_db, db_to_meter, Command, Flow};

const CLASS_NAME: &str = "WavemixOsdClass";
const WM_POKE: u32 = WM_APP + 1;
const FRAME_MS: u32 = 16;
const POLL_MS: u32 = 50;
const TIMER_ANIM: usize = 1;
const TIMER_POLL: usize = 2;

const FADE_IN_SECS: f32 = 0.11;
const FADE_OUT_SECS: f32 = 0.22;
const POP_IN_SECS: f32 = 0.26;
const POP_OUT_SECS: f32 = 0.20;
/// Smallest scale at the start of the pop, so it emerges from the icon rather than from nothing.
const MIN_SCALE: f32 = 0.28;
/// Used when the Windows accent colour isn't wanted or can't be read.
const DEFAULT_ACCENT: (u8, u8, u8) = (0x4c, 0xa8, 0xff);
/// A volume change smaller than this isn't worth showing (it is float noise from the mixer).
const CHANGE_EPSILON: f32 = 0.004;

#[derive(Clone, Debug, PartialEq)]
pub struct OsdSettings {
    pub enabled: bool,
    pub vertical: bool,
    pub opacity_pct: u32,
    pub position: OsdPosition,
    /// Dragged-to position, used when `position` is `Custom`.
    pub custom: (i32, i32),
    pub timeout_ms: u32,
    pub accent_system: bool,
    /// Percentage points per wheel notch.
    pub volume_step: u32,
    /// Hide Windows' own volume popup while ours is up.
    pub hide_windows_popup: bool,
    /// Names you gave devices, by device id.
    pub device_names: HashMap<String, String>,
    /// Draw on a dark card.
    pub dark: bool,
}

/// What the display asks of the app.
#[derive(Clone, Debug)]
pub enum OsdEvent {
    /// The bar was clicked: bring up the app.
    Open,
    /// The bar was dragged somewhere and should stay there.
    Moved(i32, i32),
}

struct Shared {
    settings: Mutex<OsdSettings>,
    /// Screen position of the tray icon's centre, kept fresh by the UI thread.
    tray: Mutex<Option<(i32, i32)>>,
    /// True while the app's own window is up (it shows the change itself).
    suppressed: AtomicBool,
}

/// Handle to the display thread.
pub struct Osd {
    thread_id: u32,
    shared: Arc<Shared>,
}

impl Osd {
    /// Start the display thread. `None` if the window couldn't be created. `repaint` wakes the
    /// app after an event is queued.
    pub fn new(
        backend: Handle,
        settings: OsdSettings,
        events: mpsc::Sender<OsdEvent>,
        repaint: Arc<dyn Fn() + Send + Sync>,
    ) -> Option<Self> {
        DARK.store(settings.dark, Ordering::Relaxed);
        let shared = Arc::new(Shared {
            settings: Mutex::new(settings),
            tray: Mutex::new(None),
            suppressed: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        thread::Builder::new()
            .name("osd".into())
            .spawn(move || unsafe { run(backend, s, events, repaint, tx) })
            .ok()?;
        let thread_id = rx.recv().ok().flatten()?;
        Some(Self { thread_id, shared })
    }

    pub fn set_settings(&self, settings: OsdSettings) {
        DARK.store(settings.dark, Ordering::Relaxed);
        if let Ok(mut s) = self.shared.settings.lock() {
            *s = settings;
        }
    }

    pub fn set_tray_point(&self, point: Option<(i32, i32)>) {
        if let Ok(mut t) = self.shared.tray.lock() {
            *t = point;
        }
    }

    pub fn set_suppressed(&self, suppressed: bool) {
        self.shared.suppressed.store(suppressed, Ordering::Relaxed);
    }

    /// Show the display for the default device (used even when nothing changed, e.g. a hotkey
    /// pressed at maximum volume).
    pub fn poke(&self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_POKE, 0, 0);
        }
    }
}

/// What one frame shows.
struct Frame {
    label: String,
    percent: i32,
    muted: bool,
    meter: f32,
    meter_peak: f32,
    peak_db: f32,
}

struct Drag {
    cursor: (i32, i32),
    window: (i32, i32),
    moved: bool,
}

struct State {
    hwnd: HWND,
    surface: Option<LayeredSurface>,
    surface_key: (i32, bool, bool),
    monitor: MonitorInfo,
    /// Tray icon position captured when this showing started.
    anchor: Option<(i32, i32)>,
    animating: bool,
    hide_at: Option<Instant>,
    last_frame: Instant,
    alpha: f32,
    pop: f32,
    display_pct: f32,
    meter: f32,
    meter_peak: f32,
    /// Device being shown; `None` means the default playback device.
    shown: Option<String>,
    /// Last volume and mute seen per playback device, to notice changes.
    known: HashMap<String, (f32, bool)>,
    drag: Option<Drag>,
    wheel: WheelSteps,
}

unsafe fn run(
    backend: Handle,
    shared: Arc<Shared>,
    events: mpsc::Sender<OsdEvent>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    ready: mpsc::Sender<Option<u32>>,
) {
    let _gdiplus = Gdiplus::start();
    let hinstance = GetModuleHandleW(null_mut());
    let class = wide(CLASS_NAME);
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(DefWindowProcW),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: null_mut(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: null_mut(),
        lpszMenuName: null_mut(),
        lpszClassName: class.as_ptr(),
    };
    RegisterClassW(&wc);
    // Never activated, so it can't take focus from a game. Not click-through: it takes clicks,
    // drags and the wheel.
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class.as_ptr(),
        wide("wavemix OSD").as_ptr(),
        WS_POPUP,
        0,
        0,
        10,
        10,
        null_mut(),
        null_mut(),
        hinstance,
        null_mut(),
    );
    if hwnd.is_null() {
        let _ = ready.send(None);
        return;
    }
    let mut state = State {
        hwnd,
        surface: None,
        surface_key: (0, false, false),
        monitor: MonitorInfo::primary(),
        anchor: None,
        animating: false,
        hide_at: None,
        last_frame: Instant::now(),
        alpha: 0.0,
        pop: 0.0,
        display_pct: 0.0,
        meter: 0.0,
        meter_peak: 0.0,
        shown: None,
        known: HashMap::new(),
        drag: None,
        wheel: WheelSteps::default(),
    };
    // Make sure this thread has a message queue before anyone posts to it.
    let mut msg: MSG = std::mem::zeroed();
    PeekMessageW(&mut msg, null_mut(), WM_USER, WM_USER, PM_NOREMOVE);
    SetTimer(hwnd, TIMER_POLL, POLL_MS, None);
    let _ = ready.send(Some(GetCurrentThreadId()));

    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        let cfg = || shared.settings.lock().map(|s| s.clone()).ok();
        if msg.hwnd.is_null() && msg.message == WM_POKE {
            if let Some(cfg) = cfg().filter(|c| c.enabled) {
                state.shown = None;
                state.show(&backend, &shared, &cfg);
            }
        } else if msg.hwnd == hwnd {
            match msg.message {
                WM_TIMER if msg.wParam == TIMER_ANIM => {
                    if let Some(cfg) = cfg() {
                        state.tick(&backend, &cfg);
                    }
                }
                WM_TIMER if msg.wParam == TIMER_POLL => {
                    if let Some(cfg) = cfg() {
                        state.poll(&backend, &shared, &cfg);
                    }
                }
                WM_LBUTTONDOWN => state.mouse_down(),
                WM_MOUSEMOVE => {
                    if let Some(cfg) = cfg() {
                        state.mouse_move(&shared, &cfg);
                    }
                }
                WM_LBUTTONUP => {
                    if let Some(event) = state.mouse_up() {
                        let _ = events.send(event);
                        repaint();
                    }
                }
                WM_MOUSEWHEEL => {
                    let delta = ((msg.wParam >> 16) & 0xFFFF) as u16 as i16 as i32;
                    if let Some(cfg) = cfg() {
                        state.wheel(&backend, &cfg, delta);
                    }
                }
                WM_MBUTTONUP => state.toggle_mute(&backend),
                WM_CAPTURECHANGED => state.drag = None,
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        } else {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

impl State {
    /// Notice volume or mute changes on any playback device and show the one that moved.
    unsafe fn poll(&mut self, backend: &Handle, shared: &Shared, cfg: &OsdSettings) {
        let snap = backend.snapshot();
        let mut changed = None;
        for d in snap.devices.iter().filter(|d| d.flow == Flow::Render) {
            let now = (d.volume, d.muted);
            if let Some(prev) = self.known.insert(d.id.clone(), now) {
                if (prev.0 - now.0).abs() > CHANGE_EPSILON || prev.1 != now.1 {
                    changed = Some(d.id.clone());
                }
            }
        }
        if let Some(id) = changed {
            if cfg.enabled && !shared.suppressed.load(Ordering::Relaxed) {
                self.shown = Some(id);
                self.show(backend, shared, cfg);
            }
        }
    }

    /// Where this showing goes: the place, and which taskbar edge the tail points at.
    fn plan(&self, cfg: &OsdSettings) -> (Place, Option<Edge>) {
        match cfg.position {
            OsdPosition::AboveTray => {
                let edge = taskbar_edge(self.monitor.full, self.monitor.work, self.anchor);
                (
                    Place::Dock {
                        edge,
                        anchor: self.anchor,
                    },
                    Some(edge),
                )
            }
            OsdPosition::Custom if cfg.custom != (0, 0) => (
                Place::Custom {
                    x: cfg.custom.0,
                    y: cfg.custom.1,
                },
                None,
            ),
            _ => (Place::Centre, None),
        }
    }

    unsafe fn show(&mut self, backend: &Handle, shared: &Shared, cfg: &OsdSettings) {
        if fullscreen_foreground() {
            return;
        }
        if cfg.hide_windows_popup {
            super::winosd::hide_for_a_moment();
        }
        self.hide_at = Some(Instant::now() + Duration::from_millis(cfg.timeout_ms as u64));
        if self.animating {
            return;
        }
        // Fresh appearance: find the tray icon and the display it lives on, then pop out of it.
        self.anchor = shared.tray.lock().ok().and_then(|t| *t);
        self.monitor = match cfg.position {
            OsdPosition::AboveTray => match self.anchor {
                Some((x, y)) => MonitorInfo::at(x, y),
                None => MonitorInfo::primary(),
            },
            OsdPosition::Custom if cfg.custom != (0, 0) => {
                MonitorInfo::at(cfg.custom.0, cfg.custom.1)
            }
            _ => MonitorInfo::foreground(),
        };
        self.animating = true;
        self.pop = 0.0;
        self.alpha = 0.0;
        self.last_frame = Instant::now();
        if let Some(f) = read_frame(backend, &self.shown, &cfg.device_names) {
            self.display_pct = f.percent as f32;
        }
        SetTimer(self.hwnd, TIMER_ANIM, FRAME_MS, None);
        self.draw(backend, cfg);
        ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        SetWindowPos(
            self.hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }

    unsafe fn tick(&mut self, backend: &Handle, cfg: &OsdSettings) {
        // Timers are coarse and get coalesced under load, so step by the time that really passed
        // rather than assuming a perfect 16 ms.
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_frame)
            .as_secs_f32()
            .clamp(0.001, 0.1);
        self.last_frame = now;
        let frames = dt / (FRAME_MS as f32 / 1000.0);
        let approach = |per_frame: f32| 1.0 - (1.0 - per_frame).powf(frames);

        // Keep it up while the pointer is on it, since it's clickable and scrollable.
        if self.alpha > 0.5 && cursor_over(self.hwnd) {
            self.hide_at = Some(now + Duration::from_millis(400));
        }

        let expired = self.hide_at.map(|at| now >= at).unwrap_or(true);
        if expired && self.drag.is_none() {
            self.alpha -= dt / FADE_OUT_SECS;
            self.pop = (self.pop - dt / POP_OUT_SECS).max(0.0);
            if self.alpha <= 0.0 {
                self.alpha = 0.0;
                self.pop = 0.0;
                self.animating = false;
                self.hide_at = None;
                self.shown = None;
                ShowWindow(self.hwnd, SW_HIDE);
                KillTimer(self.hwnd, TIMER_ANIM);
                return;
            }
        } else {
            self.alpha = (self.alpha + dt / FADE_IN_SECS).min(1.0);
            self.pop = (self.pop + dt / POP_IN_SECS).min(1.0);
        }

        // Meter: snap up, fall back slowly, as a VU meter should; peak marker falls slower.
        let (level, target) = match read_frame(backend, &self.shown, &cfg.device_names) {
            Some(f) => (f.meter, f.percent as f32),
            None => (0.0, self.display_pct),
        };
        self.meter = if level > self.meter {
            level
        } else {
            self.meter + (level - self.meter) * approach(0.25)
        };
        self.meter_peak = if self.meter >= self.meter_peak {
            self.meter
        } else {
            (self.meter_peak - dt * 0.4).max(self.meter)
        };
        let diff = target - self.display_pct;
        if diff.abs() < 0.4 {
            self.display_pct = target;
        } else {
            self.display_pct += diff * approach(0.3);
        }
        self.draw(backend, cfg);
    }

    unsafe fn rebuild_surface(&mut self, cfg: &OsdSettings, tail: bool) {
        let dpi = self.monitor.dpi;
        let (w, h) = layout::size_for(cfg.vertical, dpi, tail);
        self.surface = LayeredSurface::new(w, h);
        self.surface_key = (dpi, cfg.vertical, tail);
    }

    /// Repaints the card and pushes it to the layered window.
    unsafe fn draw(&mut self, backend: &Handle, cfg: &OsdSettings) {
        let (place, tail_edge) = self.plan(cfg);
        let tail = tail_edge.is_some_and(has_tail);
        if self.surface.is_none() || self.surface_key != (self.monitor.dpi, cfg.vertical, tail) {
            self.rebuild_surface(cfg, tail);
        }
        let Some(surface) = &self.surface else { return };
        let Some(mut frame) = read_frame(backend, &self.shown, &cfg.device_names) else {
            return;
        };
        frame.meter = self.meter;
        frame.meter_peak = self.meter_peak;

        let dpi = self.surface_key.0;
        let s = |v: f32| v * dpi as f32 / 96.0;
        let canvas = surface.canvas();

        // Grow out of (and shrink back into) the tray icon.
        let pop = ease_out_cubic(self.pop.clamp(0.0, 1.0));
        let scale = MIN_SCALE + (1.0 - MIN_SCALE) * pop;
        GdipResetWorldTransform(canvas.graphics);
        GdipGraphicsClear(canvas.graphics, 0);
        GdipScaleWorldTransform(canvas.graphics, scale, scale, MatrixOrderPrepend);

        let pad = s(SHADOW as f32);
        let tail_h = if tail { s(TAIL_H as f32) } else { 0.0 };
        let tail_up = tail && tail_edge == Some(Edge::Top);
        // A tail on top pushes the whole card down to make room for it.
        if tail_up {
            GdipTranslateWorldTransform(canvas.graphics, 0.0, tail_h, MatrixOrderPrepend);
        }
        let card_w = surface.width as f32 - pad * 2.0;
        let card_h = surface.height as f32 - pad * 2.0 - tail_h;
        let vertical = cfg.vertical;
        let radius = s(if vertical { 22.0 } else { 16.0 });

        draw_shadow(&canvas, pad, card_w, card_h, radius, s(1.0));
        // Card body: subtle vertical gradient with a hairline border.
        canvas.fill_round_rect_gradient(
            pad,
            pad,
            card_w,
            card_h,
            radius,
            card_top(),
            card_bottom(),
        );
        canvas.stroke_round_rect(
            pad + 0.5,
            pad + 0.5,
            card_w - 1.0,
            card_h - 1.0,
            radius,
            edge_color(),
            s(1.0),
        );

        let (end_x, end_y) =
            layout::origin(place, self.monitor.work, surface.width, surface.height, dpi);
        if tail {
            // Point at the icon's real x, not just the card's middle, so the tail stays accurate
            // when the card is clamped against a screen edge.
            let tail_cx = match self.anchor {
                Some((tx, _)) => {
                    let local = (tx - end_x) as f32;
                    local.clamp(pad + radius + s(6.0), pad + card_w - radius - s(6.0))
                }
                None => pad + card_w / 2.0,
            };
            draw_tail(
                &canvas,
                tail_cx,
                pad,
                card_h,
                s(TAIL_W as f32),
                tail_h,
                tail_up,
            );
        }

        let accent = if cfg.accent_system {
            system_accent().unwrap_or(DEFAULT_ACCENT)
        } else {
            DEFAULT_ACCENT
        };
        let pct = (self.display_pct / 100.0).clamp(0.0, 1.0);
        if vertical {
            draw_vertical(&canvas, &frame, pad, card_w, card_h, &s, accent, pct);
        } else {
            draw_horizontal(&canvas, &frame, pad, card_w, card_h, &s, accent, pct);
        }
        drop(canvas);

        let draw_w = (surface.width as f32 * scale).round() as i32;
        let draw_h = (surface.height as f32 * scale).round() as i32;
        let (x, y) = match (&place, self.anchor) {
            // Travel out of the tray icon toward the resting place.
            (Place::Dock { .. }, Some((tx, ty))) => {
                let start_x = (tx - draw_w / 2) as f32;
                let start_y = (ty - draw_h / 2) as f32;
                (
                    (start_x + (end_x as f32 - start_x) * pop).round() as i32,
                    (start_y + (end_y as f32 - start_y) * pop).round() as i32,
                )
            }
            // Floating: grow about its own centre instead of flying across the screen.
            _ => (
                end_x + (surface.width - draw_w) / 2,
                end_y + (surface.height - draw_h) / 2,
            ),
        };
        let opacity = (cfg.opacity_pct.clamp(20, 100) as f32 / 100.0) * self.alpha;
        surface.commit(
            self.hwnd,
            x,
            y,
            draw_w,
            draw_h,
            (opacity.clamp(0.0, 1.0) * 255.0) as u8,
        );
    }

    // --- mouse ---------------------------------------------------------------------------------

    unsafe fn mouse_down(&mut self) {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let mut rc: RECT = std::mem::zeroed();
        GetWindowRect(self.hwnd, &mut rc);
        self.drag = Some(Drag {
            cursor: (pt.x, pt.y),
            window: (rc.left, rc.top),
            moved: false,
        });
        SetCapture(self.hwnd);
    }

    unsafe fn mouse_move(&mut self, shared: &Shared, cfg: &OsdSettings) {
        let Some(drag) = &mut self.drag else { return };
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let (dx, dy) = (pt.x - drag.cursor.0, pt.y - drag.cursor.1);
        if !drag.moved && dx.abs() + dy.abs() <= 6 {
            return;
        }
        if !drag.moved {
            drag.moved = true;
            // A real drag repositions the bar for good; drop the pop animation so it tracks the
            // cursor one to one.
            self.pop = 1.0;
            self.alpha = 1.0;
        }
        let (x, y) = (drag.window.0 + dx, drag.window.1 + dy);
        self.hide_at = Some(Instant::now() + Duration::from_millis(600));
        if let Ok(mut s) = shared.settings.lock() {
            s.position = OsdPosition::Custom;
            s.custom = (x, y);
        }
        let _ = cfg;
        SetWindowPos(
            self.hwnd,
            null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }

    /// Ends a press. A drag reports where the bar ended up; a plain click asks the app to open.
    unsafe fn mouse_up(&mut self) -> Option<OsdEvent> {
        let drag = self.drag.take()?;
        ReleaseCapture();
        if drag.moved {
            let mut rc: RECT = std::mem::zeroed();
            GetWindowRect(self.hwnd, &mut rc);
            // Floating now, so the tray tail is gone: redraw for the new shape.
            self.surface = None;
            Some(OsdEvent::Moved(rc.left, rc.top))
        } else {
            Some(OsdEvent::Open)
        }
    }

    fn target_device(&self, backend: &Handle) -> Option<crate::model::Device> {
        let snap = backend.snapshot();
        snap.devices
            .into_iter()
            .filter(|d| d.flow == Flow::Render)
            .find(|d| match &self.shown {
                Some(id) => &d.id == id,
                None => d.is_default,
            })
    }

    unsafe fn wheel(&mut self, backend: &Handle, cfg: &OsdSettings, delta: i32) {
        let steps = self.wheel.feed(delta);
        if steps == 0 {
            return;
        }
        if let Some(d) = self.target_device(backend) {
            let step = cfg.volume_step as f32 / 100.0;
            backend.send(Command::SetDeviceVolume {
                id: d.id,
                volume: (d.volume + steps as f32 * step).clamp(0.0, 1.0),
            });
            self.hide_at = Some(Instant::now() + Duration::from_millis(cfg.timeout_ms as u64));
        }
    }

    unsafe fn toggle_mute(&mut self, backend: &Handle) {
        if let Some(d) = self.target_device(backend) {
            backend.send(Command::SetDeviceMute {
                id: d.id,
                muted: !d.muted,
            });
        }
    }
}

/// The device the OSD shows: the one that changed, or the default.
fn read_frame(
    backend: &Handle,
    shown: &Option<String>,
    names: &HashMap<String, String>,
) -> Option<Frame> {
    let snap = backend.snapshot();
    let d = snap
        .devices
        .iter()
        .filter(|d| d.flow == Flow::Render)
        .find(|d| match shown {
            Some(id) => &d.id == id,
            None => d.is_default,
        })
        .or_else(|| {
            snap.devices
                .iter()
                .find(|d| d.flow == Flow::Render && d.is_default)
        })?;
    let peak_db = amp_to_db(d.peak);
    let label = match names.get(&d.id).map(|n| n.trim()).filter(|n| !n.is_empty()) {
        Some(own) => own.to_string(),
        None => d.name.split(" (").next().unwrap_or(&d.name).to_string(),
    };
    Some(Frame {
        label,
        percent: crate::model::percent(d.volume),
        muted: d.muted,
        meter: if d.muted { 0.0 } else { db_to_meter(peak_db) },
        meter_peak: 0.0,
        peak_db,
    })
}

fn ease_out_cubic(t: f32) -> f32 {
    let inv = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Windows' current accent colour via DWM colorization.
fn system_accent() -> Option<(u8, u8, u8)> {
    unsafe {
        let mut color: u32 = 0;
        let mut opaque: i32 = 0;
        if DwmGetColorizationColor(&mut color, &mut opaque) != 0 {
            return None;
        }
        Some(((color >> 16) as u8, (color >> 8) as u8, color as u8))
    }
}

fn cursor_over(hwnd: HWND) -> bool {
    unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        let mut rc: RECT = std::mem::zeroed();
        GetCursorPos(&mut pt) != 0
            && GetWindowRect(hwnd, &mut rc) != 0
            && pt.x >= rc.left
            && pt.x < rc.right
            && pt.y >= rc.top
            && pt.y < rc.bottom
    }
}

// --- monitors ---------------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub struct MonitorInfo {
    pub full: Area,
    pub work: Area,
    pub dpi: i32,
}

impl MonitorInfo {
    pub fn primary() -> Self {
        unsafe {
            metrics_of(MonitorFromPoint(
                POINT { x: 0, y: 0 },
                MONITOR_DEFAULTTOPRIMARY,
            ))
        }
    }

    /// The display holding a screen point.
    pub fn at(x: i32, y: i32) -> Self {
        unsafe { metrics_of(MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST)) }
    }

    /// The display the user is working on: the one holding the foreground window.
    pub fn foreground() -> Self {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() {
                return Self::primary();
            }
            metrics_of(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST))
        }
    }
}

fn area(r: RECT) -> Area {
    Area {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

unsafe fn metrics_of(monitor: HMONITOR) -> MonitorInfo {
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut mi) == 0 {
        let screen = Area {
            left: 0,
            top: 0,
            right: GetSystemMetrics(SM_CXSCREEN),
            bottom: GetSystemMetrics(SM_CYSCREEN),
        };
        return MonitorInfo {
            full: screen,
            work: screen,
            dpi: 96,
        };
    }
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    MonitorInfo {
        full: area(mi.rcMonitor),
        work: area(mi.rcWork),
        dpi: (dpi_x as i32).max(96),
    }
}

/// True when a game or presentation is filling a screen, so a topmost overlay would intrude.
fn fullscreen_foreground() -> bool {
    unsafe {
        let mut state = 0;
        if SHQueryUserNotificationState(&mut state) == 0
            && (state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE)
        {
            return true;
        }
        // Borderless-fullscreen windows don't set that state, so also check whether the active
        // window covers its entire monitor.
        let foreground = GetForegroundWindow();
        if foreground.is_null() || foreground == GetShellWindow() || is_desktop(foreground) {
            return false;
        }
        let mut rc: RECT = std::mem::zeroed();
        if GetWindowRect(foreground, &mut rc) == 0 {
            return false;
        }
        let m = MonitorInfo::foreground().full;
        rc.left <= m.left && rc.top <= m.top && rc.right >= m.right && rc.bottom >= m.bottom
    }
}

/// The desktop is a monitor-sized window too: clicking the wallpaper focuses `WorkerW` (or
/// `Progman`), which must not count as a fullscreen app.
unsafe fn is_desktop(hwnd: HWND) -> bool {
    let mut buf = [0u16; 32];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    let class = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
    class == "WorkerW" || class == "Progman"
}

// --- palette ----------------------------------------------------------------------------------

/// Whether the bar is drawn on a dark card. Set from the app's theme; read while drawing.
static DARK: AtomicBool = AtomicBool::new(false);

fn dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

/// Text and symbols: dark brown on the light card, warm white on the dark one.
fn ink(alpha: u8) -> u32 {
    if dark() {
        argb(alpha, 245, 240, 232)
    } else {
        argb(alpha, 58, 50, 40)
    }
}

fn card_top() -> u32 {
    if dark() {
        argb(247, 40, 34, 28)
    } else {
        argb(250, 255, 254, 251)
    }
}

fn card_bottom() -> u32 {
    if dark() {
        argb(247, 28, 23, 19)
    } else {
        argb(250, 246, 242, 235)
    }
}

fn edge_color() -> u32 {
    if dark() {
        argb(30, 255, 255, 255)
    } else {
        argb(46, 58, 50, 40)
    }
}

// --- drawing (ported from voicemeeter-osd) ----------------------------------------------------

/// Green through amber to red as the signal approaches clipping.
fn meter_color(level: f32) -> u32 {
    if level > 0.96 {
        argb(255, 219, 70, 64)
    } else if level > 0.88 {
        argb(255, 227, 167, 47)
    } else {
        argb(255, 88, 176, 98)
    }
}

/// Callout tail on the card's bottom edge, or its top edge when `up`. Drawn a touch inside the
/// card's edge so it merges with the body instead of showing a seam.
unsafe fn draw_tail(
    canvas: &Canvas,
    cx: f32,
    card_top: f32,
    card_h: f32,
    width: f32,
    height: f32,
    up: bool,
) {
    let half = width / 2.0;
    let (base, tip) = if up {
        (card_top + 1.0, card_top - height)
    } else {
        (card_top + card_h - 1.0, card_top + card_h + height)
    };
    canvas.fill_polygon(
        &[(cx - half, base), (cx + half, base), (cx, tip)],
        card_bottom(),
    );
    let edge = edge_color();
    canvas.stroke_line(cx - half, base, cx, tip, edge, 1.0);
    canvas.stroke_line(cx + half, base, cx, tip, edge, 1.0);
}

/// Cheap fake blur: concentric rounded rects with falling alpha.
unsafe fn draw_shadow(canvas: &Canvas, pad: f32, w: f32, h: f32, radius: f32, unit: f32) {
    let layers = 6;
    for i in (1..=layers).rev() {
        let spread = unit * i as f32 * 1.5;
        let alpha = (16.0 / i as f32) as u8;
        canvas.fill_round_rect(
            pad - spread,
            pad - spread + unit * 1.5,
            w + spread * 2.0,
            h + spread * 2.0,
            radius + spread,
            argb(alpha, 0, 0, 0),
        );
    }
}

unsafe fn draw_speaker(canvas: &Canvas, cx: f32, cy: f32, size: f32, muted: bool, color: u32) {
    let s = size;
    let body = [
        (cx - s * 0.42, cy - s * 0.16),
        (cx - s * 0.18, cy - s * 0.16),
        (cx + s * 0.10, cy - s * 0.44),
        (cx + s * 0.10, cy + s * 0.44),
        (cx - s * 0.18, cy + s * 0.16),
        (cx - s * 0.42, cy + s * 0.16),
    ];
    canvas.fill_polygon(&body, color);

    if muted {
        let x = cx + s * 0.24;
        let r = s * 0.26;
        canvas.stroke_line(x, cy - r, x + r * 1.5, cy + r, color, s * 0.14);
        canvas.stroke_line(x, cy + r, x + r * 1.5, cy - r, color, s * 0.14);
    } else {
        canvas.stroke_arc(cx + s * 0.06, cy, s * 0.30, -52.0, 104.0, color, s * 0.13);
        canvas.stroke_arc(
            cx + s * 0.06,
            cy,
            s * 0.50,
            -52.0,
            104.0,
            ink(((color >> 24) as u8).saturating_sub(70)),
            s * 0.13,
        );
    }
}

fn db_text(frame: &Frame) -> String {
    if frame.muted || frame.peak_db <= -59.9 {
        String::new()
    } else {
        format!("{:.1} dB", frame.peak_db)
    }
}

fn percent_text(frame: &Frame) -> String {
    if frame.muted {
        "Muted".to_string()
    } else {
        format!("{}%", frame.percent)
    }
}

/// (light, strong) fill colours; muted fills go red.
fn fill_colors(muted: bool, accent: (u8, u8, u8)) -> (u32, u32) {
    if muted {
        (
            tint(shade((150, 62, 62), 0.25), 255),
            tint((150, 62, 62), 255),
        )
    } else {
        (tint(shade(accent, 0.28), 255), tint(accent, 255))
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_vertical(
    canvas: &Canvas,
    frame: &Frame,
    pad: f32,
    card_w: f32,
    card_h: f32,
    s: &dyn Fn(f32) -> f32,
    accent: (u8, u8, u8),
    pct: f32,
) {
    let cx = pad + card_w / 2.0;
    draw_speaker(
        canvas,
        cx,
        pad + s(24.0),
        s(20.0),
        frame.muted,
        ink(235),
    );

    let track_w = s(10.0);
    // Centre the track+meter pair as a group, not the track alone.
    let track_x = cx - s(10.5);
    let track_top = pad + s(58.0);
    let track_bottom = pad + card_h - s(42.0);
    let track_h = track_bottom - track_top;
    canvas.fill_round_rect(
        track_x,
        track_top,
        track_w,
        track_h,
        track_w / 2.0,
        ink(38),
    );

    // Live signal meter alongside the fader, so you can see level as well as the setting.
    let meter_w = s(4.0);
    let meter_x = track_x + track_w + s(7.0);
    canvas.fill_round_rect(
        meter_x,
        track_top,
        meter_w,
        track_h,
        meter_w / 2.0,
        ink(30),
    );
    let meter_h = track_h * frame.meter.clamp(0.0, 1.0);
    if meter_h > 0.5 {
        canvas.fill_round_rect(
            meter_x,
            track_bottom - meter_h,
            meter_w,
            meter_h,
            meter_w / 2.0,
            meter_color(frame.meter),
        );
    }
    if frame.meter_peak > 0.02 {
        let y = track_bottom - track_h * frame.meter_peak.clamp(0.0, 1.0);
        canvas.fill_round_rect(
            meter_x,
            y - s(1.0),
            meter_w,
            s(2.0),
            s(1.0),
            meter_color(frame.meter_peak),
        );
    }

    let fill_h = track_h * pct;
    if fill_h > 0.5 {
        let (top_color, bottom_color) = fill_colors(frame.muted, accent);
        canvas.fill_round_rect_gradient(
            track_x,
            track_bottom - fill_h,
            track_w,
            fill_h,
            track_w / 2.0,
            top_color,
            bottom_color,
        );
    }

    canvas.draw_text(
        &frame.label,
        pad,
        pad + s(38.0),
        card_w,
        s(14.0),
        s(10.5),
        ink(150),
        ALIGN_CENTER,
        false,
    );
    canvas.draw_text(
        &percent_text(frame),
        pad,
        pad + card_h - s(36.0),
        card_w,
        s(20.0),
        s(14.0),
        ink(240),
        ALIGN_CENTER,
        true,
    );
    canvas.draw_text(
        &db_text(frame),
        pad,
        pad + card_h - s(20.0),
        card_w,
        s(14.0),
        s(10.5),
        ink(140),
        ALIGN_CENTER,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
unsafe fn draw_horizontal(
    canvas: &Canvas,
    frame: &Frame,
    pad: f32,
    card_w: f32,
    card_h: f32,
    s: &dyn Fn(f32) -> f32,
    accent: (u8, u8, u8),
    pct: f32,
) {
    draw_speaker(
        canvas,
        pad + s(34.0),
        pad + card_h / 2.0,
        s(26.0),
        frame.muted,
        ink(235),
    );

    let track_left = pad + s(62.0);
    let track_right = pad + card_w - s(74.0);
    let track_h = s(10.0);
    let track_y = pad + card_h / 2.0 - track_h / 2.0 + s(6.0);
    let track_w = track_right - track_left;

    canvas.draw_text(
        &frame.label,
        track_left,
        pad + s(14.0),
        track_w,
        s(14.0),
        s(10.5),
        ink(150),
        ALIGN_NEAR,
        false,
    );
    canvas.fill_round_rect(
        track_left,
        track_y,
        track_w,
        track_h,
        track_h / 2.0,
        ink(38),
    );

    // Live signal meter under the fader, with the same peak-hold behaviour as the vertical layout.
    let meter_h = s(4.0);
    let meter_y = track_y + track_h + s(8.0);
    canvas.fill_round_rect(
        track_left,
        meter_y,
        track_w,
        meter_h,
        meter_h / 2.0,
        ink(30),
    );
    let meter_w = track_w * frame.meter.clamp(0.0, 1.0);
    if meter_w > 0.5 {
        canvas.fill_round_rect(
            track_left,
            meter_y,
            meter_w,
            meter_h,
            meter_h / 2.0,
            meter_color(frame.meter),
        );
    }
    if frame.meter_peak > 0.02 {
        let x = track_left + track_w * frame.meter_peak.clamp(0.0, 1.0);
        canvas.fill_round_rect(
            x - s(1.0),
            meter_y - s(1.0),
            s(2.0),
            meter_h + s(2.0),
            s(1.0),
            meter_color(frame.meter_peak),
        );
    }

    let fill_w = track_w * pct;
    if fill_w > 0.5 {
        // Strong colour at the left, fading lighter toward the tip.
        let (light, strong) = fill_colors(frame.muted, accent);
        canvas.fill_round_rect_gradient(
            track_left,
            track_y,
            fill_w,
            track_h,
            track_h / 2.0,
            strong,
            light,
        );
    }

    canvas.draw_text(
        &percent_text(frame),
        pad + card_w - s(70.0),
        pad + card_h / 2.0 - s(14.0),
        s(58.0),
        s(20.0),
        s(15.0),
        ink(240),
        ALIGN_CENTER,
        true,
    );
    canvas.draw_text(
        &db_text(frame),
        pad + card_w - s(70.0),
        pad + card_h / 2.0 + s(6.0),
        s(58.0),
        s(14.0),
        s(10.5),
        ink(140),
        ALIGN_CENTER,
        false,
    );
}
