//! Answers requests from the local control interface by doing what the buttons do.

use eframe::egui;
use serde_json::{json, Value};

use super::MixerApp;
use crate::engine::mixer::MAX_BUS_DELAY_MS;
use crate::engine::params::{bus_name, BUSES, GAIN_MAX_DB, GAIN_MIN_DB, STRIPS};
use crate::remote::{self, Request};

/// "1", "on", "true" / "0", "off", "false" / "toggle" (relative to `current`).
fn flag(value: Option<&String>, current: bool) -> Option<bool> {
    match value?.to_lowercase().as_str() {
        "1" | "on" | "true" | "yes" => Some(true),
        "0" | "off" | "false" | "no" => Some(false),
        "toggle" => Some(!current),
        _ => None,
    }
}

fn number(value: Option<&String>) -> Option<f32> {
    value?.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn ok() -> (u16, String) {
    (200, json!({ "ok": true }).to_string())
}

fn fail(code: u16, why: &str) -> (u16, String) {
    (code, json!({ "ok": false, "error": why }).to_string())
}

impl MixerApp {
    /// Runs every frame: starts or stops the server to match the setting, and answers requests.
    pub(super) fn drive_remote(&mut self, ctx: &egui::Context) {
        let want = self.config.remote.enabled && !self.config.remote.token.is_empty();
        let running_as = self.remote.as_ref().map(|r| (r.port, r.token.clone()));
        let wanted_as = (self.config.remote.port, self.config.remote.token.clone());
        if !want {
            self.remote = None;
            self.remote_error = None;
        } else if running_as.as_ref() != Some(&wanted_as) && self.remote_retry_ok() {
            self.remote = None;
            let c = ctx.clone();
            match remote::start(wanted_as.0, wanted_as.1, move || c.request_repaint()) {
                Ok(r) => {
                    self.remote = Some(r);
                    self.remote_error = None;
                }
                Err(e) => {
                    self.remote_error = Some(format!("Couldn't start: {e}"));
                    self.remote_failed_at = Some(std::time::Instant::now());
                }
            }
        }
        let mut jobs = Vec::new();
        if let Some(r) = &self.remote {
            while let Ok(job) = r.jobs.try_recv() {
                jobs.push(job);
            }
        }
        for job in jobs {
            let answer = self.handle_remote(&job.request);
            let _ = job.reply.send(answer);
        }
    }

    /// After a failed start (port taken), wait a few seconds before trying again.
    fn remote_retry_ok(&self) -> bool {
        self.remote_failed_at
            .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(5))
    }

    pub(super) fn handle_remote(&mut self, req: &Request) -> (u16, String) {
        let path: Vec<&str> = req.path.iter().map(String::as_str).collect();
        let q = &req.query;
        match path.as_slice() {
            ["api", "state"] => (200, self.state_json().to_string()),
            ["api", "engine"] => {
                let Some(on) = flag(q.get("on"), self.engine.is_running()) else {
                    return fail(400, "use on=1, on=0 or on=toggle");
                };
                self.engine.set_running(on);
                self.config.engine.io.engine_on = on;
                self.mark_engine_dirty();
                ok()
            }
            ["api", "bypass"] => {
                let mut p = self.engine.params();
                let Some(on) = flag(q.get("on"), p.bypass_fx) else {
                    return fail(400, "use on=1, on=0 or on=toggle");
                };
                p.bypass_fx = on;
                self.commit_params(p);
                ok()
            }
            ["api", "duck"] => {
                let mut p = self.engine.params();
                let Some(on) = flag(q.get("on"), p.duck.enabled) else {
                    return fail(400, "use on=1, on=0 or on=toggle");
                };
                p.duck.enabled = on;
                self.commit_params(p);
                ok()
            }
            ["api", "chatmix"] => {
                let mut p = self.engine.params();
                if let Some(on) = flag(q.get("on"), p.chatmix.enabled) {
                    p.chatmix.enabled = on;
                }
                if let Some(v) = number(q.get("balance")) {
                    p.chatmix.balance = v.clamp(-1.0, 1.0);
                    // Turning the dial is a clear sign the person wants it on.
                    if q.get("on").is_none() {
                        p.chatmix.enabled = true;
                    }
                }
                if let Some(v) = number(q.get("nudge")) {
                    p.chatmix.balance = (p.chatmix.balance + v).clamp(-1.0, 1.0);
                    p.chatmix.enabled = true;
                }
                self.commit_params(p);
                ok()
            }
            ["api", "strip", i] => {
                let Some(i) = i.parse::<usize>().ok().filter(|i| *i < STRIPS) else {
                    return fail(404, "no such strip");
                };
                let mut p = self.engine.params();
                let s = &mut p.strips[i];
                if let Some(db) = number(q.get("gain_db")) {
                    s.gain_db = db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
                }
                if let Some(pan) = number(q.get("pan")) {
                    s.pan = pan.clamp(-1.0, 1.0);
                }
                for (key, field) in [
                    ("mute", &mut s.mute),
                    ("solo", &mut s.solo),
                    ("mono", &mut s.mono),
                    ("denoise", &mut s.denoise),
                ] {
                    if let Some(v) = flag(q.get(key), *field) {
                        *field = v;
                    }
                }
                for b in 0..BUSES {
                    let key = format!("send_{}", bus_name(b).to_lowercase());
                    if let Some(v) = flag(q.get(&key), s.sends[b]) {
                        s.sends[b] = v;
                    }
                }
                self.commit_params(p);
                ok()
            }
            ["api", "bus", b] => {
                let Some(b) = b.parse::<usize>().ok().filter(|b| *b < BUSES) else {
                    return fail(404, "no such bus");
                };
                let mut p = self.engine.params();
                let bus = &mut p.buses[b];
                if let Some(db) = number(q.get("gain_db")) {
                    bus.gain_db = db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
                }
                if let Some(ms) = number(q.get("delay_ms")) {
                    bus.delay_ms = ms.clamp(0.0, MAX_BUS_DELAY_MS);
                }
                if let Some(v) = flag(q.get("mute"), bus.mute) {
                    bus.mute = v;
                }
                if let Some(v) = flag(q.get("limiter"), bus.limiter) {
                    bus.limiter = v;
                }
                if let Some(v) = number(q.get("crossfeed")) {
                    bus.crossfeed = v.clamp(0.0, 1.0);
                }
                if let Some(v) = number(q.get("width")) {
                    bus.width = v.clamp(0.0, 2.0);
                }
                self.commit_params(p);
                ok()
            }
            ["api", "preset"] => {
                let Some(name) = q.get("name") else {
                    return fail(400, "give a name");
                };
                let Some(i) = self
                    .config
                    .presets
                    .iter()
                    .position(|p| p.name.eq_ignore_ascii_case(name))
                else {
                    return fail(404, "no preset with that name");
                };
                let snap = self.backend.snapshot();
                for c in crate::preset::commands(&self.config.presets[i], &snap) {
                    self.backend.send(c);
                }
                ok()
            }
            ["api", "record"] => {
                let recording = self.engine.recording().is_some();
                let Some(on) = flag(q.get("on"), recording) else {
                    return fail(400, "use on=1, on=0 or on=toggle");
                };
                let bus = number(q.get("bus")).map(|b| b as usize);
                self.set_recording(on, bus);
                ok()
            }
            ["api", "macro"] => match q.get("name") {
                Some(name) => self.run_macro(name),
                None => fail(400, "give a name"),
            },
            ["api", "key"] => self.remote_key(q.get("action").map(String::as_str).unwrap_or("")),
            _ => fail(404, "unknown request; see Settings > Remote control"),
        }
    }

    /// The same actions as the keyboard shortcuts.
    #[cfg(windows)]
    fn remote_key(&mut self, action: &str) -> (u16, String) {
        use crate::hotkeys::HotkeyAction as A;
        let a = match action {
            "volume_up" => A::VolumeUp,
            "volume_down" => A::VolumeDown,
            "mute" => A::ToggleMute,
            "mic_mute" => A::MicMute,
            "next_output" => A::NextOutput,
            "previous_output" => A::PrevOutput,
            "app_up" => A::AppUp,
            "app_down" => A::AppDown,
            "app_mute" => A::AppMute,
            _ => return fail(400, "unknown action"),
        };
        self.run_hotkey(a);
        ok()
    }

    #[cfg(not(windows))]
    fn remote_key(&mut self, _action: &str) -> (u16, String) {
        fail(404, "keyboard actions need Windows")
    }

    fn state_json(&self) -> Value {
        let params = self.engine.params();
        let strips: Vec<Value> = params
            .strips
            .iter()
            .enumerate()
            .map(|(i, s)| {
                json!({
                    "index": i,
                    "name": self.config.strip_title(i),
                    "gain_db": s.gain_db, "mute": s.mute, "solo": s.solo, "mono": s.mono,
                    "pan": s.pan, "denoise": s.denoise,
                    "sends": (0..BUSES).filter(|b| s.sends[*b]).map(bus_name).collect::<Vec<_>>(),
                })
            })
            .collect();
        let buses: Vec<Value> = params
            .buses
            .iter()
            .enumerate()
            .map(|(b, p)| {
                json!({
                    "index": b, "id": bus_name(b), "name": self.config.bus_title(b),
                    "gain_db": p.gain_db, "mute": p.mute, "limiter": p.limiter,
                    "delay_ms": p.delay_ms, "eq": p.eq.enabled,
                    "crossfeed": p.crossfeed, "width": p.width,
                })
            })
            .collect();
        let recording = self
            .engine
            .recording()
            .map(|(bus, path, secs)| json!({ "bus": bus, "file": path, "seconds": secs }));
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "engine_on": self.engine.is_running(),
            "bypass_fx": params.bypass_fx,
            "ducking": params.duck.enabled,
            "chatmix": {
                "on": params.chatmix.enabled, "balance": params.chatmix.balance,
                "game": params.chatmix.game, "chat": params.chatmix.chat,
            },
            "recording": recording,
            "strips": strips,
            "buses": buses,
            "presets": self.config.presets.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            "macros": self.config.macros.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
        })
    }
}
