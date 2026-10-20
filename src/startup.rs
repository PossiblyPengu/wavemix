//! "Run at startup": a value under the per-user Run key, so no admin rights and nothing outside
//! the user's own profile.

use std::io;

use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "wavemix";

/// Launch at login straight into the tray (`--background`) instead of popping a window up.
pub fn set_enabled(on: bool) -> io::Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
    if on {
        let exe = std::env::current_exe()?;
        key.set_value(VALUE, &format!("\"{}\" --background", exe.display()))
    } else {
        match key.delete_value(VALUE) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}
