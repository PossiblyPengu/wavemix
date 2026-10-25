//! Small persisted settings, stored as JSON next to the user's other app data.

use std::collections::BTreeMap;
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

/// Light, dark, or follow Windows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
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
    /// Mute or unmute the default microphone.
    pub mic_mute: String,
    /// Switch to the next / previous output (speakers, headphones, ...).
    pub next_output: String,
    pub prev_output: String,
    /// Volume and mute of the app in the front window.
    pub app_up: String,
    pub app_down: String,
    pub app_mute: String,
    /// Show or hide Wavemix.
    pub open: String,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            volume_up: "ctrl+alt+up".into(),
            volume_down: "ctrl+alt+down".into(),
            mute: "ctrl+alt+m".into(),
            mic_mute: String::new(),
            next_output: String::new(),
            prev_output: String::new(),
            app_up: String::new(),
            app_down: String::new(),
            app_mute: String::new(),
            open: String::new(),
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Hotkeys {
    /// Every binding with its label, in the order of [`crate::hotkeys::ACTIONS`].
    pub fn entries(&self) -> [(&'static str, &str); 10] {
        [
            ("Volume up", &self.volume_up),
            ("Volume down", &self.volume_down),
            ("Mute", &self.mute),
            ("Mute microphone", &self.mic_mute),
            ("Next output", &self.next_output),
            ("Previous output", &self.prev_output),
            ("Front app louder", &self.app_up),
            ("Front app quieter", &self.app_down),
            ("Front app mute", &self.app_mute),
            ("Show / hide Wavemix", &self.open),
        ]
    }

    pub fn entries_mut(&mut self) -> [(&'static str, &mut String); 10] {
        [
            ("Volume up", &mut self.volume_up),
            ("Volume down", &mut self.volume_down),
            ("Mute", &mut self.mute),
            ("Mute microphone", &mut self.mic_mute),
            ("Next output", &mut self.next_output),
            ("Previous output", &mut self.prev_output),
            ("Front app louder", &mut self.app_up),
            ("Front app quieter", &mut self.app_down),
            ("Front app mute", &mut self.app_mute),
            ("Show / hide Wavemix", &mut self.open),
        ]
    }

    /// The keyboard's own volume keys for the main volume, and Ctrl plus them for the app in front.
    /// Windows' own volume popup doesn't appear for keys Wavemix has claimed.
    pub fn volume_keys(&self) -> Self {
        Self {
            volume_up: "AudioVolumeUp".into(),
            volume_down: "AudioVolumeDown".into(),
            mute: "AudioVolumeMute".into(),
            app_up: "ctrl+AudioVolumeUp".into(),
            app_down: "ctrl+AudioVolumeDown".into(),
            app_mute: "ctrl+AudioVolumeMute".into(),
            ..self.clone()
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
    pub theme: ThemeMode,
    /// Look for new releases now and then.
    pub check_updates: bool,
    /// Interface zoom: 1.0 is normal.
    pub ui_scale: f32,
    /// A version the user said "Later" to, so its banner stays away.
    pub skipped_update: String,
    /// Names you gave devices, keyed by Windows' stable device id (so twins and driver renames
    /// don't mix them up).
    pub device_names: BTreeMap<String, String>,
    /// Names you gave mixer strips and buses; an empty entry means the default ("Input 1", "A1").
    pub strip_names: Vec<String>,
    pub bus_names: Vec<String>,
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
            theme: ThemeMode::System,
            check_updates: true,
            ui_scale: 1.0,
            skipped_update: String::new(),
            device_names: BTreeMap::new(),
            strip_names: Vec::new(),
            bus_names: Vec::new(),
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
        let Some(p) = path() else {
            return Self::default();
        };
        let Ok(text) = fs::read_to_string(&p) else {
            return Self::default();
        };
        match serde_json::from_str(&text) {
            Ok(config) => config,
            Err(_) => {
                // Unreadable settings: keep a copy before the defaults overwrite them, so a stray
                // edit or a half-written file doesn't silently erase everything.
                let _ = fs::copy(&p, p.with_extension("json.bad"));
                Self::default()
            }
        }
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
            trigger: crate::preset::Trigger::AppPlays("game".into()),
        });
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.presets, c.presets);
    }
}

impl Config {
    /// What to call a device: the name you gave it, if any.
    pub fn device_alias(&self, device_id: &str) -> Option<&str> {
        self.device_names
            .get(device_id)
            .map(String::as_str)
            .filter(|n| !n.trim().is_empty())
    }

    pub fn strip_title(&self, i: usize) -> String {
        match self.strip_names.get(i).map(|n| n.trim()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => crate::engine::params::strip_name(i),
        }
    }

    pub fn bus_title(&self, i: usize) -> String {
        match self.bus_names.get(i).map(|n| n.trim()) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => crate::engine::params::bus_name(i).to_string(),
        }
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;

    #[test]
    fn custom_names_win_and_blank_ones_fall_back() {
        let mut c = Config::default();
        assert_eq!(c.strip_title(0), "Input 1");
        assert_eq!(c.bus_title(0), "A1");
        c.strip_names = vec!["Mic".into(), "  ".into()];
        c.bus_names = vec!["Desk".into()];
        assert_eq!(c.strip_title(0), "Mic");
        assert_eq!(c.strip_title(1), "Input 2");
        assert_eq!(c.bus_title(0), "Desk");
        assert_eq!(c.bus_title(1), "A2");
        c.device_names
            .insert("{0.0.0}.{abc}".into(), "Desk speakers".into());
        assert_eq!(c.device_alias("{0.0.0}.{abc}"), Some("Desk speakers"));
        assert_eq!(c.device_alias("Other"), None);
    }
}
