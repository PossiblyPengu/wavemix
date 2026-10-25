//! MIDI control: faders, knobs and pads on a controller drive the mixer.
//!
//! This file is the part that never touches a device: reading raw MIDI bytes, deciding which
//! bindings a message triggers, and turning that into the same one-line commands that macros and
//! the remote interface use (`strip/0?gain_db=-6`), so a knob does exactly what the matching
//! button or web address does. The Windows device code lives in `midi_device.rs`.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::engine::params::{GAIN_MAX_DB, GAIN_MIN_DB};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    /// A knob or fader ("control change").
    Cc,
    /// A key or pad.
    Note,
    /// Pitch bend, which is how many motorised faders report.
    Bend,
}

/// One message from a controller, reduced to what the mixer cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    /// 0-15 (shown to people as 1-16).
    pub channel: u8,
    /// Controller number or note number; 0 for pitch bend.
    pub number: u8,
    /// 0-127. A key being released has value 0.
    pub value: u8,
}

/// Read one raw MIDI message. Anything the mixer has no use for (clock, aftertouch, program
/// changes, system messages) is `None`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse(bytes: &[u8]) -> Option<Event> {
    let (&status, data) = bytes.split_first()?;
    let channel = status & 0x0f;
    match (status & 0xf0, data) {
        // A note-on with velocity 0 is how many keyboards say "released".
        (0x90, [note, velocity, ..]) => Some(Event {
            kind: Kind::Note,
            channel,
            number: note & 0x7f,
            value: velocity & 0x7f,
        }),
        (0x80, [note, _, ..]) => Some(Event {
            kind: Kind::Note,
            channel,
            number: note & 0x7f,
            value: 0,
        }),
        (0xb0, [controller, value, ..]) => Some(Event {
            kind: Kind::Cc,
            channel,
            number: controller & 0x7f,
            value: value & 0x7f,
        }),
        // Fourteen bits, least significant first; the top seven are plenty for a fader.
        (0xe0, [_, msb, ..]) => Some(Event {
            kind: Kind::Bend,
            channel,
            number: 0,
            value: msb & 0x7f,
        }),
        _ => None,
    }
}

/// Which control a binding listens to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Source {
    pub kind: Kind,
    pub channel: u8,
    pub number: u8,
}

impl Source {
    pub fn of(e: &Event) -> Self {
        Self {
            kind: e.kind,
            channel: e.channel,
            number: e.number,
        }
    }

    /// "CC 7, channel 1".
    pub fn label(&self) -> String {
        let what = match self.kind {
            Kind::Cc => format!("CC {}", self.number),
            Kind::Note => format!("Note {}", self.number),
            Kind::Bend => "Pitch bend".to_string(),
        };
        format!("{what}, channel {}", self.channel + 1)
    }
}

/// What a control does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    StripGain(usize),
    StripPan(usize),
    StripMute(usize),
    StripSolo(usize),
    BusGain(usize),
    BusMute(usize),
    /// The game / chat dial.
    ChatMix,
    Engine,
    Duck,
    Bypass,
    Record,
    Macro(String),
    Preset(String),
}

/// What each kind of target is called in the settings, in the order of [`Target::kind_index`].
pub const TARGET_KINDS: [&str; 13] = [
    "Input volume",
    "Input pan",
    "Input mute",
    "Input solo",
    "Output volume",
    "Output mute",
    "Game / chat dial",
    "Engine on / off",
    "Ducking",
    "Bypass effects",
    "Record",
    "Run a macro",
    "Load a preset",
];

impl Target {
    /// Position in [`TARGET_KINDS`].
    pub fn kind_index(&self) -> usize {
        match self {
            Self::StripGain(_) => 0,
            Self::StripPan(_) => 1,
            Self::StripMute(_) => 2,
            Self::StripSolo(_) => 3,
            Self::BusGain(_) => 4,
            Self::BusMute(_) => 5,
            Self::ChatMix => 6,
            Self::Engine => 7,
            Self::Duck => 8,
            Self::Bypass => 9,
            Self::Record => 10,
            Self::Macro(_) => 11,
            Self::Preset(_) => 12,
        }
    }

    /// A target of the given kind with the first strip, bus or name filled in.
    pub fn of_kind(kind: usize, first_name: &str) -> Self {
        match kind {
            0 => Self::StripGain(0),
            1 => Self::StripPan(0),
            2 => Self::StripMute(0),
            3 => Self::StripSolo(0),
            4 => Self::BusGain(0),
            5 => Self::BusMute(0),
            6 => Self::ChatMix,
            7 => Self::Engine,
            8 => Self::Duck,
            9 => Self::Bypass,
            10 => Self::Record,
            11 => Self::Macro(first_name.to_string()),
            _ => Self::Preset(first_name.to_string()),
        }
    }

    /// Switches and pads act once when pressed; faders and knobs follow the control.
    pub fn is_button(&self) -> bool {
        !matches!(
            self,
            Self::StripGain(_) | Self::StripPan(_) | Self::BusGain(_) | Self::ChatMix
        )
    }

    /// The command this control sends for a reading of `value` (0-127).
    pub fn line(&self, value: u8) -> String {
        match self {
            Self::StripGain(i) => format!("strip/{i}?gain_db={:.1}", value_to_db(value)),
            Self::StripPan(i) => format!("strip/{i}?pan={:.3}", value_to_centered(value)),
            Self::StripMute(i) => format!("strip/{i}?mute=toggle"),
            Self::StripSolo(i) => format!("strip/{i}?solo=toggle"),
            Self::BusGain(b) => format!("bus/{b}?gain_db={:.1}", value_to_db(value)),
            Self::BusMute(b) => format!("bus/{b}?mute=toggle"),
            Self::ChatMix => format!("chatmix?balance={:.3}", value_to_centered(value)),
            Self::Engine => "engine?on=toggle".into(),
            Self::Duck => "duck?on=toggle".into(),
            Self::Bypass => "bypass?on=toggle".into(),
            Self::Record => "record?on=toggle".into(),
            Self::Macro(name) => format!("macro?name={}", escape(name)),
            Self::Preset(name) => format!("preset?name={}", escape(name)),
        }
    }
}

/// A fader position as decibels: 100 is 0 dB (where a fader's marked "unity" usually sits), the
/// bottom is silence, and the last stretch is the gain above unity.
pub fn value_to_db(value: u8) -> f32 {
    let v = f32::from(value.min(127));
    if v < 100.0 {
        GAIN_MIN_DB * (1.0 - v / 100.0)
    } else {
        GAIN_MAX_DB * (v - 100.0) / 27.0
    }
}

/// A knob position as -1 ..= 1, with the middle detent (64) exactly 0.
pub fn value_to_centered(value: u8) -> f32 {
    ((f32::from(value.min(127)) - 64.0) / 63.0).clamp(-1.0, 1.0)
}

/// Percent-encode the characters that would break a query string.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    /// None until a control has been picked ("learn").
    pub source: Option<Source>,
    pub target: Target,
}

impl Binding {
    /// Take the control that sent `e` as this binding's source. A key being released says nothing
    /// about which key it was, so it is ignored.
    pub fn learn(&mut self, e: &Event) -> bool {
        if e.kind == Kind::Note && e.value == 0 {
            return false;
        }
        self.source = Some(Source::of(e));
        true
    }
}

/// Turns messages into commands. Remembers which pads are held down so a pad that reports twice
/// (press and release, or a repeating knob) acts once.
#[derive(Default)]
pub struct Mapper {
    held: HashSet<Source>,
}

impl Mapper {
    /// The commands to run for a batch of messages, in order. A fader that moved many times since
    /// the last batch only sends where it ended up.
    pub fn commands(&mut self, bindings: &[Binding], events: &[Event]) -> Vec<String> {
        // (binding index if it follows a control, command)
        let mut out: Vec<(Option<usize>, String)> = Vec::new();
        for e in events {
            let source = Source::of(e);
            let down = match e.kind {
                Kind::Note => e.value > 0,
                _ => e.value >= 64,
            };
            let fresh = if down {
                self.held.insert(source)
            } else {
                self.held.remove(&source);
                false
            };
            for (n, b) in bindings.iter().enumerate() {
                if b.source != Some(source) {
                    continue;
                }
                if b.target.is_button() {
                    if fresh {
                        out.push((None, b.target.line(e.value)));
                    }
                } else {
                    let line = b.target.line(e.value);
                    match out.iter_mut().find(|(k, _)| *k == Some(n)) {
                        Some(entry) => entry.1 = line,
                        None => out.push((Some(n), line)),
                    }
                }
            }
        }
        out.into_iter().map(|(_, line)| line).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cc(number: u8, value: u8) -> Event {
        Event {
            kind: Kind::Cc,
            channel: 0,
            number,
            value,
        }
    }

    fn bind(kind: Kind, number: u8, target: Target) -> Binding {
        Binding {
            source: Some(Source {
                kind,
                channel: 0,
                number,
            }),
            target,
        }
    }

    #[test]
    fn raw_messages_are_read() {
        assert_eq!(parse(&[0xb0, 7, 100]), Some(cc(7, 100)));
        let on = parse(&[0x93, 36, 90]).unwrap();
        assert_eq!(
            (on.kind, on.channel, on.number, on.value),
            (Kind::Note, 3, 36, 90)
        );
        assert_eq!(
            parse(&[0x90, 36, 0]).unwrap().value,
            0,
            "velocity 0 is a release"
        );
        assert_eq!(parse(&[0x80, 36, 64]).unwrap().value, 0);
        let bend = parse(&[0xe2, 0x00, 0x40]).unwrap();
        assert_eq!((bend.kind, bend.channel, bend.value), (Kind::Bend, 2, 64));
        assert_eq!(parse(&[0xf8]), None, "clock ticks are not for us");
        assert_eq!(parse(&[0xc0, 5]), None, "program changes neither");
        assert_eq!(parse(&[0xb0, 7]), None, "a truncated message is ignored");
        assert_eq!(parse(&[]), None);
    }

    #[test]
    fn fader_positions_map_to_gain() {
        assert_eq!(value_to_db(0), GAIN_MIN_DB);
        assert_eq!(value_to_db(100), 0.0);
        assert_eq!(value_to_db(127), GAIN_MAX_DB);
        let mut last = f32::MIN;
        for v in 0..=127 {
            let db = value_to_db(v);
            assert!(db > last, "must rise all the way up the fader at {v}");
            last = db;
        }
    }

    #[test]
    fn knobs_have_a_centre_detent() {
        assert_eq!(value_to_centered(64), 0.0);
        assert_eq!(value_to_centered(0), -1.0);
        assert_eq!(value_to_centered(127), 1.0);
    }

    #[test]
    fn a_fader_sends_a_gain_command() {
        let bindings = [bind(Kind::Cc, 7, Target::StripGain(2))];
        let lines = Mapper::default().commands(&bindings, &[cc(7, 100)]);
        assert_eq!(lines, ["strip/2?gain_db=0.0"]);
        assert!(Mapper::default()
            .commands(&bindings, &[cc(8, 100)])
            .is_empty());
    }

    #[test]
    fn a_fader_that_moved_a_lot_only_reports_where_it_ended() {
        let bindings = [
            bind(Kind::Cc, 7, Target::StripGain(0)),
            bind(Kind::Cc, 8, Target::BusGain(1)),
        ];
        let events = [cc(7, 10), cc(8, 50), cc(7, 20), cc(7, 100)];
        let lines = Mapper::default().commands(&bindings, &events);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "strip/0?gain_db=0.0");
        assert!(lines[1].starts_with("bus/1?gain_db="));
    }

    #[test]
    fn a_pad_acts_once_per_press() {
        let bindings = [bind(Kind::Note, 36, Target::StripMute(0))];
        let note = |value| Event {
            kind: Kind::Note,
            channel: 0,
            number: 36,
            value,
        };
        let mut m = Mapper::default();
        assert_eq!(m.commands(&bindings, &[note(90)]), ["strip/0?mute=toggle"]);
        assert!(m.commands(&bindings, &[note(95)]).is_empty(), "still held");
        assert!(
            m.commands(&bindings, &[note(0)]).is_empty(),
            "release does nothing"
        );
        assert_eq!(m.commands(&bindings, &[note(80)]).len(), 1, "pressed again");
    }

    #[test]
    fn a_quick_tap_in_one_batch_still_counts() {
        let bindings = [bind(Kind::Cc, 20, Target::Engine)];
        let events = [cc(20, 127), cc(20, 0), cc(20, 127)];
        let lines = Mapper::default().commands(&bindings, &events);
        assert_eq!(lines, ["engine?on=toggle", "engine?on=toggle"]);
    }

    #[test]
    fn one_control_can_drive_several_things() {
        let bindings = [
            bind(Kind::Cc, 1, Target::ChatMix),
            bind(Kind::Cc, 1, Target::StripPan(0)),
        ];
        let lines = Mapper::default().commands(&bindings, &[cc(1, 127)]);
        assert_eq!(lines, ["chatmix?balance=1.000", "strip/0?pan=1.000"]);
    }

    #[test]
    fn learning_takes_the_next_control_and_ignores_releases() {
        let mut b = Binding {
            source: None,
            target: Target::Duck,
        };
        let release = Event {
            kind: Kind::Note,
            channel: 1,
            number: 40,
            value: 0,
        };
        assert!(!b.learn(&release));
        assert_eq!(b.source, None);
        assert!(b.learn(&cc(21, 5)));
        assert_eq!(b.source.unwrap().label(), "CC 21, channel 1");
    }

    #[test]
    fn target_kinds_round_trip() {
        for kind in 0..TARGET_KINDS.len() {
            assert_eq!(Target::of_kind(kind, "x").kind_index(), kind);
        }
    }

    #[test]
    fn names_survive_the_trip_through_a_command_line() {
        let line = Target::Macro("Stream & chill 100%".into()).line(127);
        let request = crate::remote::request_from_line(&line).unwrap();
        assert_eq!(request.path, ["api", "macro"]);
        assert_eq!(
            request.query.get("name").map(String::as_str),
            Some("Stream & chill 100%")
        );
    }
}
