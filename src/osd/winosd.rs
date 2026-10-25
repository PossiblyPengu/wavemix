//! Keeps Windows' own volume popup from appearing on top of ours.
//!
//! There is no setting for it, so this watches for a short while after our bar is shown and hides
//! the popup window if Windows opens it. The same shell process (`explorer.exe`) also owns the
//! taskbar, Start and Quick Settings, so the filter is narrow: a visible window of a class the
//! popup is known to use, owned by `explorer.exe`, and small compared with the screen.
//!
//! Every explorer-owned window it considers is appended to `%TEMP%\wavemix-winosd.txt` (once per
//! window per run), with what was done with it, so if a Windows version uses a different window
//! the log says which.

use std::collections::HashSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
    GetSystemMetrics, IsWindowVisible, ShowWindow, GWL_EXSTYLE, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE,
    WS_EX_TOPMOST,
};

/// Window classes the Windows 10 and 11 volume popups are drawn in.
const CLASSES: [&str; 2] = ["NativeHWNDHost", "XamlExplorerHostIslandWindow"];
/// The popup is small: at most this share of the screen. Bigger windows are some other piece of the
/// shell (Start, Search, Quick Settings).
const MAX_WIDTH_SHARE: f32 = 0.6;
const MAX_HEIGHT_SHARE: f32 = 0.25;
/// How often to look. Looking is cheap (a class-name check per visible window), and a short beat
/// keeps the popup from being on screen long enough to notice.
const TICK: Duration = Duration::from_millis(25);

static ENABLED: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Turn the watcher on or off. While on, a background thread hides Windows' volume popup the moment
/// it appears.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
    if on && !RUNNING.swap(true, Ordering::AcqRel) {
        thread::spawn(|| {
            let mut sweep = Sweep::default();
            while ENABLED.load(Ordering::Acquire) {
                unsafe { sweep.run() };
                thread::sleep(TICK);
            }
            RUNNING.store(false, Ordering::Release);
            // Turned back on between the loop ending and the flag clearing.
            if ENABLED.load(Ordering::Acquire) {
                set_enabled(true);
            }
        });
    }
}

#[derive(Default)]
struct Sweep {
    /// Windows already written to the log this run.
    logged: HashSet<isize>,
}

impl Sweep {
    unsafe fn run(&mut self) {
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            let sweep = &mut *(lparam as *mut Sweep);
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let class = class_name(hwnd);
            let listed = CLASSES.contains(&class.as_str());
            let mut rect: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut rect);
            let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
            let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let small = w > 0
                && h > 0
                && (w as f32) <= sw as f32 * MAX_WIDTH_SHARE
                && (h as f32) <= sh as f32 * MAX_HEIGHT_SHARE;
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            // Cheap checks first; only look up the owning process for plausible candidates.
            if !(listed || (small && ex & WS_EX_TOPMOST != 0)) {
                return 1;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId() || !process_name(pid).eq_ignore_ascii_case("explorer.exe")
            {
                return 1;
            }
            let hide = listed && small;
            // Log what a volume popup might be, once per window.
            if sweep.logged.len() < 200 && sweep.logged.insert(hwnd as isize) {
                log(&format!(
                    "{class} {w}x{h} at ({},{}) ex={ex:#x} -> {}",
                    rect.left,
                    rect.top,
                    if hide { "hidden" } else { "left alone" }
                ));
            }
            if hide {
                ShowWindow(hwnd, SW_HIDE);
            }
            1
        }
        EnumWindows(Some(each), self as *mut Sweep as LPARAM);
    }
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe fn process_name(pid: u32) -> String {
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h.is_null() {
        return String::new();
    }
    let mut buf = [0u16; 520];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
    CloseHandle(h);
    if ok == 0 {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit('\\').next().unwrap_or("").to_string()
}

fn log(line: &str) {
    let Some(dir) = std::env::var_os("TEMP") else {
        return;
    };
    let path = std::path::Path::new(&dir).join("wavemix-winosd.txt");
    // Keep the file from growing without bound.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 20_000) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}
