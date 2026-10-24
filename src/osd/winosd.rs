//! Keeps Windows' own volume popup from appearing on top of ours.
//!
//! There is no setting for it, so this watches for a short while after our bar is shown and hides
//! the popup window if Windows opens it. The filter is deliberately narrow, because the same
//! shell process owns the taskbar, Start and Quick Settings: only a small, topmost, non-taskbar
//! tool window of the classes the popup is known to use, owned by `explorer.exe`, is touched.
//!
//! What it sees is appended to `%TEMP%\wavemix-winosd.txt` (a few lines per run), so if a Windows
//! version uses a different window the log says which.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId,
    IsWindowVisible, ShowWindow, GWL_EXSTYLE, SW_HIDE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST,
};

/// Window classes the Windows 10 and 11 volume popups are drawn in.
const CLASSES: [&str; 2] = ["NativeHWNDHost", "XamlExplorerHostIslandWindow"];
/// The popup is small; anything bigger than this is some other piece of the shell.
const MAX_SIZE: (i32, i32) = (900, 400);
/// How long to keep watching after each request, and how often to look.
const WATCH: Duration = Duration::from_millis(900);
const TICK: Duration = Duration::from_millis(12);

static RUNNING: AtomicBool = AtomicBool::new(false);
static UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// Watch for, and hide, Windows' volume popup for the next moment. Cheap to call repeatedly.
pub fn hide_for_a_moment() {
    if let Ok(mut u) = UNTIL.lock() {
        *u = Some(Instant::now() + WATCH);
    }
    if RUNNING.swap(true, Ordering::AcqRel) {
        return; // already watching; the deadline above was extended
    }
    thread::spawn(|| {
        let mut logged = 0;
        loop {
            let until = UNTIL.lock().ok().and_then(|u| *u);
            match until {
                Some(t) if Instant::now() < t => {}
                _ => break,
            }
            unsafe { sweep(&mut logged) };
            thread::sleep(TICK);
        }
        RUNNING.store(false, Ordering::Release);
    });
}

unsafe fn sweep(logged: &mut u32) {
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
        let logged = &mut *(lparam as *mut u32);
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let class = class_name(hwnd);
        if !CLASSES.contains(&class.as_str()) {
            return 1;
        }
        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut rect);
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let exe = process_name(pid);
        let ours = pid == GetCurrentProcessId();
        let shaped = ex & WS_EX_TOPMOST != 0
            && ex & WS_EX_TOOLWINDOW != 0
            && ex & WS_EX_APPWINDOW == 0
            && w > 0
            && h > 0
            && w <= MAX_SIZE.0
            && h <= MAX_SIZE.1;
        let hide = shaped && !ours && exe.eq_ignore_ascii_case("explorer.exe");
        if *logged < 12 {
            *logged += 1;
            log(&format!(
                "{class} {w}x{h} at ({},{}) ex={ex:#x} exe={exe} -> {}",
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
    EnumWindows(Some(each), logged as *mut u32 as LPARAM);
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
