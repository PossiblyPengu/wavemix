//! The Windows engine, on top of Core Audio (WASAPI session and endpoint APIs).
//!
//! Everything COM lives on the engine thread. A full rescan enumerates devices and sessions and
//! caches their interface pointers; the ~30 Hz ticks in between only read volumes and peak meters
//! through those cached pointers, which is cheap.

// The IPolicyConfig method names are the Win32 ones, which are PascalCase.
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::{interface, Interface, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, AudioSessionStateActive,
    AudioSessionStateExpired, EDataFlow, ERole, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionEnumerator, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

use super::appinfo::{self, AppInfo};
use super::Engine;
use crate::model::{Command, Device, Flow, Session, Snapshot};

/// The undocumented-but-stable interface every "switch default audio device" tool uses. Only the
/// slot order matters for the vtable; the methods before `SetDefaultEndpoint` are placeholders.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: windows::core::IUnknown {
    unsafe fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
    unsafe fn GetDeviceFormat(
        &self,
        device: PCWSTR,
        default: i32,
        format: *mut *mut c_void,
    ) -> HRESULT;
    unsafe fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
    unsafe fn SetDeviceFormat(
        &self,
        device: PCWSTR,
        endpoint: *mut c_void,
        mix: *mut c_void,
    ) -> HRESULT;
    unsafe fn GetProcessingPeriod(
        &self,
        device: PCWSTR,
        default: i32,
        default_period: *mut i64,
        min_period: *mut i64,
    ) -> HRESULT;
    unsafe fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
    unsafe fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    unsafe fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    unsafe fn GetPropertyValue(
        &self,
        device: PCWSTR,
        fx_store: i32,
        key: *const c_void,
        value: *mut c_void,
    ) -> HRESULT;
    unsafe fn SetPropertyValue(
        &self,
        device: PCWSTR,
        fx_store: i32,
        key: *const c_void,
        value: *mut c_void,
    ) -> HRESULT;
    unsafe fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
    unsafe fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
}

/// CPolicyConfigClient
const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

struct DeviceHandle {
    info: Device,
    volume: IAudioEndpointVolume,
    meter: IAudioMeterInformation,
    /// The endpoint itself. The interfaces above were activated from it, and Windows can stop
    /// updating them once it goes away, so it lives as long as they do.
    _device: IMMDevice,
}

struct SessionHandle {
    info: Session,
    volume: ISimpleAudioVolume,
    meter: IAudioMeterInformation,
    control: IAudioSessionControl,
    /// The session list and manager the control came from; dropped too early, a session's meter
    /// can read zero forever.
    _list: IAudioSessionEnumerator,
    _manager: IAudioSessionManager2,
}

pub struct WasapiEngine {
    /// `None` when Windows' audio service can't be reached; the mixer then just shows nothing
    /// instead of taking the whole app down.
    enumerator: Option<IMMDeviceEnumerator>,
    devices: Vec<DeviceHandle>,
    sessions: Vec<SessionHandle>,
    /// pid -> who it is, so a rescan doesn't reopen every process and re-extract its icon.
    app_cache: HashMap<u32, AppInfo>,
    /// Meter reads that failed, and the last error: if levels sit at zero, this says why.
    meter_errors: u64,
    last_meter_error: Option<String>,
}

impl WasapiEngine {
    /// Must be called on the thread that will own the engine.
    pub fn new() -> Self {
        unsafe {
            // Already-initialised (S_FALSE) and changed-mode errors are both survivable.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: Option<IMMDeviceEnumerator> =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok();
            Self {
                enumerator,
                devices: Vec::new(),
                sessions: Vec::new(),
                app_cache: HashMap::new(),
                meter_errors: 0,
                last_meter_error: None,
            }
        }
    }

    fn rescan(&mut self) {
        // The audio service may start (or restart) after us; keep trying to reach it.
        if self.enumerator.is_none() {
            self.enumerator =
                unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() };
        }
        let Some(enumerator) = self.enumerator.clone() else {
            self.devices.clear();
            self.sessions.clear();
            return;
        };
        let mut devices = Vec::new();
        let mut sessions = Vec::new();
        for (flow, data_flow) in [(Flow::Render, eRender), (Flow::Capture, eCapture)] {
            let default_id = self.default_id(data_flow);
            let Some(list) = (unsafe {
                enumerator
                    .EnumAudioEndpoints(data_flow, DEVICE_STATE_ACTIVE)
                    .ok()
            }) else {
                continue;
            };
            let count = unsafe { list.GetCount() }.unwrap_or(0);
            for i in 0..count {
                let Ok(device) = (unsafe { list.Item(i) }) else {
                    continue;
                };
                let Some(handle) = open_device(&device, flow, default_id.as_deref()) else {
                    continue;
                };
                if flow == Flow::Render {
                    self.scan_sessions(&device, &handle.info.id, &mut sessions);
                }
                devices.push(handle);
            }
        }
        self.devices = devices;
        self.sessions = sessions;
    }

    fn default_id(&self, flow: EDataFlow) -> Option<String> {
        unsafe {
            let d = self
                .enumerator
                .as_ref()?
                .GetDefaultAudioEndpoint(flow, eConsole)
                .ok()?;
            take_pwstr(d.GetId().ok()?)
        }
    }

    fn scan_sessions(&mut self, device: &IMMDevice, device_id: &str, out: &mut Vec<SessionHandle>) {
        unsafe {
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                return;
            };
            let Ok(list) = manager.GetSessionEnumerator() else {
                return;
            };
            let count = list.GetCount().unwrap_or(0);
            for i in 0..count {
                let Ok(control) = list.GetSession(i) else {
                    continue;
                };
                if control.GetState().ok() == Some(AudioSessionStateExpired) {
                    continue;
                }
                let (Ok(control2), Ok(volume), Ok(meter)) = (
                    control.cast::<IAudioSessionControl2>(),
                    control.cast::<ISimpleAudioVolume>(),
                    control.cast::<IAudioMeterInformation>(),
                ) else {
                    continue;
                };
                let Some(id) = control2
                    .GetSessionInstanceIdentifier()
                    .ok()
                    .and_then(|p| take_pwstr(p))
                else {
                    continue;
                };
                let system = control2.IsSystemSoundsSession() == S_OK;
                let pid = control2.GetProcessId().unwrap_or(0);
                let app = if system || pid == 0 {
                    AppInfo::default()
                } else {
                    self.app_info(pid)
                };
                let exe = app.stem.clone();
                let display = control
                    .GetDisplayName()
                    .ok()
                    .and_then(|p| take_pwstr(p))
                    // "@%SystemRoot%\system32\...,-123" is an unresolved resource string.
                    .filter(|n| !n.is_empty() && !n.starts_with('@'));
                let name = if system {
                    "System sounds".to_string()
                } else {
                    display.or(app.name.clone()).unwrap_or_else(|| {
                        if exe.is_empty() {
                            format!("Process {pid}")
                        } else {
                            exe.clone()
                        }
                    })
                };
                out.push(SessionHandle {
                    info: Session {
                        id,
                        device_id: device_id.to_string(),
                        pid,
                        exe: if system { "system-sounds".into() } else { exe },
                        name,
                        icon: app.icon,
                        volume: volume.GetMasterVolume().unwrap_or(1.0),
                        muted: volume.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                        peak: 0.0,
                        active: control.GetState().ok() == Some(AudioSessionStateActive),
                    },
                    volume,
                    meter,
                    control,
                    _list: list.clone(),
                    _manager: manager.clone(),
                });
            }
        }
    }

    fn app_info(&mut self, pid: u32) -> AppInfo {
        self.app_cache
            .entry(pid)
            .or_insert_with(|| appinfo::lookup(pid))
            .clone()
    }

    fn refresh_levels(&mut self) {
        for d in &mut self.devices {
            unsafe {
                d.info.volume = d
                    .volume
                    .GetMasterVolumeLevelScalar()
                    .unwrap_or(d.info.volume);
                d.info.muted = d
                    .volume
                    .GetMute()
                    .map(|b| b.as_bool())
                    .unwrap_or(d.info.muted);
                d.info.peak =
                    read_peak(&d.meter, &mut self.meter_errors, &mut self.last_meter_error);
            }
        }
        for s in &mut self.sessions {
            unsafe {
                s.info.volume = s.volume.GetMasterVolume().unwrap_or(s.info.volume);
                s.info.muted = s
                    .volume
                    .GetMute()
                    .map(|b| b.as_bool())
                    .unwrap_or(s.info.muted);
                s.info.peak =
                    read_peak(&s.meter, &mut self.meter_errors, &mut self.last_meter_error);
                s.info.active = s.control.GetState().ok() == Some(AudioSessionStateActive);
            }
        }
    }

    fn set_default(&self, id: &str) {
        let wide: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let Ok(policy) =
                CoCreateInstance::<_, IPolicyConfig>(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)
            else {
                return;
            };
            for role in [eConsole, eMultimedia, eCommunications] {
                let _ = policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role);
            }
        }
    }
}

impl Default for WasapiEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine for WasapiEngine {
    fn diagnostics(&self) -> String {
        format!(
            "meter reads that failed: {}{}",
            self.meter_errors,
            self.last_meter_error
                .as_ref()
                .map(|e| format!(" (last error: {e})"))
                .unwrap_or_default()
        )
    }

    fn tick(&mut self, rescan: bool) -> Snapshot {
        if rescan {
            self.rescan();
        }
        self.refresh_levels();
        Snapshot {
            devices: self.devices.iter().map(|d| d.info.clone()).collect(),
            sessions: self.sessions.iter().map(|s| s.info.clone()).collect(),
        }
    }

    fn apply(&mut self, cmd: &Command) {
        unsafe {
            match cmd {
                Command::SetDeviceVolume { id, volume } => {
                    if let Some(d) = self.devices.iter().find(|d| &d.info.id == id) {
                        let _ = d
                            .volume
                            .SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), std::ptr::null());
                    }
                }
                Command::SetDeviceMute { id, muted } => {
                    if let Some(d) = self.devices.iter().find(|d| &d.info.id == id) {
                        let _ = d.volume.SetMute(*muted, std::ptr::null());
                    }
                }
                Command::SetSessionVolume {
                    device_id,
                    id,
                    volume,
                } => {
                    if let Some(s) = self.session(device_id, id) {
                        let _ = s
                            .volume
                            .SetMasterVolume(volume.clamp(0.0, 1.0), std::ptr::null());
                    }
                }
                Command::SetSessionMute {
                    device_id,
                    id,
                    muted,
                } => {
                    if let Some(s) = self.session(device_id, id) {
                        let _ = s.volume.SetMute(*muted, std::ptr::null());
                    }
                }
                Command::SetDefaultDevice { id, .. } => self.set_default(id),
                Command::SetAppDevice { pids, device_id } => {
                    super::routing::set_app_device(pids, device_id.as_deref());
                }
            }
        }
    }
}

impl WasapiEngine {
    fn session(&self, device_id: &str, id: &str) -> Option<&SessionHandle> {
        self.sessions
            .iter()
            .find(|s| s.info.device_id == device_id && s.info.id == id)
    }
}

/// A peak reading, with failures counted and remembered rather than quietly turned into silence.
unsafe fn read_peak(
    meter: &IAudioMeterInformation,
    errors: &mut u64,
    last: &mut Option<String>,
) -> f32 {
    match meter.GetPeakValue() {
        Ok(v) => v,
        Err(e) => {
            *errors += 1;
            *last = Some(e.to_string());
            0.0
        }
    }
}

fn open_device(device: &IMMDevice, flow: Flow, default_id: Option<&str>) -> Option<DeviceHandle> {
    unsafe {
        let id = take_pwstr(device.GetId().ok()?)?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        let meter: IAudioMeterInformation = device.Activate(CLSCTX_ALL, None).ok()?;
        let name = friendly_name(device).unwrap_or_else(|| id.clone());
        Some(DeviceHandle {
            info: Device {
                is_default: default_id == Some(id.as_str()),
                id,
                name,
                flow,
                volume: volume.GetMasterVolumeLevelScalar().unwrap_or(0.0),
                muted: volume.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                peak: 0.0,
            },
            volume,
            meter,
            _device: device.clone(),
        })
    }
}

fn friendly_name(device: &IMMDevice) -> Option<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        take_pwstr(PropVariantToStringAlloc(&value).ok()?)
    }
}

/// Read a COM-allocated wide string and free it.
unsafe fn take_pwstr(p: PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as *const c_void));
    s
}
