//! MIDI input devices. Windows only: elsewhere there are simply none.

use std::sync::mpsc::Receiver;

use crate::midi::Event;

/// The names of the connected MIDI inputs.
#[cfg(windows)]
pub fn port_names() -> Vec<String> {
    let Ok(input) = midir::MidiInput::new("Wavemix") else {
        return Vec::new();
    };
    input
        .ports()
        .iter()
        .filter_map(|p| input.port_name(p).ok())
        .collect()
}

#[cfg(not(windows))]
pub fn port_names() -> Vec<String> {
    Vec::new()
}

/// Open connections to MIDI inputs, and the messages arriving from them.
pub struct Inputs {
    pub events: Receiver<Event>,
    /// What was asked for when these were opened (a port's name, or empty for every port).
    pub device: String,
    /// Which ports are open, to notice when a controller is plugged in or removed.
    pub open: Vec<String>,
    /// Held to keep the connections alive.
    #[cfg(windows)]
    _connections: Vec<midir::MidiInputConnection<()>>,
}

/// Listen to `device` (every input if it is empty). `wake` runs after each message so the UI can
/// wake up and handle it.
#[cfg(windows)]
pub fn open(device: &str, wake: impl Fn() + Send + Sync + Clone + 'static) -> Inputs {
    let (tx, events) = std::sync::mpsc::channel();
    let mut connections = Vec::new();
    let mut open = Vec::new();
    for name in port_names() {
        if !device.is_empty() && name != device {
            continue;
        }
        let Ok(mut input) = midir::MidiInput::new("Wavemix") else {
            continue;
        };
        // Timing clock, active sensing and sysex flood a controller's port and mean nothing here.
        input.ignore(midir::Ignore::All);
        let Some(port) = input
            .ports()
            .into_iter()
            .find(|p| input.port_name(p).ok().as_deref() == Some(name.as_str()))
        else {
            continue;
        };
        let (tx, wake) = (tx.clone(), wake.clone());
        let connected = input.connect(
            &port,
            "wavemix-in",
            move |_, bytes, _| {
                if let Some(e) = crate::midi::parse(bytes) {
                    if tx.send(e).is_ok() {
                        wake();
                    }
                }
            },
            (),
        );
        if let Ok(c) = connected {
            connections.push(c);
            open.push(name);
        }
    }
    Inputs {
        events,
        device: device.to_string(),
        open,
        _connections: connections,
    }
}

#[cfg(not(windows))]
pub fn open(device: &str, _wake: impl Fn() + Send + Sync + Clone + 'static) -> Inputs {
    let (_tx, events) = std::sync::mpsc::channel();
    Inputs {
        events,
        device: device.to_string(),
        open: Vec::new(),
    }
}
