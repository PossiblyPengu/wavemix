//! The Windows engine, on top of Core Audio (WASAPI session and endpoint APIs).
//!
//! Everything COM lives on the engine thread. A full rescan enumerates devices and sessions and
//! caches their interface pointers; the ~30 Hz ticks in between only read volumes and peak meters
//! through those cached pointers, which is cheap.

// The IPolicyConfig method names are the Win32 ones, which are PascalCase.
#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::Path;

use windows::core::{interface, Interface, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, S_OK};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, AudioSessionStateActive,
    AudioSessionStateExpired, EDataFlow, ERole, IAudioSessionControl, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

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
}

struct SessionHandle {
    info: Session,
    volume: ISimpleAudioVolume,
    meter: IAudioMeterInformation,
    control: IAudioSessionControl,
}

pub struct WasapiEngine {
    enumerator: IMMDeviceEnumerator,
    devices: Vec<DeviceHandle>,
    sessions: Vec<SessionHandle>,
    /// pid -> executable stem, so a rescan doesn't reopen every process.
    exe_cache: HashMap<u32, String>,
}

impl WasapiEngine {
    /// Must be called on the thread that will own the engine.
    pub fn new() -> Self {
        unsafe {
            // Already-initialised (S_FALSE) and changed-mode errors are both survivable.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .expect("create the audio device enumerator");
            Self {
                enumerator,
                devices: Vec::new(),
                sessions: Vec::new(),
                exe_cache: HashMap::new(),
            }
        }
    }

    fn rescan(&mut self) {
        let mut devices = Vec::new();
        let mut sessions = Vec::new();
        for (flow, data_flow) in [(Flow::Render, eRender), (Flow::Capture, eCapture)] {
            let default_id = self.default_id(data_flow);
            let Some(list) = (unsafe {
                self.enumerator
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
                let exe = if system || pid == 0 {
                    String::new()
                } else {
                    self.exe_name(pid)
                };
                let display = control
                    .GetDisplayName()
                    .ok()
                    .and_then(|p| take_pwstr(p))
                    // "@%SystemRoot%\system32\...,-123" is an unresolved resource string.
                    .filter(|n| !n.is_empty() && !n.starts_with('@'));
                let name = if system {
                    "System sounds".to_string()
                } else {
                    display.unwrap_or_else(|| {
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
                        volume: volume.GetMasterVolume().unwrap_or(1.0),
                        muted: volume.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                        peak: 0.0,
                        active: control.GetState().ok() == Some(AudioSessionStateActive),
                    },
                    volume,
                    meter,
                    control,
                });
            }
        }
    }

    fn exe_name(&mut self, pid: u32) -> String {
        self.exe_cache
            .entry(pid)
            .or_insert_with(|| process_stem(pid).unwrap_or_default())
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
                d.info.peak = d.meter.GetPeakValue().unwrap_or(0.0);
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
                s.info.peak = s.meter.GetPeakValue().unwrap_or(0.0);
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

fn process_stem(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
    }
}
