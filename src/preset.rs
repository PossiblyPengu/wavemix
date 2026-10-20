//! Presets: a saved set of volumes and mutes that can be re-applied in one click. Pure logic over
//! snapshots, so it is unit-tested without any audio system.

use serde::{Deserialize, Serialize};

use crate::model::{group_sessions, Command, Snapshot};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DevicePreset {
    pub id: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppPreset {
    /// Lower-case executable stem, the same key sessions are grouped by.
    pub key: String,
    pub device_id: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub devices: Vec<DevicePreset>,
    pub apps: Vec<AppPreset>,
}

/// Record the current state. Apps without a stable executable name (keyed by pid) are skipped,
/// since a pid means nothing next time.
pub fn capture(name: &str, snap: &Snapshot) -> Preset {
    let devices = snap
        .devices
        .iter()
        .map(|d| DevicePreset {
            id: d.id.clone(),
            volume: d.volume,
            muted: d.muted,
        })
        .collect();
    let mut apps = Vec::new();
    for d in &snap.devices {
        for g in group_sessions(&snap.sessions, &d.id) {
            if g.key.starts_with("pid:") {
                continue;
            }
            apps.push(AppPreset {
                key: g.key,
                device_id: d.id.clone(),
                volume: g.volume,
                muted: g.muted,
            });
        }
    }
    Preset {
        name: name.to_string(),
        devices,
        apps,
    }
}

/// Commands that bring `snap` to the preset. Anything in the preset that isn't present right now
/// (an unplugged device, an app that isn't running) is skipped rather than an error.
pub fn commands(preset: &Preset, snap: &Snapshot) -> Vec<Command> {
    let mut out = Vec::new();
    for p in &preset.devices {
        if snap.devices.iter().any(|d| d.id == p.id) {
            out.push(Command::SetDeviceVolume {
                id: p.id.clone(),
                volume: p.volume,
            });
            out.push(Command::SetDeviceMute {
                id: p.id.clone(),
                muted: p.muted,
            });
        }
    }
    for p in &preset.apps {
        for g in group_sessions(&snap.sessions, &p.device_id)
            .into_iter()
            .filter(|g| g.key == p.key)
        {
            for (device_id, id) in g.sessions {
                out.push(Command::SetSessionVolume {
                    device_id: device_id.clone(),
                    id: id.clone(),
                    volume: p.volume,
                });
                out.push(Command::SetSessionMute {
                    device_id,
                    id,
                    muted: p.muted,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Device, Flow, Session};

    fn snap() -> Snapshot {
        let dev = |id: &str, vol: f32| Device {
            id: id.into(),
            name: id.into(),
            flow: Flow::Render,
            volume: vol,
            muted: false,
            peak: 0.0,
            is_default: false,
        };
        let sess = |id: &str, dev: &str, exe: &str, pid: u32, vol: f32| Session {
            id: id.into(),
            device_id: dev.into(),
            pid,
            exe: exe.into(),
            name: exe.into(),
            icon: None,
            volume: vol,
            muted: false,
            peak: 0.0,
            active: true,
        };
        Snapshot {
            devices: vec![dev("d1", 0.5), dev("d2", 0.9)],
            sessions: vec![
                sess("a", "d1", "spotify", 1, 0.3),
                sess("b", "d1", "chrome", 2, 0.6),
                sess("c", "d1", "chrome", 3, 0.6),
                sess("z", "d2", "", 4, 0.2),
            ],
        }
    }

    #[test]
    fn capture_records_devices_and_named_apps_only() {
        let p = capture("Gaming", &snap());
        assert_eq!(p.devices.len(), 2);
        let keys: Vec<_> = p.apps.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, ["chrome", "spotify"]);
        assert!(p.apps.iter().all(|a| a.device_id == "d1"));
    }

    #[test]
    fn applying_restores_volumes_across_every_session_of_an_app() {
        let mut s = snap();
        let p = capture("x", &s);
        for sess in &mut s.sessions {
            sess.volume = 1.0;
        }
        for d in &mut s.devices {
            d.volume = 0.0;
        }
        let cmds = commands(&p, &s);
        let chrome_sets = cmds
            .iter()
            .filter(|c| matches!(c, Command::SetSessionVolume { volume, .. } if *volume == 0.6))
            .count();
        assert_eq!(chrome_sets, 2, "both chrome sessions");
        assert!(cmds.contains(&Command::SetDeviceVolume {
            id: "d2".into(),
            volume: 0.9
        }));
    }

    #[test]
    fn missing_devices_and_apps_are_skipped() {
        let p = capture("x", &snap());
        let empty = Snapshot::default();
        assert!(commands(&p, &empty).is_empty());
    }
}
