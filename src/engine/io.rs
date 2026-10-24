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

/// The part of a device name inside the last pair of parentheses: for VB-Cable, the cable's own
/// name, which both ends share ("CABLE Input (VB-Audio Virtual Cable)" and "CABLE Output
/// (VB-Audio Virtual Cable)").
fn cable_key(name: &str) -> Option<&str> {
    let open = name.rfind('(')?;
    let close = name.rfind(')')?;
    (close > open + 1).then(|| name[open + 1..close].trim())
}

/// True when `input` is the recording end of the same virtual cable that `output` plays into.
/// Routing a strip that listens to that input onto that output would feed the mix back into
/// itself, which howls.
pub fn same_cable(input: &str, output: &str) -> bool {
    looks_virtual(input)
        && looks_virtual(output)
        && matches!((cable_key(input), cable_key(output)), (Some(a), Some(b)) if a == b)
}

/// A running capture or playback stream. Dropping it stops the stream.
pub struct Opened {
    /// The device's own sample rate.
    pub rate: u32,
    /// Held only to keep the stream running; dropping it stops the device.
    #[allow(dead_code)]
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
    use super::{looks_virtual, same_cable};

    #[test]
    fn a_cables_two_ends_form_a_loop_but_other_pairs_do_not() {
        let out_a = "CABLE Output (VB-Audio Virtual Cable)";
        let in_a = "CABLE Input (VB-Audio Virtual Cable)";
        let out_b = "CABLE-A Output (VB-Audio Cable A)";
        let in_b = "CABLE-A Input (VB-Audio Cable A)";
        assert!(same_cable(out_a, in_a));
        assert!(same_cable(out_b, in_b));
        assert!(!same_cable(out_a, in_b), "different cables are independent");
        assert!(!same_cable(
            "Microphone (Blue Yeti)",
            "Speakers (Blue Yeti)"
        ));
        assert!(!same_cable(out_a, "Speakers (Realtek)"));
        assert!(
            !same_cable("CABLE Output", "CABLE Input"),
            "no key, no claim"
        );
    }

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
