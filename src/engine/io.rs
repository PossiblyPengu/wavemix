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
    /// "What you hear": captures everything an output device is playing, rather than recording
    /// from a microphone.
    pub is_loopback: bool,
}

/// Loopback inputs are named after the output they capture, behind this prefix.
pub const LOOPBACK_PREFIX: &str = "System audio: ";

/// The input name for capturing what `output` plays.
pub fn loopback_name(output: &str) -> String {
    format!("{LOOPBACK_PREFIX}{output}")
}

/// The output device a loopback input name refers to.
pub fn loopback_target(input: &str) -> Option<&str> {
    input.strip_prefix(LOOPBACK_PREFIX)
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

/// True when a VBAN sender (`output` device name) and receiver (`input` device name) are on the
/// same machine (localhost) on the same port and stream — routing audio through such a pair loops
/// it back into itself.
fn same_vban_loopback(input: &str, output: &str) -> bool {
    use crate::engine::vban::{IN_PREFIX, OUT_PREFIX};
    let Some(in_rest) = input.strip_prefix(IN_PREFIX) else {
        return false;
    };
    let Some(out_rest) = output.strip_prefix(OUT_PREFIX) else {
        return false;
    };
    // Output format: "<stream> to <host>:<port>"
    // Input format:  "<stream> on port <port>"  OR  "any stream on port <port>"
    let Some((out_stream, out_addr)) = out_rest.split_once(" to ") else {
        return false;
    };
    let Some((out_host, out_port)) = out_addr.rsplit_once(':') else {
        return false;
    };
    // Only loopback addresses loop on the same machine.
    if out_host != "127.0.0.1" && out_host != "localhost" && out_host != "::1" {
        return false;
    }
    // Input: "any stream on port P" matches any stream on that port.
    if in_rest == format!("any stream on port {out_port}") {
        return true;
    }
    // Input: "<stream> on port P" must match both stream name and port.
    if let Some((in_stream, in_port_part)) = in_rest.split_once(" on port ") {
        return in_stream == out_stream && in_port_part == out_port;
    }
    false
}

/// True when sending a strip that listens to `input` onto the bus playing to `output` would feed
/// the mix back into itself: either the two ends of one virtual cable, a "what you hear" capture
/// of the very device the bus plays to, or a VBAN loopback on the same port and stream.
pub fn feeds_itself(input: &str, output: &str) -> bool {
    loopback_target(input) == Some(output)
        || same_cable(input, output)
        || same_vban_loopback(input, output)
}

/// A running capture or playback stream. Dropping it stops the stream.
pub struct Opened {
    /// The device's own sample rate.
    pub rate: u32,
    /// The source stops sending data while it is silent (loopback), so starvation isn't a fault.
    pub loopback: bool,
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
    use super::{feeds_itself, looks_virtual, loopback_name, loopback_target, same_cable};

    #[test]
    fn loopback_names_round_trip() {
        let n = loopback_name("Speakers (Realtek)");
        assert_eq!(n, "System audio: Speakers (Realtek)");
        assert_eq!(loopback_target(&n), Some("Speakers (Realtek)"));
        assert_eq!(loopback_target("Microphone (Yeti)"), None);
    }

    #[test]
    fn capturing_what_a_bus_plays_is_a_loop() {
        let speakers = "Speakers (Realtek)";
        assert!(feeds_itself(&loopback_name(speakers), speakers));
        assert!(!feeds_itself(&loopback_name(speakers), "Headphones (USB)"));
        assert!(feeds_itself(
            "CABLE Output (VB-Audio Virtual Cable)",
            "CABLE Input (VB-Audio Virtual Cable)"
        ));
        assert!(!feeds_itself("Microphone (Yeti)", speakers));
    }

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
    fn vban_loopback_on_localhost_is_a_loop() {
        let rx = "VBAN in: MyStream on port 6980";
        let tx = "VBAN out: MyStream to 127.0.0.1:6980";
        assert!(feeds_itself(rx, tx));
        // Different stream name: not a loop.
        assert!(!feeds_itself("VBAN in: Other on port 6980", tx));
        // Different port: not a loop.
        assert!(!feeds_itself("VBAN in: MyStream on port 6981", tx));
        // Remote host: not a loop.
        assert!(!feeds_itself(rx, "VBAN out: MyStream to 192.168.1.5:6980"));
        // "any stream" receiver catches all streams on the same port.
        assert!(feeds_itself("VBAN in: any stream on port 6980", tx));
        assert!(!feeds_itself("VBAN in: any stream on port 6981", tx));
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
