//! The boundary between the engine and real (or simulated) audio devices.

use std::any::Any;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use ringbuf::{HeapCons, HeapProd};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    /// A virtual audio cable rather than real hardware (VB-Cable, Voicemeeter's own devices).
    pub is_virtual: bool,
}

/// Names that identify virtual cables. Matching is on the device's display name, which is all
/// the audio APIs give us.
pub fn looks_virtual(name: &str) -> bool {
    let n = name.to_lowercase();
    ["cable", "vb-audio", "voicemeeter", "virtual"]
        .iter()
        .any(|k| n.contains(k))
}

/// A running capture or playback stream. Dropping it stops the stream.
pub struct Opened {
    /// The device's own sample rate.
    pub rate: u32,
    pub keepalive: Box<dyn Any>,
}

/// Opens devices. Lives on the engine's I/O thread, so implementations may hold thread-bound
/// handles (COM objects, streams that aren't `Send`).
pub trait AudioIo {
    fn inputs(&mut self) -> Vec<DeviceInfo>;
    fn outputs(&mut self) -> Vec<DeviceInfo>;

    /// Start capturing `name`, pushing interleaved stereo frames into `prod` at the device rate.
    fn open_input(&mut self, name: &str, prod: HeapProd<f32>) -> Result<Opened, String>;

    /// Start playing `name`. The device callback reads `cons` (engine-rate stereo frames) through
    /// a drift-compensating resampler holding about `target_ms` of audio, and counts dropouts in
    /// `underruns`.
    fn open_output(
        &mut self,
        name: &str,
        cons: HeapCons<f32>,
        engine_rate: u32,
        target_ms: u32,
        underruns: Arc<AtomicU64>,
    ) -> Result<Opened, String>;
}

#[cfg(test)]
mod tests {
    use super::looks_virtual;

    #[test]
    fn recognises_virtual_cables() {
        assert!(looks_virtual("CABLE Input (VB-Audio Virtual Cable)"));
        assert!(looks_virtual("CABLE-A Output (VB-Audio Cable A)"));
        assert!(looks_virtual(
            "VoiceMeeter Aux Input (VB-Audio VoiceMeeter AUX VAIO)"
        ));
        assert!(!looks_virtual("Speakers (Realtek High Definition Audio)"));
        assert!(!looks_virtual("Microphone (Blue Yeti)"));
    }
}
