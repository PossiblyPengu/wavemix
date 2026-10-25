//! One Wavemix at a time. Two copies would both claim the tray, the hotkeys and the audio
//! devices and fight over them, which happens easily (a login launch plus a Start-menu click, or
//! the updater restarting the app). The second copy asks the first to open its window and exits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    INFINITE,
};

const MUTEX_NAME: &str = "Local\\Wavemix.Instance";
const EVENT_NAME: &str = "Local\\Wavemix.Open";

static OPEN_REQUESTED: AtomicBool = AtomicBool::new(false);
static WAKE: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Claim the right to be *the* Wavemix. Returns `false` if another copy already runs (after
/// telling it to show itself); the caller should then exit.
pub fn claim() -> bool {
    unsafe {
        // Read the error straight after the call: anything in between (freeing the name, say)
        // could replace it.
        let name = wide(MUTEX_NAME);
        let mutex = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        let already = GetLastError() == ERROR_ALREADY_EXISTS;
        if mutex.is_null() {
            return true; // can't tell; carry on rather than refuse to start
        }
        if already {
            let ev = OpenEventW(EVENT_MODIFY_STATE, 0, wide(EVENT_NAME).as_ptr());
            if !ev.is_null() {
                SetEvent(ev);
            }
            return false;
        }
        // Held for the life of the process (the OS releases it on exit).
        let event = CreateEventW(std::ptr::null(), 0, 0, wide(EVENT_NAME).as_ptr());
        if !event.is_null() {
            let handle = event as usize;
            std::thread::spawn(move || loop {
                if WaitForSingleObject(handle as HANDLE, INFINITE) != 0 {
                    break;
                }
                OPEN_REQUESTED.store(true, Ordering::Release);
                if let Ok(wake) = WAKE.lock() {
                    if let Some(f) = wake.as_ref() {
                        f();
                    }
                }
            });
        }
        true
    }
}

/// Called once the UI exists, so a request from a second copy can wake it.
pub fn on_open_request(wake: impl Fn() + Send + 'static) {
    if let Ok(mut w) = WAKE.lock() {
        *w = Some(Box::new(wake));
    }
}

/// Has a second copy asked us to show the window? Clears the request.
pub fn take_open_request() -> bool {
    OPEN_REQUESTED.swap(false, Ordering::AcqRel)
}
