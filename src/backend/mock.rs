//! A fake audio system with animated levels. It is the engine on non-Windows platforms (so the UI
//! can be developed and screenshotted anywhere) and with `--demo` on Windows.

use std::time::Instant;

use super::Engine;
use crate::model::{apply_command, Command, Device, Flow, Session, Snapshot};

pub struct MockEngine {
    state: Snapshot,
    start: Instant,
}

fn dev(id: &str, name: &str, flow: Flow, volume: f32, is_default: bool) -> Device {
    Device {
        id: id.into(),
        name: name.into(),
        flow,
        volume,
        muted: false,
        peak: 0.0,
        is_default,
    }
}

fn sess(id: &str, device: &str, pid: u32, exe: &str, name: &str, volume: f32) -> Session {
    Session {
        id: id.into(),
        device_id: device.into(),
        pid,
        exe: exe.into(),
        name: name.into(),
        volume,
        muted: false,
        peak: 0.0,
        active: true,
    }
}

impl MockEngine {
    pub fn new() -> Self {
        let mut idle = sess("s-teams", "spk", 40, "teams", "Microsoft Teams", 0.8);
        idle.active = false;
        let mut state = Snapshot {
            devices: vec![
                dev("spk", "Speakers (Realtek Audio)", Flow::Render, 0.62, true),
                dev("hp", "Headphones (USB DAC)", Flow::Render, 0.45, false),
                dev("cable", "CABLE Input (VB-Audio)", Flow::Render, 1.0, false),
                dev("mic", "Microphone (Blue Yeti)", Flow::Capture, 0.7, true),
                dev("line", "Line In (Realtek Audio)", Flow::Capture, 0.5, false),
            ],
            sessions: vec![
                sess("s-spotify", "spk", 10, "spotify", "Spotify", 0.55),
                sess("s-chrome1", "spk", 20, "chrome", "Google Chrome", 0.9),
                sess("s-chrome2", "spk", 21, "chrome", "Google Chrome", 0.7),
                sess("s-discord", "hp", 30, "discord", "Discord", 0.85),
                sess("s-game", "hp", 50, "eldenring", "ELDEN RING", 0.4),
                idle,
                sess("s-sys", "spk", 4, "", "System sounds", 0.3),
                sess("s-obs", "cable", 60, "obs64", "OBS Studio", 1.0),
            ],
        };
        // Sessions of one app share a volume in practice, so the grouped strip is honest.
        if let Some(s) = state.sessions.iter_mut().find(|s| s.id == "s-chrome2") {
            s.volume = 0.9;
        }
        Self {
            state,
            start: Instant::now(),
        }
    }
}

impl Default for MockEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// A wobbling "audio-like" level in 0..1 that differs per `seed`.
fn wobble(t: f32, seed: f32) -> f32 {
    let a = (t * 3.1 + seed).sin() * 0.5 + 0.5;
    let b = (t * 7.7 + seed * 2.3).sin() * 0.5 + 0.5;
    let c = (t * 0.6 + seed * 0.7).sin() * 0.5 + 0.5;
    (a * b * c).powf(0.8)
}

impl Engine for MockEngine {
    fn tick(&mut self, _rescan: bool) -> Snapshot {
        let t = self.start.elapsed().as_secs_f32();
        for (i, d) in self.state.devices.iter_mut().enumerate() {
            let live = if d.muted { 0.0 } else { 1.0 };
            d.peak = wobble(t, i as f32 * 1.7) * d.volume * live;
        }
        for (i, s) in self.state.sessions.iter_mut().enumerate() {
            let live = if s.muted || !s.active { 0.0 } else { 1.0 };
            s.peak = wobble(t, 10.0 + i as f32 * 2.9) * s.volume * live;
        }
        self.state.clone()
    }

    fn apply(&mut self, cmd: &Command) {
        apply_command(&mut self.state, cmd);
    }
}
