//! Checking GitHub for a newer Wavemix, and installing it.
//!
//! The newest release is read from the GitHub API; if it is newer than this build, the installer
//! attached to it can be downloaded and run silently. The installer replaces the old version in
//! place (keeping your settings), and the app is started again afterwards.

use std::sync::{Arc, Mutex};

use serde_json::Value;

const REPO: &str = "possiblypengu/wavemix";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.13.0", without the leading `v`.
    pub version: String,
    /// Where the installer can be downloaded from.
    pub installer_url: String,
    /// What's new, as written on the release.
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum State {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(String),
    /// The installer is on disk and ready to run.
    Ready(std::path::PathBuf),
    Failed(String),
}

pub type Shared = Arc<Mutex<State>>;

/// "v1.2.3" or "1.2.3-beta" to (1, 2, 3).
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Is `candidate` a later version than `current`?
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Read a GitHub "latest release" document. `None` if it has no installer attached.
pub fn parse_release(json: &str) -> Result<Option<Release>, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("unreadable reply: {e}"))?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or("the release has no version")?
        .trim_start_matches('v')
        .to_string();
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    let installer = v["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let name = a["name"].as_str()?;
            let url = a["browser_download_url"].as_str()?;
            (name.starts_with("wavemix-setup")
                && name.ends_with(".exe")
                && url.starts_with(&prefix))
            .then(|| url.to_string())
        })
        .next();
    Ok(installer.map(|installer_url| Release {
        version: tag,
        installer_url,
        notes: v["body"].as_str().unwrap_or_default().trim().to_string(),
    }))
}

fn set(shared: &Shared, state: State) {
    if let Ok(mut s) = shared.lock() {
        *s = state;
    }
}

/// Look for a newer release on a background thread; the result lands in `shared`. `wake` repaints
/// the UI when it does.
pub fn check(shared: &Shared, wake: impl Fn() + Send + 'static) {
    {
        let Ok(mut s) = shared.lock() else { return };
        if matches!(*s, State::Checking | State::Downloading(_)) {
            return;
        }
        *s = State::Checking;
    }
    let shared = shared.clone();
    std::thread::spawn(move || {
        let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let result = http::get(&url, 1 << 20)
            .and_then(|body| String::from_utf8(body).map_err(|e| e.to_string()))
            .and_then(|text| parse_release(&text));
        let state = match result {
            Ok(Some(r)) if is_newer(&r.version, env!("CARGO_PKG_VERSION")) => State::Available(r),
            Ok(_) => State::UpToDate,
            Err(e) => State::Failed(e),
        };
        set(&shared, state);
        wake();
    });
}

/// Download the installer for `release` into the temp folder.
pub fn download(shared: &Shared, release: Release, wake: impl Fn() + Send + 'static) {
    set(shared, State::Downloading(release.version.clone()));
    let shared = shared.clone();
    std::thread::spawn(move || {
        let path = std::env::temp_dir().join(format!("wavemix-setup-{}.exe", release.version));
        let result = http::get(&release.installer_url, 80 << 20).and_then(|bytes| {
            // A real installer is a few megabytes; anything tiny is an error page.
            if bytes.len() < 500_000 || !bytes.starts_with(b"MZ") {
                return Err("the download wasn't an installer".to_string());
            }
            std::fs::write(&path, bytes).map_err(|e| e.to_string())
        });
        set(
            &shared,
            match result {
                Ok(()) => State::Ready(path),
                Err(e) => State::Failed(e),
            },
        );
        wake();
    });
}

/// Run the downloaded installer silently, then start Wavemix again. The caller should quit
/// straight afterwards; the installer closes the old copy itself if it is still running.
#[cfg(windows)]
pub fn install_and_restart(installer: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // cmd runs the installer to completion, then relaunches whatever is installed.
    let script = format!(
        "\"{}\" /S & start \"\" \"{}\" --background",
        installer.display(),
        exe.display()
    );
    std::process::Command::new("cmd")
        .args(["/C", &script])
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
pub fn install_and_restart(_installer: &std::path::Path) -> Result<(), String> {
    Err("Updating needs Windows".into())
}

#[cfg(windows)]
mod http {
    //! A small blocking HTTPS GET on top of WinHTTP, so there is no TLS library to ship. It uses
    //! Windows' own certificate store and proxy settings.

    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Networking::WinHttp::*;

    struct Handle(*mut std::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn get(url: &str, limit: usize) -> Result<Vec<u8>, String> {
        let rest = url
            .strip_prefix("https://")
            .ok_or("only https addresses are used")?;
        let (host, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        unsafe {
            let agent = wide(concat!("Wavemix/", env!("CARGO_PKG_VERSION")));
            let session = Handle(WinHttpOpen(
                agent.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                null(),
                null(),
                0,
            ));
            if session.0.is_null() {
                return Err("couldn't start a connection".into());
            }
            let whost = wide(host);
            let connect = Handle(WinHttpConnect(session.0, whost.as_ptr(), 443, 0));
            if connect.0.is_null() {
                return Err(format!("couldn't reach {host}"));
            }
            let verb = wide("GET");
            let wpath = wide(path);
            let request = Handle(WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                wpath.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            ));
            if request.0.is_null() {
                return Err("couldn't build the request".into());
            }
            let accept = wide("Accept: application/vnd.github+json");
            WinHttpAddRequestHeaders(
                request.0,
                accept.as_ptr(),
                u32::MAX,
                WINHTTP_ADDREQ_FLAG_ADD,
            );
            if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
                || WinHttpReceiveResponse(request.0, null_mut()) == 0
            {
                return Err(format!("couldn't reach {host}"));
            }
            let mut status = 0u32;
            let mut size = std::mem::size_of::<u32>() as u32;
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                &mut status as *mut u32 as *mut _,
                &mut size,
                null_mut(),
            );
            if status == 404 {
                return Err("no release has been published yet".into());
            }
            if status != 200 {
                return Err(format!("the server answered {status}"));
            }
            let mut out = Vec::new();
            loop {
                let mut available = 0u32;
                if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                    return Err("the connection dropped".into());
                }
                if available == 0 {
                    break;
                }
                let mut chunk = vec![0u8; available as usize];
                let mut read = 0u32;
                if WinHttpReadData(
                    request.0,
                    chunk.as_mut_ptr() as *mut _,
                    available,
                    &mut read,
                ) == 0
                {
                    return Err("the connection dropped".into());
                }
                out.extend_from_slice(&chunk[..read as usize]);
                if out.len() > limit {
                    return Err("the reply was larger than expected".into());
                }
            }
            Ok(out)
        }
    }
}

#[cfg(not(windows))]
mod http {
    pub fn get(_url: &str, _limit: usize) -> Result<Vec<u8>, String> {
        Err("Updating needs Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.13.0", "0.12.1"));
        assert!(is_newer("v1.0.0", "0.99.9"));
        assert!(is_newer("0.12.10", "0.12.9"));
        assert!(!is_newer("0.12.1", "0.12.1"));
        assert!(!is_newer("0.12.0", "0.12.1"));
        assert!(!is_newer("junk", "0.12.1"));
        assert!(is_newer("0.13.0-beta", "0.12.1"));
    }

    #[test]
    fn a_release_document_yields_the_installer() {
        let json = r#"{
            "tag_name": "v0.13.0",
            "body": "Faster meters.\n",
            "assets": [
                {"name": "wavemix.exe",
                 "browser_download_url": "https://github.com/possiblypengu/wavemix/releases/download/v0.13.0/wavemix.exe"},
                {"name": "wavemix-setup-0.13.0.exe",
                 "browser_download_url": "https://github.com/possiblypengu/wavemix/releases/download/v0.13.0/wavemix-setup-0.13.0.exe"}
            ]
        }"#;
        let r = parse_release(json).unwrap().unwrap();
        assert_eq!(r.version, "0.13.0");
        assert!(r.installer_url.ends_with("wavemix-setup-0.13.0.exe"));
        assert_eq!(r.notes, "Faster meters.");
    }

    #[test]
    fn installers_from_other_places_are_ignored() {
        let json = r#"{"tag_name": "v9.9.9", "assets": [
            {"name": "wavemix-setup-9.9.9.exe", "browser_download_url": "https://evil.example/wavemix-setup-9.9.9.exe"}]}"#;
        assert_eq!(parse_release(json).unwrap(), None);
        assert!(parse_release("not json").is_err());
    }
}
