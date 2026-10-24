//! Talking to a running Voicemeeter through its Remote API (`VoicemeeterRemote64.dll`), loaded at
//! run time so wavemix has no dependency on it. Ported from voicemeeter-osd's `vmr.rs`.

use std::ffi::CString;
use std::mem::transmute;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WRITE};
use winreg::RegKey;

use super::import::{Edition, VmBus, VmState, VmStrip};

type FnLogin = unsafe extern "system" fn() -> i32;
type FnLogout = unsafe extern "system" fn() -> i32;
type FnIsParametersDirty = unsafe extern "system" fn() -> i32;
type FnGetParameterFloat = unsafe extern "system" fn(*const i8, *mut f32) -> i32;
type FnSetParameterFloat = unsafe extern "system" fn(*const i8, f32) -> i32;
type FnGetParameterStringW = unsafe extern "system" fn(*const i8, *mut u16) -> i32;
type FnGetVoicemeeterType = unsafe extern "system" fn(*mut i32) -> i32;

const UNINSTALL_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\VB:Voicemeeter {17359A74-1236-5467}";
const DEFAULT_DLL: &str = r"C:\Program Files (x86)\VB\Voicemeeter\VoicemeeterRemote64.dll";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// Where Voicemeeter's Remote API library is, if Voicemeeter is installed. The uninstaller entry
/// is the only reliable pointer to the install directory; all editions share one key.
pub fn dll_path() -> Option<String> {
    let from_registry = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(UNINSTALL_KEY, KEY_READ | KEY_WOW64_32KEY)
        .ok()
        .and_then(|k| k.get_value::<String, _>("UninstallString").ok())
        .and_then(|raw| {
            // May be wrapped in quotes, optionally with trailing switches.
            let exe = raw
                .trim()
                .trim_start_matches('"')
                .split('"')
                .next()?
                .trim_end()
                .to_string();
            let dir = exe.rfind('\\').map(|i| exe[..i].to_string())?;
            Some(format!("{dir}\\VoicemeeterRemote64.dll"))
        })
        .filter(|p| Path::new(p).exists());
    from_registry.or_else(|| {
        Path::new(DEFAULT_DLL)
            .exists()
            .then(|| DEFAULT_DLL.to_string())
    })
}

pub fn installed() -> bool {
    dll_path().is_some()
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub struct Remote {
    module: HMODULE,
    login: FnLogin,
    logout: FnLogout,
    is_dirty: FnIsParametersDirty,
    get_float: FnGetParameterFloat,
    set_float: FnSetParameterFloat,
    get_string: FnGetParameterStringW,
    get_type: FnGetVoicemeeterType,
}

impl Remote {
    /// Load the API and log in. Fails if Voicemeeter isn't installed.
    pub fn connect() -> Result<Self, String> {
        let path = dll_path().ok_or("Voicemeeter doesn't seem to be installed")?;
        let wpath = wide(&path);
        let module = unsafe { LoadLibraryW(wpath.as_ptr()) };
        if module.is_null() {
            return Err(format!("couldn't load {path}"));
        }
        macro_rules! proc {
            ($name:literal, $ty:ty) => {{
                let cname = CString::new($name).unwrap();
                match unsafe { GetProcAddress(module, cname.as_ptr() as *const u8) } {
                    Some(f) => unsafe { transmute::<unsafe extern "system" fn() -> isize, $ty>(f) },
                    None => {
                        unsafe { FreeLibrary(module) };
                        return Err(format!("Voicemeeter's API is missing {}", $name));
                    }
                }
            }};
        }
        let remote = Remote {
            module,
            login: proc!("VBVMR_Login", FnLogin),
            logout: proc!("VBVMR_Logout", FnLogout),
            is_dirty: proc!("VBVMR_IsParametersDirty", FnIsParametersDirty),
            get_float: proc!("VBVMR_GetParameterFloat", FnGetParameterFloat),
            set_float: proc!("VBVMR_SetParameterFloat", FnSetParameterFloat),
            get_string: proc!("VBVMR_GetParameterStringW", FnGetParameterStringW),
            get_type: proc!("VBVMR_GetVoicemeeterType", FnGetVoicemeeterType),
        };
        // 0 = ok, 1 = ok but the Voicemeeter app isn't running (yet).
        let rc = unsafe { (remote.login)() };
        if rc != 0 && rc != 1 {
            return Err(format!("couldn't log in to Voicemeeter (code {rc})"));
        }
        Ok(remote)
    }

    /// The running edition, or `None` if Voicemeeter's app isn't running.
    fn edition(&self) -> Option<Edition> {
        let mut kind = 0i32;
        unsafe {
            if (self.get_type)(&mut kind) != 0 {
                return None;
            }
        }
        match kind {
            1 => Some(Edition::Standard),
            2 => Some(Edition::Banana),
            3 | 6 => Some(Edition::Potato),
            _ => None,
        }
    }

    fn float(&self, param: &str) -> Option<f32> {
        let c = CString::new(param).ok()?;
        let mut v = 0.0f32;
        (unsafe { (self.get_float)(c.as_ptr(), &mut v) } == 0).then_some(v)
    }

    fn flag(&self, param: &str) -> bool {
        self.float(param).is_some_and(|v| v >= 0.5)
    }

    fn string(&self, param: &str) -> Option<String> {
        let c = CString::new(param).ok()?;
        let mut buf = [0u16; 512];
        if unsafe { (self.get_string)(c.as_ptr(), buf.as_mut_ptr()) } != 0 {
            return None;
        }
        let len = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..len]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// Read the whole configuration. Voicemeeter's app must be running.
    pub fn read_state(&self) -> Result<VmState, String> {
        // The API serves cached values that refresh on `IsParametersDirty`, and the app can take
        // a moment to answer right after login.
        let mut edition = None;
        for _ in 0..20 {
            unsafe { (self.is_dirty)() };
            edition = self.edition();
            if edition.is_some() {
                break;
            }
            sleep(Duration::from_millis(60));
        }
        let edition = edition.ok_or("Voicemeeter isn't running. Start it, then try again.")?;
        unsafe { (self.is_dirty)() };

        // (strips, first virtual strip, A buses, B buses)
        let (n_strips, first_virtual, n_a, n_b) = match edition {
            Edition::Standard => (3, 2, 1, 1),
            Edition::Banana => (5, 3, 3, 2),
            Edition::Potato => (8, 5, 5, 3),
        };

        let strips = (0..n_strips)
            .map(|i| {
                let p = |name: &str| format!("Strip[{i}].{name}");
                let is_virtual = i >= first_virtual;
                VmStrip {
                    label: self.string(&p("Label")).unwrap_or_default(),
                    is_virtual,
                    device: if is_virtual {
                        None
                    } else {
                        self.string(&p("device.name"))
                    },
                    gain_db: self.float(&p("Gain")).unwrap_or(0.0),
                    mute: self.flag(&p("Mute")),
                    solo: self.flag(&p("Solo")),
                    mono: self.flag(&p("Mono")),
                    pan_x: self.float(&p("Pan_x")).unwrap_or(0.0),
                    sends_a: (1..=n_a).map(|k| self.flag(&p(&format!("A{k}")))).collect(),
                    sends_b: (1..=n_b).map(|k| self.flag(&p(&format!("B{k}")))).collect(),
                    eq: [
                        self.float(&p("EQGain1")).unwrap_or(0.0),
                        self.float(&p("EQGain2")).unwrap_or(0.0),
                        self.float(&p("EQGain3")).unwrap_or(0.0),
                    ],
                    comp: self.float(&p("Comp")).unwrap_or(0.0),
                    gate: self.float(&p("Gate")).unwrap_or(0.0),
                }
            })
            .collect();

        let buses = (0..(n_a + n_b))
            .map(|i| {
                let p = |name: &str| format!("Bus[{i}].{name}");
                let is_b = i >= n_a;
                VmBus {
                    label: self.string(&p("Label")).unwrap_or_default(),
                    is_b,
                    // B buses are virtual outputs; only the hardware A buses name a device.
                    device: if is_b {
                        None
                    } else {
                        self.string(&p("device.name"))
                    },
                    gain_db: self.float(&p("Gain")).unwrap_or(0.0),
                    mute: self.flag(&p("Mute")),
                }
            })
            .collect();

        Ok(VmState {
            edition,
            strips,
            buses,
        })
    }

    /// Ask Voicemeeter to quit.
    pub fn shutdown(&self) -> bool {
        let c = CString::new("Command.Shutdown").unwrap();
        unsafe { (self.set_float)(c.as_ptr(), 1.0) == 0 }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        unsafe {
            (self.logout)();
            FreeLibrary(self.module);
        }
    }
}

/// Stop Voicemeeter starting with Windows: remove per-user startup entries that mention it.
/// Returns the names removed. Only touches entries naming Voicemeeter, never anything else.
pub fn disable_autostart() -> Vec<String> {
    let Ok(key) =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE)
    else {
        return Vec::new();
    };
    let names: Vec<String> = key
        .enum_values()
        .filter_map(|v| v.ok())
        .map(|(n, _)| n)
        .collect();
    let mut removed = Vec::new();
    for name in names {
        let data = key.get_value::<String, _>(&name).unwrap_or_default();
        let mentions = |s: &str| s.to_lowercase().contains("voicemeeter");
        if (mentions(&name) || mentions(&data)) && key.delete_value(&name).is_ok() {
            removed.push(name);
        }
    }
    removed
}
