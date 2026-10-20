//! The on-screen volume display, ported from voicemeeter-osd: a click-through, never-activating,
//! always-on-top layered window drawn with GDI+. It runs on its own thread with its own message
//! loop, so it keeps animating whether or not the mixer window exists, and it follows the default
//! playback device by reading the engine's snapshot.

use std::ptr::null_mut;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows_sys::Win32::Graphics::GdiPlus::{
    GdipGraphicsClear, GdipResetWorldTransform, GdipScaleWorldTransform, MatrixOrderPrepend,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::gfx::{argb, shade, tint, Canvas, Gdiplus, LayeredSurface, ALIGN_CENTER, ALIGN_NEAR};
use super::layout::{self, Area, SHADOW};
use crate::backend::Handle;
use crate::model::{amp_to_db, db_to_meter, Flow};

const CLASS_NAME: &str = "WavemixOsdClass";
const WM_POKE: u32 = WM_APP + 1;
const FRAME_MS: u32 = 16;
const TIMER_ID: usize = 1;

const HOLD: Duration = Duration::from_millis(1400);
const FADE_IN_SECS: f32 = 0.11;
const FADE_OUT_SECS: f32 = 0.22;
const POP_IN_SECS: f32 = 0.26;
const POP_OUT_SECS: f32 = 0.20;
/// Smallest scale at the start of the pop, so it grows out of nothing rather than appearing.
const MIN_SCALE: f32 = 0.28;
/// Matches the mixer's own accent blue.
const ACCENT: (u8, u8, u8) = (0x4c, 0xa8, 0xff);

#[derive(Clone, Debug, PartialEq)]
pub struct OsdSettings {
    pub enabled: bool,
    pub vertical: bool,
    pub opacity_pct: u32,
}

/// Handle to the display thread.
pub struct Osd {
    thread_id: u32,
    settings: Arc<Mutex<OsdSettings>>,
}

impl Osd {
    /// Start the display thread. `None` if the window couldn't be created.
    pub fn new(backend: Handle, settings: OsdSettings) -> Option<Self> {
        let shared = Arc::new(Mutex::new(settings));
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        thread::Builder::new()
            .name("osd".into())
            .spawn(move || unsafe { run(backend, s, tx) })
            .ok()?;
        let thread_id = rx.recv().ok().flatten()?;
        Some(Self {
            thread_id,
            settings: shared,
        })
    }

    pub fn set_settings(&self, settings: OsdSettings) {
        if let Ok(mut s) = self.settings.lock() {
            *s = settings;
        }
    }

    /// The volume just changed: show the display, or keep it up a little longer.
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

struct State {
    hwnd: HWND,
    surface: Option<LayeredSurface>,
    surface_key: (i32, bool),
    monitor: MonitorInfo,
    animating: bool,
    hide_at: Option<Instant>,
    last_frame: Instant,
    alpha: f32,
    pop: f32,
    display_pct: f32,
    meter: f32,
    meter_peak: f32,
}

unsafe fn run(
    backend: Handle,
    settings: Arc<Mutex<OsdSettings>>,
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
    // Click-through (TRANSPARENT) and never activated (NOACTIVATE): a volume popup must not take
    // focus from a game or steal the click you meant for the window underneath.
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
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
        surface_key: (0, false),
        monitor: MonitorInfo::primary(),
        animating: false,
        hide_at: None,
        last_frame: Instant::now(),
        alpha: 0.0,
        pop: 0.0,
        display_pct: 0.0,
        meter: 0.0,
        meter_peak: 0.0,
    };
    // Make sure this thread has a message queue before anyone posts to it.
    let mut msg: MSG = std::mem::zeroed();
    PeekMessageW(&mut msg, null_mut(), WM_USER, WM_USER, PM_NOREMOVE);
    let _ = ready.send(Some(GetCurrentThreadId()));

    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        if msg.hwnd.is_null() && msg.message == WM_POKE {
            let cfg = settings.lock().map(|s| s.clone()).ok();
            if let Some(cfg) = cfg.filter(|c| c.enabled) {
                state.poke(&backend, &cfg);
            }
        } else if msg.hwnd == hwnd && msg.message == WM_TIMER {
            let cfg = settings.lock().map(|s| s.clone()).ok();
            if let Some(cfg) = cfg {
                state.tick(&backend, &cfg);
            }
        } else {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

impl State {
    unsafe fn poke(&mut self, backend: &Handle, cfg: &OsdSettings) {
        if fullscreen_foreground() {
            return;
        }
        self.hide_at = Some(Instant::now() + HOLD);
        if !self.animating {
            // Fresh appearance: pick the display being worked on, start from the current level
            // so the bar doesn't sweep up from zero.
            self.animating = true;
            self.monitor = MonitorInfo::foreground();
            self.pop = 0.0;
            self.alpha = 0.0;
            self.last_frame = Instant::now();
            if let Some(f) = read_frame(backend) {
                self.display_pct = f.percent as f32;
            }
            SetTimer(self.hwnd, TIMER_ID, FRAME_MS, None);
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

        let expired = self.hide_at.map(|at| now >= at).unwrap_or(true);
        if expired {
            self.alpha -= dt / FADE_OUT_SECS;
            self.pop = (self.pop - dt / POP_OUT_SECS).max(0.0);
            if self.alpha <= 0.0 {
                self.alpha = 0.0;
                self.pop = 0.0;
                self.animating = false;
                self.hide_at = None;
                ShowWindow(self.hwnd, SW_HIDE);
                KillTimer(self.hwnd, TIMER_ID);
                return;
            }
        } else {
            self.alpha = (self.alpha + dt / FADE_IN_SECS).min(1.0);
            self.pop = (self.pop + dt / POP_IN_SECS).min(1.0);
        }

        // Meter: snap up, fall back slowly, as a VU meter should; peak marker falls away slower.
        let (level, target) = match read_frame(backend) {
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

    unsafe fn rebuild_surface(&mut self, cfg: &OsdSettings) {
        let dpi = self.monitor.dpi;
        let (w, h) = layout::size_for(cfg.vertical, dpi);
        self.surface = LayeredSurface::new(w, h);
        self.surface_key = (dpi, cfg.vertical);
    }

    /// Repaints the card and pushes it to the layered window.
    unsafe fn draw(&mut self, backend: &Handle, cfg: &OsdSettings) {
        if self.surface.is_none() || self.surface_key != (self.monitor.dpi, cfg.vertical) {
            self.rebuild_surface(cfg);
        }
        let Some(surface) = &self.surface else { return };
        let Some(mut frame) = read_frame(backend) else {
            return;
        };
        frame.meter = self.meter;
        frame.meter_peak = self.meter_peak;

        let dpi = self.surface_key.0;
        let s = |v: f32| v * dpi as f32 / 96.0;
        let canvas = surface.canvas();

        let pop = ease_out_cubic(self.pop.clamp(0.0, 1.0));
        let scale = MIN_SCALE + (1.0 - MIN_SCALE) * pop;
        GdipResetWorldTransform(canvas.graphics);
        GdipGraphicsClear(canvas.graphics, 0);
        GdipScaleWorldTransform(canvas.graphics, scale, scale, MatrixOrderPrepend);

        let pad = s(SHADOW as f32);
        let card_w = surface.width as f32 - pad * 2.0;
        let card_h = surface.height as f32 - pad * 2.0;
        let radius = s(if cfg.vertical { 22.0 } else { 16.0 });

        draw_shadow(&canvas, pad, card_w, card_h, radius, s(1.0));
        // Card body: subtle vertical gradient with a hairline border.
        canvas.fill_round_rect_gradient(
            pad,
            pad,
            card_w,
            card_h,
            radius,
            argb(247, 38, 38, 42),
            argb(247, 24, 24, 27),
        );
        canvas.stroke_round_rect(
            pad + 0.5,
            pad + 0.5,
            card_w - 1.0,
            card_h - 1.0,
            radius,
            argb(26, 255, 255, 255),
            s(1.0),
        );

        let pct = (self.display_pct / 100.0).clamp(0.0, 1.0);
        if cfg.vertical {
            draw_vertical(&canvas, &frame, pad, card_w, card_h, &s, pct);
        } else {
            draw_horizontal(&canvas, &frame, pad, card_w, card_h, &s, pct);
        }
        drop(canvas);

        // Floating: grow about its own centre.
        let draw_w = (surface.width as f32 * scale).round() as i32;
        let draw_h = (surface.height as f32 * scale).round() as i32;
        let (ox, oy) = layout::origin_for(self.monitor.work, surface.width, surface.height);
        let x = ox + (surface.width - draw_w) / 2;
        let y = oy + (surface.height - draw_h) / 2;
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
}

/// The default playback device as the OSD shows it.
fn read_frame(backend: &Handle) -> Option<Frame> {
    let snap = backend.snapshot();
    let d = snap
        .devices
        .iter()
        .find(|d| d.flow == Flow::Render && d.is_default)?;
    let peak_db = amp_to_db(d.peak);
    Some(Frame {
        label: d.name.clone(),
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

// --- monitors ---------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct MonitorInfo {
    work: Area,
    dpi: i32,
}

impl MonitorInfo {
    fn primary() -> Self {
        unsafe {
            metrics_of(MonitorFromPoint(
                POINT { x: 0, y: 0 },
                MONITOR_DEFAULTTOPRIMARY,
            ))
        }
    }

    /// The display the user is working on: the one holding the foreground window.
    fn foreground() -> Self {
        unsafe {
            let fg = GetForegroundWindow();
            if fg.is_null() {
                return Self::primary();
            }
            metrics_of(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST))
        }
    }
}

unsafe fn metrics_of(monitor: HMONITOR) -> MonitorInfo {
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(monitor, &mut mi) == 0 {
        return MonitorInfo {
            work: Area {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            },
            dpi: 96,
        };
    }
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    MonitorInfo {
        work: Area {
            left: mi.rcWork.left,
            top: mi.rcWork.top,
            right: mi.rcWork.right,
            bottom: mi.rcWork.bottom,
        },
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
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(
            MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST),
            &mut mi,
        ) == 0
        {
            return false;
        }
        let full = mi.rcMonitor;
        rc.left <= full.left
            && rc.top <= full.top
            && rc.right >= full.right
            && rc.bottom >= full.bottom
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

// --- drawing (ported from voicemeeter-osd) ----------------------------------------------------

/// Green through amber to red as the signal approaches clipping.
fn meter_color(level: f32) -> u32 {
    if level > 0.96 {
        argb(255, 240, 80, 70)
    } else if level > 0.88 {
        argb(255, 235, 175, 70)
    } else {
        argb(255, 110, 205, 130)
    }
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
            argb(((color >> 24) as u8).saturating_sub(70), 255, 255, 255),
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

unsafe fn draw_vertical(
    canvas: &Canvas,
    frame: &Frame,
    pad: f32,
    card_w: f32,
    card_h: f32,
    s: &dyn Fn(f32) -> f32,
    pct: f32,
) {
    let cx = pad + card_w / 2.0;
    draw_speaker(
        canvas,
        cx,
        pad + s(24.0),
        s(20.0),
        frame.muted,
        argb(235, 255, 255, 255),
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
        argb(38, 255, 255, 255),
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
        argb(30, 255, 255, 255),
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
        let (top_color, bottom_color) = fill_colors(frame.muted);
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
        argb(150, 255, 255, 255),
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
        argb(240, 255, 255, 255),
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
        argb(140, 255, 255, 255),
        ALIGN_CENTER,
        false,
    );
}

unsafe fn draw_horizontal(
    canvas: &Canvas,
    frame: &Frame,
    pad: f32,
    card_w: f32,
    card_h: f32,
    s: &dyn Fn(f32) -> f32,
    pct: f32,
) {
    draw_speaker(
        canvas,
        pad + s(34.0),
        pad + card_h / 2.0,
        s(26.0),
        frame.muted,
        argb(235, 255, 255, 255),
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
        argb(150, 255, 255, 255),
        ALIGN_NEAR,
        false,
    );
    canvas.fill_round_rect(
        track_left,
        track_y,
        track_w,
        track_h,
        track_h / 2.0,
        argb(38, 255, 255, 255),
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
        argb(30, 255, 255, 255),
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
        let (light, strong) = fill_colors(frame.muted);
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
        argb(240, 255, 255, 255),
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
        argb(140, 255, 255, 255),
        ALIGN_CENTER,
        false,
    );
}

/// (light, strong) fill colours; muted fills go red.
fn fill_colors(muted: bool) -> (u32, u32) {
    if muted {
        (
            tint(shade((150, 62, 62), 0.25), 255),
            tint((150, 62, 62), 255),
        )
    } else {
        (tint(shade(ACCENT, 0.28), 255), tint(ACCENT, 255))
    }
}
