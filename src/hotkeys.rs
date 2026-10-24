//! Global hotkeys for the default playback device: volume up/down and mute. Events arrive on the
//! shell's thread, are forwarded over a channel, and wake the UI so they work with the window hidden.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use eframe::egui;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::config;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    VolumeUp,
    VolumeDown,
    ToggleMute,
}

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    map: Arc<Mutex<HashMap<u32, HotkeyAction>>>,
    rx: Receiver<HotkeyAction>,
}

impl Hotkeys {
    pub fn new(ctx: &egui::Context) -> Self {
        let map: Arc<Mutex<HashMap<u32, HotkeyAction>>> = Arc::default();
        let (tx, rx) = mpsc::channel();
        let (m, c) = (map.clone(), ctx.clone());
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state != HotKeyState::Pressed {
                return;
            }
            let action = m.lock().ok().and_then(|m| m.get(&e.id).copied());
            if let Some(a) = action {
                let _ = tx.send(a);
                c.request_repaint();
            }
        }));
        Self {
            manager: GlobalHotKeyManager::new().ok(),
            registered: Vec::new(),
            map,
            rx,
        }
    }

    pub fn poll(&self) -> Vec<HotkeyAction> {
        self.rx.try_iter().collect()
    }

    /// (Re)register from config. Returns one message per hotkey that couldn't be bound, e.g.
    /// because another program already owns that combination.
    pub fn apply(&mut self, cfg: &config::Hotkeys) -> Vec<String> {
        let Some(manager) = &self.manager else {
            return vec!["Global hotkeys are unavailable on this system".into()];
        };
        let _ = manager.unregister_all(&self.registered);
        self.registered.clear();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.clear();

        let mut errors = Vec::new();
        for (label, text, action) in [
            ("Volume up", &cfg.volume_up, HotkeyAction::VolumeUp),
            ("Volume down", &cfg.volume_down, HotkeyAction::VolumeDown),
            ("Mute", &cfg.mute, HotkeyAction::ToggleMute),
        ] {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let hotkey = match HotKey::from_str(text) {
                Ok(h) => h,
                Err(e) => {
                    errors.push(format!("{label}: can't read \"{text}\" ({e})"));
                    continue;
                }
            };
            match manager.register(hotkey) {
                Ok(()) => {
                    map.insert(hotkey.id(), action);
                    self.registered.push(hotkey);
                }
                Err(_) => errors.push(format!("{label}: \"{text}\" is already in use")),
            }
        }
        errors
    }
}
