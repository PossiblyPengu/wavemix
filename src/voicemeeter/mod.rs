//! Moving a Voicemeeter user over to wavemix: read their settings, translate them, and take
//! Voicemeeter out of the way. `import` is pure and tested everywhere; `remote` talks to the
//! running Voicemeeter and only exists on Windows.

pub mod import;
#[cfg(windows)]
pub mod remote;

pub use import::{plan_import, VmState};
