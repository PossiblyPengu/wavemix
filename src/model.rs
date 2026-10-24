//! Platform-independent mixer state. The backend produces a [`Snapshot`]; the UI reads it and sends
//! [`Command`]s back. Nothing in here touches the OS, so it is all unit-testable.

use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Flow {
    /// Playback: speakers, headphones, virtual cables' inputs.
    Render,
    /// Recording: microphones, line-in.
    Capture,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub flow: Flow,
    /// Master volume scalar, 0.0..=1.0.
    pub volume: f32,
    pub muted: bool,
    /// Current peak level, linear 0.0..=1.0.
    pub peak: f32,
    /// True for the default device of its flow.
    pub is_default: bool,
}

/// An app icon as straight RGBA, shared cheaply between snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct IconData {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// One WASAPI audio session: a single stream group from one process on one device.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    /// Unique per session instance; stable while the session lives.
    pub id: String,
    pub device_id: String,
    pub pid: u32,
    /// Executable name without extension, e.g. `chrome`. Empty for system sounds.
    pub exe: String,
    /// Name to show: the session's own display name if it set one, else the executable.
    pub name: String,
    pub icon: Option<Arc<IconData>>,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
    /// False once the session has gone inactive (no audio playing); still listed so the fader
    /// can be set before the app makes a sound.
    pub active: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub sessions: Vec<Session>,
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::enum_variant_names)]
pub enum Command {
    SetDeviceVolume {
        id: String,
        volume: f32,
    },
    SetDeviceMute {
        id: String,
        muted: bool,
    },
    SetSessionVolume {
        device_id: String,
        id: String,
        volume: f32,
    },
    SetSessionMute {
        device_id: String,
        id: String,
        muted: bool,
    },
    SetDefaultDevice {
        id: String,
        flow: Flow,
    },
    /// Pin an app's output to a device (`None` returns it to the Windows default).
    SetAppDevice {
        pids: Vec<u32>,
        device_id: Option<String>,
    },
}

/// Apply a command to a snapshot in place, the same way the backend eventually will.
pub fn apply_command(snap: &mut Snapshot, cmd: &Command) {
    match cmd {
        Command::SetDeviceVolume { id, volume } => {
            for d in snap.devices.iter_mut().filter(|d| &d.id == id) {
                d.volume = volume.clamp(0.0, 1.0);
            }
        }
        Command::SetDeviceMute { id, muted } => {
            for d in snap.devices.iter_mut().filter(|d| &d.id == id) {
                d.muted = *muted;
            }
        }
        Command::SetSessionVolume {
            device_id,
            id,
            volume,
        } => {
            for s in snap
                .sessions
                .iter_mut()
                .filter(|s| &s.device_id == device_id && &s.id == id)
            {
                s.volume = volume.clamp(0.0, 1.0);
            }
        }
        Command::SetSessionMute {
            device_id,
            id,
            muted,
        } => {
            for s in snap
                .sessions
                .iter_mut()
                .filter(|s| &s.device_id == device_id && &s.id == id)
            {
                s.muted = *muted;
            }
        }
        // The app moves devices only once the engine re-enumerates, so there is nothing to patch.
        Command::SetAppDevice { .. } => {}
        Command::SetDefaultDevice { id, flow } => {
            for d in snap.devices.iter_mut().filter(|d| d.flow == *flow) {
                d.is_default = &d.id == id;
            }
        }
    }
}

/// All sessions of one application on one device, shown as a single strip.
#[derive(Clone, Debug, PartialEq)]
pub struct AppStrip {
    pub key: String,
    pub name: String,
    pub icon: Option<Arc<IconData>>,
    pub pids: Vec<u32>,
    pub device_id: String,
    /// `(device_id, session_id)` for every underlying session; commands fan out to all of them.
    pub sessions: Vec<(String, String)>,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
    pub active: bool,
}

/// Our own audio sessions (the mixer playing to a speaker) are not apps to control.
fn is_own(s: &Session) -> bool {
    s.pid != 0 && s.pid == std::process::id()
}

fn app_key(s: &Session) -> String {
    if s.exe.is_empty() {
        format!("pid:{}", s.pid)
    } else {
        s.exe.to_lowercase()
    }
}

fn new_strip(key: String, s: &Session) -> AppStrip {
    AppStrip {
        key,
        name: s.name.clone(),
        icon: None,
        pids: Vec::new(),
        device_id: s.device_id.clone(),
        sessions: Vec::new(),
        volume: 0.0,
        muted: true,
        peak: 0.0,
        active: false,
    }
}

fn add_session(g: &mut AppStrip, s: &Session) {
    g.sessions.push((s.device_id.clone(), s.id.clone()));
    if s.pid != 0 && !g.pids.contains(&s.pid) {
        g.pids.push(s.pid);
    }
    if g.icon.is_none() {
        g.icon = s.icon.clone();
    }
    // The strip's fader shows the loudest member; muted only if every member is.
    g.volume = g.volume.max(s.volume);
    g.muted &= s.muted;
    g.peak = g.peak.max(s.peak);
    g.active |= s.active;
}

/// Active apps first, then alphabetical, so a strip doesn't jump around while it plays.
fn sorted(groups: BTreeMap<String, AppStrip>) -> Vec<AppStrip> {
    let mut v: Vec<AppStrip> = groups.into_values().collect();
    v.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v
}

/// Group sessions by (device, application) the way EarTrumpet does, so an app with several
/// sessions (a browser with a tab per stream) is one fader instead of five.
pub fn group_sessions(sessions: &[Session], device_id: &str) -> Vec<AppStrip> {
    let mut groups: BTreeMap<String, AppStrip> = BTreeMap::new();
    for s in sessions
        .iter()
        .filter(|s| s.device_id == device_id && !is_own(s))
    {
        let key = app_key(s);
        let g = groups
            .entry(key.clone())
            .or_insert_with(|| new_strip(key, s));
        add_session(g, s);
    }
    sorted(groups)
}

/// One strip per application across every playback device. Windows keeps a (usually silent)
/// session for an app on each device it has ever used, so listing per device shows the same
/// program over and over. The strip's device is where it is actually playing; failing that, the
/// default device if it has a session there. Volume and mute changes still reach every session.
pub fn group_apps(sessions: &[Session], default_device: Option<&str>) -> Vec<AppStrip> {
    let mut groups: BTreeMap<String, (u8, AppStrip)> = BTreeMap::new();
    for s in sessions.iter().filter(|s| !is_own(s)) {
        let key = app_key(s);
        let rank = u8::from(s.active) * 2 + u8::from(Some(s.device_id.as_str()) == default_device);
        let entry = groups
            .entry(key.clone())
            .or_insert_with(|| (rank, new_strip(key, s)));
        if rank > entry.0 {
            entry.0 = rank;
            entry.1.device_id = s.device_id.clone();
        }
        add_session(&mut entry.1, s);
    }
    sorted(groups.into_iter().map(|(k, (_, g))| (k, g)).collect())
}

pub const DB_FLOOR: f32 = -60.0;

/// Linear amplitude to dBFS, floored so silence doesn't produce -inf.
pub fn amp_to_db(amp: f32) -> f32 {
    if amp <= 0.000_001 {
        DB_FLOOR
    } else {
        (20.0 * amp.log10()).max(DB_FLOOR)
    }
}

/// Position of a dB level on a meter, 0.0 (floor) ..= 1.0 (0 dBFS).
pub fn db_to_meter(db: f32) -> f32 {
    ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0)
}

pub fn percent(volume: f32) -> i32 {
    (volume.clamp(0.0, 1.0) * 100.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(id: &str, dev: &str, exe: &str, vol: f32, muted: bool, active: bool) -> Session {
        Session {
            id: id.into(),
            device_id: dev.into(),
            pid: 1,
            exe: exe.into(),
            name: exe.into(),
            volume: vol,
            muted,
            icon: None,
            peak: 0.0,
            active,
        }
    }

    #[test]
    fn an_app_with_sessions_on_several_devices_is_one_strip_on_the_device_it_plays() {
        let on = |id: &str, dev: &str, active: bool| {
            sess(id, dev, "steam", 1.0, false, active)
        };
        let list = [on("a", "d1", false), on("b", "d2", true), on("c", "d3", false)];
        let apps = group_apps(&list, Some("d1"));
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].device_id, "d2"); // the one that is playing
        assert_eq!(apps[0].sessions.len(), 3); // but a slider change reaches all of them
        // Nothing playing: fall back to the default device.
        let quiet = [on("a", "d2", false), on("b", "d1", false)];
        assert_eq!(group_apps(&quiet, Some("d1"))[0].device_id, "d1");
    }

    #[test]
    fn our_own_sessions_are_not_listed() {
        let mut me = sess("me", "d1", "wavemix", 1.0, false, true);
        me.pid = std::process::id();
        assert!(group_sessions(&[me.clone()], "d1").is_empty());
        assert!(group_apps(&[me], None).is_empty());
    }

    #[test]
    fn groups_same_app_on_same_device() {
        let s = vec![
            sess("a", "d1", "chrome", 0.4, false, true),
            sess("b", "d1", "Chrome", 0.8, true, false),
            sess("c", "d2", "chrome", 0.1, false, true),
        ];
        let g = group_sessions(&s, "d1");
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].sessions.len(), 2);
        assert_eq!(g[0].volume, 0.8);
        assert!(!g[0].muted, "muted only when every member is muted");
        assert!(g[0].active);
    }

    #[test]
    fn all_muted_members_mute_the_strip() {
        let s = vec![
            sess("a", "d1", "x", 0.4, true, true),
            sess("b", "d1", "x", 0.8, true, true),
        ];
        assert!(group_sessions(&s, "d1")[0].muted);
    }

    #[test]
    fn active_strips_sort_first_then_alphabetical() {
        let s = vec![
            sess("a", "d1", "zoom", 0.5, false, true),
            sess("b", "d1", "alpha", 0.5, false, false),
            sess("c", "d1", "beta", 0.5, false, true),
        ];
        let names: Vec<_> = group_sessions(&s, "d1")
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, ["beta", "zoom", "alpha"]);
    }

    #[test]
    fn processless_sessions_do_not_merge() {
        let mut a = sess("a", "d1", "", 0.5, false, true);
        let mut b = sess("b", "d1", "", 0.5, false, true);
        a.pid = 10;
        b.pid = 20;
        assert_eq!(group_sessions(&[a, b], "d1").len(), 2);
    }

    #[test]
    fn commands_patch_the_snapshot() {
        let mut snap = Snapshot {
            devices: vec![],
            sessions: vec![sess("a", "d1", "x", 0.5, false, true)],
        };
        apply_command(
            &mut snap,
            &Command::SetSessionVolume {
                device_id: "d1".into(),
                id: "a".into(),
                volume: 1.7,
            },
        );
        assert_eq!(snap.sessions[0].volume, 1.0, "clamped");
        apply_command(
            &mut snap,
            &Command::SetSessionMute {
                device_id: "other".into(),
                id: "a".into(),
                muted: true,
            },
        );
        assert!(!snap.sessions[0].muted, "wrong device is untouched");
    }

    #[test]
    fn db_math() {
        assert_eq!(amp_to_db(0.0), DB_FLOOR);
        assert!((amp_to_db(1.0)).abs() < 1e-4);
        assert!((amp_to_db(0.5) + 6.02).abs() < 0.01);
        assert_eq!(db_to_meter(DB_FLOOR), 0.0);
        assert_eq!(db_to_meter(0.0), 1.0);
        assert_eq!(db_to_meter(10.0), 1.0);
        assert_eq!(percent(0.755), 76);
    }
}
