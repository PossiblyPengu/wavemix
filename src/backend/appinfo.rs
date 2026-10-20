//! Who is behind an audio session: executable name, a human-readable name from the file's version
//! resource ("Google Chrome" rather than "chrome"), and the app's icon.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::sync::Arc;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Shell::ExtractIconExW;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

use crate::model::IconData;

#[derive(Clone, Default)]
pub struct AppInfo {
    /// Executable file name without extension; empty if the process couldn't be opened.
    pub stem: String,
    /// `FileDescription` from the exe's version resource, if it has one.
    pub name: Option<String>,
    pub icon: Option<Arc<IconData>>,
}

pub fn lookup(pid: u32) -> AppInfo {
    let Some(path) = process_path(pid) else {
        return AppInfo::default();
    };
    let stem = Path::new(&path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let wide = wide(&path);
    AppInfo {
        stem,
        name: unsafe { file_description(&wide) },
        icon: unsafe { extract_icon(&wide) }.map(Arc::new),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn process_path(pid: u32) -> Option<String> {
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
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// `path` is NUL-terminated UTF-16.
unsafe fn file_description(path: &[u16]) -> Option<String> {
    let path = PCWSTR(path.as_ptr());
    let size = GetFileVersionInfoSizeW(path, None);
    if size == 0 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    GetFileVersionInfoW(path, None, size, block.as_mut_ptr() as *mut c_void).ok()?;

    // The language/codepage pair names the string table to read.
    let mut ptr: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    let translation = wide("\\VarFileInfo\\Translation");
    if !VerQueryValueW(
        block.as_ptr() as *const c_void,
        PCWSTR(translation.as_ptr()),
        &mut ptr,
        &mut len,
    )
    .as_bool()
        || ptr.is_null()
        || len < 4
    {
        return None;
    }
    let pair = std::slice::from_raw_parts(ptr as *const u16, 2);
    let key = wide(&format!(
        "\\StringFileInfo\\{:04x}{:04x}\\FileDescription",
        pair[0], pair[1]
    ));
    let mut text: *mut c_void = std::ptr::null_mut();
    let mut text_len = 0u32;
    if !VerQueryValueW(
        block.as_ptr() as *const c_void,
        PCWSTR(key.as_ptr()),
        &mut text,
        &mut text_len,
    )
    .as_bool()
        || text.is_null()
        || text_len == 0
    {
        return None;
    }
    let chars = std::slice::from_raw_parts(text as *const u16, text_len as usize);
    let s = String::from_utf16_lossy(chars)
        .trim_end_matches('\0')
        .trim()
        .to_string();
    (!s.is_empty()).then_some(s)
}

unsafe fn extract_icon(path: &[u16]) -> Option<IconData> {
    let mut large = HICON::default();
    let n = ExtractIconExW(PCWSTR(path.as_ptr()), 0, Some(&mut large), None, 1);
    if n == 0 || large.is_invalid() {
        return None;
    }
    let data = icon_to_rgba(large);
    let _ = DestroyIcon(large);
    data
}

unsafe fn icon_to_rgba(icon: HICON) -> Option<IconData> {
    let mut info = ICONINFO::default();
    GetIconInfo(icon, &mut info).ok()?;
    let data = (|| {
        if info.hbmColor.is_invalid() {
            return None;
        }
        let mut bm = BITMAP::default();
        let got = GetObjectW(
            info.hbmColor.into(),
            size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut c_void),
        );
        if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative: top-down rows
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(
            dc,
            info.hbmColor,
            0,
            h as u32,
            Some(px.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        if lines == 0 {
            return None;
        }
        // BGRA -> RGBA. Old icons carry no alpha channel at all; treat those as opaque.
        let any_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
        for p in px.chunks_exact_mut(4) {
            p.swap(0, 2);
            if !any_alpha {
                p[3] = 255;
            }
        }
        Some(IconData {
            width: w as usize,
            height: h as usize,
            rgba: px,
        })
    })();
    let _ = DeleteObject(info.hbmColor.into());
    let _ = DeleteObject(info.hbmMask.into());
    data
}
