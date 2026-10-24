//! Small persisted settings, stored as JSON next to the user's other app data.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::params::{EngineParams, IoConfig};
use crate::preset::Preset;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Console,
    Compact,
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
            io: IoConfig {
                engine_on: true,
                ..IoConfig::default()
            },
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: EngineConfig::default(),
            view: View::default(),
            run_at_startup: false,
            hotkeys: Hotkeys::default(),
            volume_step: 5,
            experimental_routing: false,
            presets: Vec::new(),
            osd: true,
            osd_vertical: false,
            osd_opacity: 95,
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
        assert_eq!(c.view, View::Console);
        let c: Config = serde_json::from_str(r#"{"view":"compact","future":1}"#).unwrap();
        assert_eq!(c.view, View::Compact);
        assert_eq!(c.volume_step, 5);
        assert_eq!(c.hotkeys.mute, "ctrl+alt+m");
        assert!(c.osd);
        assert_eq!(c.osd_opacity, 95);
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
