//! Per-app output routing: "send Spotify to the headphones". This is what Settings > Sound > Volume
//! mixer does, through an undocumented WinRT factory. The interface is the Windows 10 21H2 / Windows
//! 11 layout; on an older build the factory refuses the interface ID and nothing happens.
//!
//! EXPERIMENTAL: the vtable layout below comes from reverse-engineered documentation, so this is
//! gated behind a setting and every failure is swallowed rather than surfaced.
#![allow(non_snake_case)]

use std::ffi::c_void;

use windows::core::{interface, HRESULT, HSTRING};
use windows::Win32::Media::Audio::{eConsole, eMultimedia, eRender, EDataFlow, ERole};
use windows::Win32::System::WinRT::RoGetActivationFactory;

/// Only the order of the vtable matters; the 3 IInspectable slots and the 19 methods before `SetPersistedDefaultAudioEndpoint`
/// (volume-group and chat-app plumbing) are placeholders that are never called.
#[interface("ab3d4648-e242-459f-b02f-541c70306324")]
unsafe trait IAudioPolicyConfig: windows::core::IUnknown {
    // IInspectable's three slots: GetIids, GetRuntimeClassName, GetTrustLevel.
    unsafe fn GetIids(&self) -> HRESULT;
    unsafe fn GetRuntimeClassName(&self) -> HRESULT;
    unsafe fn GetTrustLevel(&self) -> HRESULT;
    unsafe fn Unused0(&self) -> HRESULT;
    unsafe fn Unused1(&self) -> HRESULT;
    unsafe fn Unused2(&self) -> HRESULT;
    unsafe fn Unused3(&self) -> HRESULT;
    unsafe fn Unused4(&self) -> HRESULT;
    unsafe fn Unused5(&self) -> HRESULT;
    unsafe fn Unused6(&self) -> HRESULT;
    unsafe fn Unused7(&self) -> HRESULT;
    unsafe fn Unused8(&self) -> HRESULT;
    unsafe fn Unused9(&self) -> HRESULT;
    unsafe fn Unused10(&self) -> HRESULT;
    unsafe fn Unused11(&self) -> HRESULT;
    unsafe fn Unused12(&self) -> HRESULT;
    unsafe fn Unused13(&self) -> HRESULT;
    unsafe fn Unused14(&self) -> HRESULT;
    unsafe fn Unused15(&self) -> HRESULT;
    unsafe fn Unused16(&self) -> HRESULT;
    unsafe fn Unused17(&self) -> HRESULT;
    unsafe fn Unused18(&self) -> HRESULT;
    unsafe fn SetPersistedDefaultAudioEndpoint(
        &self,
        process_id: u32,
        flow: EDataFlow,
        role: ERole,
        device_id: *mut c_void,
    ) -> HRESULT;
    unsafe fn GetPersistedDefaultAudioEndpoint(
        &self,
        process_id: u32,
        flow: EDataFlow,
        role: ERole,
        device_id: *mut *mut c_void,
    ) -> HRESULT;
    unsafe fn ClearAllPersistedApplicationDefaultEndpoints(&self) -> HRESULT;
}

/// Convert an `IMMDevice` id (`{0.0.0.00000000}.{guid}`) into the interface path this API wants.
fn device_path(id: &str, capture: bool) -> String {
    let class = if capture {
        "{2eef81be-33fa-4800-9670-1cd474972c3f}"
    } else {
        "{e6327cad-dcec-4949-ae8a-991e976a79d4}"
    };
    format!(r"\\?\SWD#MMDEVAPI#{id}#{class}")
}

/// Route every listed process's playback to `device_id`, or back to the Windows default when
/// `None`. Returns whether the factory was reachable and every call succeeded.
pub fn set_app_device(pids: &[u32], device_id: Option<&str>) -> bool {
    unsafe {
        let Ok(policy) = RoGetActivationFactory::<IAudioPolicyConfig>(&HSTRING::from(
            "Windows.Media.Internal.AudioPolicyConfig",
        )) else {
            return false;
        };
        // An empty HSTRING is a null pointer, which clears the override.
        let hs = device_id
            .map(|id| HSTRING::from(device_path(id, false)))
            .unwrap_or_default();
        let raw: *mut c_void = std::mem::transmute_copy(&hs);
        let mut ok = true;
        for &pid in pids {
            for role in [eConsole, eMultimedia] {
                ok &= policy
                    .SetPersistedDefaultAudioEndpoint(pid, eRender, role, raw)
                    .is_ok();
            }
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_match_the_mmdevapi_form() {
        assert_eq!(
            device_path("{0.0.0.00000000}.{abc}", false),
            r"\\?\SWD#MMDEVAPI#{0.0.0.00000000}.{abc}#{e6327cad-dcec-4949-ae8a-991e976a79d4}"
        );
        assert!(device_path("x", true).ends_with("{2eef81be-33fa-4800-9670-1cd474972c3f}"));
    }
}
