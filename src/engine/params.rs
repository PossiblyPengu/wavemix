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

/// How many bands a bus equaliser has.
pub const BUS_EQ_BANDS: usize = 6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BandKind {
    #[default]
    Peak,
    LowShelf,
    HighShelf,
    HighPass,
    LowPass,
}

impl BandKind {
    pub const ALL: [BandKind; 5] = [
        BandKind::Peak,
        BandKind::LowShelf,
        BandKind::HighShelf,
        BandKind::HighPass,
        BandKind::LowPass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BandKind::Peak => "Bell",
            BandKind::LowShelf => "Low shelf",
            BandKind::HighShelf => "High shelf",
            BandKind::HighPass => "High-pass",
            BandKind::LowPass => "Low-pass",
        }
    }

    /// Filters that cut rather than boost have no gain control.
    pub fn has_gain(self) -> bool {
        !matches!(self, BandKind::HighPass | BandKind::LowPass)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BandParams {
    pub on: bool,
    pub kind: BandKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

impl Default for BandParams {
    fn default() -> Self {
        Self {
            on: true,
            kind: BandKind::Peak,
            freq: 1000.0,
            gain_db: 0.0,
            q: 1.0,
        }
    }
}

/// A six-band parametric equaliser, for shaping a whole output.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParamEqParams {
    pub enabled: bool,
    pub bands: [BandParams; BUS_EQ_BANDS],
}

impl Default for ParamEqParams {
    fn default() -> Self {
        let band = |kind, freq| BandParams {
            kind,
            freq,
            ..BandParams::default()
        };
        Self {
            enabled: false,
            bands: [
                band(BandKind::LowShelf, 80.0),
                band(BandKind::Peak, 220.0),
                band(BandKind::Peak, 600.0),
                band(BandKind::Peak, 1800.0),
                band(BandKind::Peak, 5000.0),
                band(BandKind::HighShelf, 10_000.0),
            ],
        }
    }
}

/// Game / chat balance: one dial that turns one strip down as it favours the other, like the
/// ChatMix dial on a gaming headset. 0 is even, -1 is all game, +1 is all chat.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatMixParams {
    pub enabled: bool,
    pub balance: f32,
    /// The strips on each side (0-based).
    pub game: usize,
    pub chat: usize,
}

impl Default for ChatMixParams {
    fn default() -> Self {
        Self {
            enabled: false,
            balance: 0.0,
            game: HARDWARE_STRIPS + 1,
            chat: HARDWARE_STRIPS,
        }
    }
}

/// How far (in dB) the dial turns the far side down when pushed all the way.
pub const CHATMIX_RANGE_DB: f32 = 30.0;

/// Ducking: when the `source` strip (a microphone) is loud, turn the virtual strips (music, game
/// and chat from apps) down, and bring them back when it stops.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckParams {
    pub enabled: bool,
    /// Which strip listens (0-based; one of the hardware inputs).
    pub source: usize,
    /// The source must be louder than this to duck.
    pub threshold_db: f32,
    /// How far the others are turned down.
    pub reduction_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
}

impl Default for DuckParams {
    fn default() -> Self {
        Self {
            enabled: false,
            source: 0,
            threshold_db: -40.0,
            reduction_db: 12.0,
            attack_ms: 40.0,
            release_ms: 600.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StripParams {
    /// Remove background noise (a trained noise suppressor, meant for microphones).
    pub denoise: bool,
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
            denoise: false,
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
    /// Equaliser across the whole output, before the limiter.
    pub eq: ParamEqParams,
    /// Mix a little of each side into the other, as speakers do, so headphones sound less
    /// "inside the head" (0 = off, 1 = a lot).
    pub crossfeed: f32,
    /// Stereo width: 0 is mono, 1 is as recorded, 2 is wider.
    pub width: f32,
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
            eq: ParamEqParams::default(),
            crossfeed: 0.0,
            width: 1.0,
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
    pub duck: DuckParams,
    pub chatmix: ChatMixParams,
}

impl Default for EngineParams {
    fn default() -> Self {
        Self {
            strips: [StripParams::default(); STRIPS],
            buses: [BusParams::default(); BUSES],
            bypass_fx: false,
            duck: DuckParams::default(),
            chatmix: ChatMixParams::default(),
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
