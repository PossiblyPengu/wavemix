//! Everything the user can turn: strip and bus settings. Plain data so it can be saved, sent to
//! the audio thread, and edited by the UI without touching any audio code.

use serde::{Deserialize, Serialize};

/// Hardware or virtual inputs, left to right: 3 hardware, 2 virtual (Voicemeeter Banana's layout).
pub const STRIPS: usize = 5;
pub const HARDWARE_STRIPS: usize = 3;
/// Output buses: A1-A3 are hardware outputs, B1-B2 are virtual (cable) outputs.
pub const BUSES: usize = 5;

pub const GAIN_MIN_DB: f32 = -60.0;
pub const GAIN_MAX_DB: f32 = 12.0;

pub fn strip_name(i: usize) -> String {
    if i < HARDWARE_STRIPS {
        format!("Input {}", i + 1)
    } else {
        format!("Virtual {}", i + 1 - HARDWARE_STRIPS)
    }
}

pub fn bus_name(i: usize) -> &'static str {
    ["A1", "A2", "A3", "B1", "B2"][i.min(BUSES - 1)]
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqParams {
    pub enabled: bool,
    pub low_freq: f32,
    pub low_gain_db: f32,
    pub mid_freq: f32,
    pub mid_gain_db: f32,
    pub mid_q: f32,
    pub high_freq: f32,
    pub high_gain_db: f32,
}

impl Default for EqParams {
    fn default() -> Self {
        Self {
            enabled: false,
            low_freq: 120.0,
            low_gain_db: 0.0,
            mid_freq: 1000.0,
            mid_gain_db: 0.0,
            mid_q: 1.0,
            high_freq: 8000.0,
            high_gain_db: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GateParams {
    pub enabled: bool,
    pub threshold_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -45.0,
            attack_ms: 2.0,
            release_ms: 120.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompParams {
    pub enabled: bool,
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_db: f32,
}

impl Default for CompParams {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -20.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 120.0,
            makeup_db: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StripParams {
    pub gain_db: f32,
    pub mute: bool,
    pub solo: bool,
    /// Sum to mono before panning.
    pub mono: bool,
    /// Balance, -1 (left) ..= 1 (right).
    pub pan: f32,
    /// Which buses this strip feeds (A1 A2 A3 B1 B2).
    pub sends: [bool; BUSES],
    pub gate: GateParams,
    pub eq: EqParams,
    pub comp: CompParams,
}

impl Default for StripParams {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            mute: false,
            solo: false,
            mono: false,
            pan: 0.0,
            // Out of the box a strip feeds the first hardware output.
            sends: [true, false, false, false, false],
            gate: GateParams::default(),
            eq: EqParams::default(),
            comp: CompParams::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BusParams {
    pub gain_db: f32,
    pub mute: bool,
    /// Brick-wall limiter at the end of the bus.
    pub limiter: bool,
    pub limiter_ceiling_db: f32,
    /// Delay applied to the whole bus, in milliseconds, to line up outputs whose devices have
    /// different latencies (Bluetooth headphones against wired speakers).
    pub delay_ms: f32,
}

impl Default for BusParams {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            mute: false,
            limiter: true,
            limiter_ceiling_db: -0.5,
            delay_ms: 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineParams {
    pub strips: [StripParams; STRIPS],
    pub buses: [BusParams; BUSES],
    /// Skip every strip's gate, EQ and compressor, to hear what they are doing.
    pub bypass_fx: bool,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            strips: [StripParams::default(); STRIPS],
            buses: [BusParams::default(); BUSES],
            bypass_fx: false,
        }
    }
}

/// Which device each slot uses, by device name. `None` leaves the slot empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IoConfig {
    pub inputs: [Option<String>; STRIPS],
    pub outputs: [Option<String>; BUSES],
    pub engine_on: bool,
}
