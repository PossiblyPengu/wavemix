//! Small persisted settings, stored as JSON next to the user's other app data.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::params::{EngineParams, IoConfig};
use crate::preset::Preset;

/// The two modes of the app. The names in the file stay `compact` and `console` so saved settings
/// keep working.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    /// Simple mode: a small EarTrumpet-style volume control.
    #[default]
    Compact,
    /// Pro mode: the full mixing console.
    Console,
}

/// Where the on-screen volume display appears.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsdPosition {
    /// Beside the tray icon, popping out of it with a callout tail.
    #[default]
    AboveTray,
    /// Low and in the middle of the screen.
    Centre,
    /// Wherever the user dragged it.
    Custom,
}

/// Global hotkeys, as strings like `ctrl+alt+up`. Empty means unbound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub volume_up: String,
    pub volume_down: String,
    pub mute: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            volume_up: "ctrl+alt+up".into(),
            volume_down: "ctrl+alt+down".into(),
            mute: "ctrl+alt+m".into(),
        }
    }
}

/// The mixing engine's saved state: which devices, every strip and bus setting.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub io: IoConfig,
    pub params: EngineParams,
    /// Latency target per device link, in milliseconds.
    pub latency_ms: u32,
    /// False until the first run has picked sensible default devices.
    pub configured: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            // Off until the user first opens Pro mode, so a casual user's microphone isn't
            // opened for no reason.
            io: IoConfig::default(),
            params: EngineParams::default(),
            latency_ms: crate::engine::runtime::DEFAULT_LATENCY_MS,
            configured: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub engine: EngineConfig,
    pub view: View,
    /// Pro mode: show the per-app list beside the console.
    pub pro_apps_panel: bool,
    pub run_at_startup: bool,
    pub hotkeys: Hotkeys,
    /// Hotkey volume step, in percentage points.
    pub volume_step: u32,
    /// Per-app output routing uses an undocumented Windows interface, so it is opt-in.
    pub experimental_routing: bool,
    pub presets: Vec<Preset>,
    /// Show the on-screen volume display when a hotkey changes the volume.
    pub osd: bool,
    pub osd_vertical: bool,
    /// OSD opacity, percent.
    pub osd_opacity: u32,
    pub osd_position: OsdPosition,
    /// Where the OSD was dragged to (used when `osd_position` is `Custom`).
    pub osd_x: i32,
    pub osd_y: i32,
    /// How long the OSD stays up after the last change, in milliseconds.
    pub osd_timeout_ms: u32,
    /// Draw the OSD in the Windows accent colour.
    pub osd_accent_system: bool,
    /// Hide Windows' own volume popup whenever the wavemix bar is shown.
    pub hide_windows_osd: bool,
    /// Offer to bring a Voicemeeter setup over, until the user does or says no.
    pub voicemeeter_offer: bool,
    /// Windows' default playback device from before "Route Windows audio" moved it to a virtual
    /// cable, so it can be put back.
    pub restore_playback: Option<String>,
    /// Route Windows audio through the mixer whenever the engine is on.
    pub auto_route: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: EngineConfig::default(),
            view: View::default(),
            pro_apps_panel: false,
            run_at_startup: false,
            hotkeys: Hotkeys::default(),
            volume_step: 5,
            experimental_routing: false,
            presets: Vec::new(),
            osd: true,
            osd_vertical: false,
            osd_opacity: 95,
            osd_position: OsdPosition::default(),
            osd_x: 0,
            osd_y: 0,
            osd_timeout_ms: 1500,
            osd_accent_system: true,
            hide_windows_osd: true,
            voicemeeter_offer: true,
            restore_playback: None,
            auto_route: false,
        }
    }
}

fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("wavemix").join("config.json"))
}

impl Config {
    /// Missing or unreadable config falls back to defaults; a bad file must never block startup.
    pub fn load() -> Self {
        path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(p) = path() else { return };
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(p, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_missing_fields_use_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.view, View::Compact, "Simple mode is the default");
        assert!(
            !c.engine.io.engine_on,
            "engine stays off until Pro is opened"
        );
        let c: Config = serde_json::from_str(r#"{"view":"compact","future":1}"#).unwrap();
        assert_eq!(c.view, View::Compact);
        let c: Config = serde_json::from_str(r#"{"view":"console"}"#).unwrap();
        assert_eq!(c.view, View::Console);
        assert_eq!(c.volume_step, 5);
        assert_eq!(c.hotkeys.mute, "ctrl+alt+m");
        assert!(c.osd);
        assert_eq!(c.osd_opacity, 95);
        assert_eq!(c.osd_position, OsdPosition::AboveTray);
        assert_eq!(c.osd_timeout_ms, 1500);
    }

    #[test]
    fn presets_round_trip() {
        let mut c = Config::default();
        c.presets.push(Preset {
            name: "Call".into(),
            devices: vec![],
            apps: vec![],
        });
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.presets, c.presets);
    }
}
