//! Translating a Voicemeeter configuration into wavemix settings.
//!
//! Voicemeeter comes in three editions with different channel counts. wavemix has the layout of
//! Banana (3 hardware + 2 virtual strips, A1-A3 and B1-B2 buses), so the mapping is by role:
//! hardware strips to hardware strips, virtual to virtual, A buses to A, B buses to B. Whatever
//! doesn't fit is reported rather than silently dropped.

use crate::engine::io::DeviceInfo;
use crate::engine::params::{
    CompParams, EngineParams, EqParams, GateParams, IoConfig, BUSES, GAIN_MAX_DB, GAIN_MIN_DB,
    HARDWARE_STRIPS, STRIPS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Edition {
    Standard,
    Banana,
    Potato,
}

impl Edition {
    pub fn name(self) -> &'static str {
        match self {
            Edition::Standard => "Voicemeeter",
            Edition::Banana => "Voicemeeter Banana",
            Edition::Potato => "Voicemeeter Potato",
        }
    }
}

/// One Voicemeeter input strip, as read from its Remote API.
#[derive(Clone, Debug, PartialEq)]
pub struct VmStrip {
    pub label: String,
    /// True for the virtual inputs (VAIO / AUX), false for hardware inputs.
    pub is_virtual: bool,
    /// The hardware device chosen for it, if any (never set for virtual strips).
    pub device: Option<String>,
    pub gain_db: f32,
    pub mute: bool,
    pub solo: bool,
    pub mono: bool,
    /// Balance as Voicemeeter reports it: -0.5 (left) ..= 0.5 (right).
    pub pan_x: f32,
    /// Sends to A1.. and B1..; shorter than 5 / 3 for editions with fewer buses.
    pub sends_a: Vec<bool>,
    pub sends_b: Vec<bool>,
    /// Bass, mid and treble in dB.
    pub eq: [f32; 3],
    /// The compressor and gate knobs, 0 (off) to 10.
    pub comp: f32,
    pub gate: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VmBus {
    pub label: String,
    /// B buses are the virtual outputs; A buses are hardware.
    pub is_b: bool,
    pub device: Option<String>,
    pub gain_db: f32,
    pub mute: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VmState {
    pub edition: Edition,
    pub strips: Vec<VmStrip>,
    pub buses: Vec<VmBus>,
}

/// What switching over will do, worked out before anything is touched.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportPlan {
    pub params: EngineParams,
    pub io: IoConfig,
    /// Playback device to make the Windows default, if the current default is one of
    /// Voicemeeter's own virtual devices (so audio keeps playing once Voicemeeter closes).
    pub default_playback: Option<String>,
    /// Same for the default recording device.
    pub default_recording: Option<String>,
    /// Plain-language notes: what mapped, and what couldn't.
    pub notes: Vec<String>,
}

/// A made-up Banana setup, so the switch flow can be tried (and screenshotted) without Voicemeeter.
#[cfg_attr(windows, allow(dead_code))]
pub fn demo_state() -> VmState {
    let strip = |is_virtual: bool, label: &str, device: Option<&str>| VmStrip {
        label: label.into(),
        is_virtual,
        device: device.map(str::to_string),
        gain_db: 0.0,
        mute: false,
        solo: false,
        mono: false,
        pan_x: 0.0,
        sends_a: vec![false; 3],
        sends_b: vec![false; 2],
        eq: [0.0; 3],
        comp: 0.0,
        gate: 0.0,
    };
    let mut mic = strip(false, "Mic", Some("wdm: Microphone (Blue Yeti)"));
    mic.gain_db = -3.0;
    mic.sends_b = vec![true, false];
    mic.comp = 4.0;
    mic.gate = 3.0;
    mic.eq = [2.0, 0.0, 1.5];
    let mut line = strip(false, "Line", Some("wdm: Line In (Realtek Audio)"));
    line.mute = true;
    let mut music = strip(true, "Music", None);
    music.sends_a = vec![true, false, false];
    let bus = |is_b: bool, device: Option<&str>| VmBus {
        label: String::new(),
        is_b,
        device: device.map(str::to_string),
        gain_db: 0.0,
        mute: false,
    };
    VmState {
        edition: Edition::Banana,
        strips: vec![
            mic,
            line,
            strip(false, "", None),
            music,
            strip(true, "", None),
        ],
        buses: vec![
            bus(false, Some("wdm: Speakers (Realtek Audio)")),
            bus(false, None),
            bus(false, None),
            bus(true, None),
            bus(true, None),
        ],
    }
}

/// True for a device Voicemeeter itself provides (its VAIO inputs and its outputs).
pub fn is_voicemeeter_device(name: &str) -> bool {
    name.to_lowercase().contains("voicemeeter")
}

/// Voicemeeter prefixes device names with the driver it uses ("wdm: ", "mme: ", ...).
fn strip_driver(name: &str) -> &str {
    let lower = name.to_lowercase();
    for prefix in ["wdm: ", "mme: ", "ks: ", "asio: ", "wasapi: "] {
        if lower.starts_with(prefix) {
            return &name[prefix.len()..];
        }
    }
    name
}

/// Find the device a Voicemeeter name refers to. The older MME driver truncates names to 31
/// characters, so a prefix counts as a match.
fn find_device<'a>(wanted: &str, list: &'a [DeviceInfo]) -> Option<&'a DeviceInfo> {
    let wanted = strip_driver(wanted).trim().to_lowercase();
    if wanted.is_empty() {
        return None;
    }
    let candidates = || list.iter().filter(|d| !d.is_loopback);
    candidates()
        .find(|d| d.name.to_lowercase() == wanted)
        .or_else(|| {
            candidates()
                .filter(|d| {
                    let n = d.name.to_lowercase();
                    wanted.len() >= 4 && (n.starts_with(&wanted) || wanted.starts_with(&n))
                })
                .max_by_key(|d| d.is_default)
        })
}

/// Voicemeeter's compressor knob (0-10) as a compressor: more knob, lower threshold and higher
/// ratio. An approximation: Voicemeeter's own curve isn't published.
pub fn comp_from_knob(knob: f32) -> CompParams {
    let k = knob.clamp(0.0, 10.0);
    CompParams {
        enabled: k > 0.05,
        threshold_db: -10.0 - 3.0 * k,
        ratio: 1.5 + 0.55 * k,
        attack_ms: 10.0,
        release_ms: 150.0,
        makeup_db: 0.0,
    }
}

/// Voicemeeter's gate knob (0-10): more knob, higher threshold.
pub fn gate_from_knob(knob: f32) -> GateParams {
    let k = knob.clamp(0.0, 10.0);
    GateParams {
        enabled: k > 0.05,
        threshold_db: -70.0 + 5.0 * k,
        attack_ms: 2.0,
        release_ms: 150.0,
    }
}

/// Which of our slots a Voicemeeter strip lands in: hardware strips fill our hardware slots in
/// order, virtual strips our virtual ones. `None` when there's no room.
fn strip_slots(strips: &[VmStrip]) -> Vec<Option<usize>> {
    let (mut hw, mut virt) = (0, 0);
    strips
        .iter()
        .map(|s| {
            if s.is_virtual {
                virt += 1;
                (virt <= STRIPS - HARDWARE_STRIPS).then(|| HARDWARE_STRIPS + virt - 1)
            } else {
                hw += 1;
                (hw <= HARDWARE_STRIPS).then(|| hw - 1)
            }
        })
        .collect()
}

/// Our bus for a Voicemeeter bus: A buses to A1-A3, B buses to B1-B2.
fn bus_slots(buses: &[VmBus]) -> Vec<Option<usize>> {
    let (mut a, mut b) = (0, 0);
    buses
        .iter()
        .map(|bus| {
            if bus.is_b {
                b += 1;
                (b <= BUSES - 3).then(|| 3 + b - 1)
            } else {
                a += 1;
                (a <= 3).then(|| a - 1)
            }
        })
        .collect()
}

/// Work out the import. `inputs` and `outputs` are the devices wavemix can see, `current` the
/// devices it already uses, and `defaults` the names of Windows' current default
/// playback / recording devices.
pub fn plan_import(
    state: &VmState,
    inputs: &[DeviceInfo],
    outputs: &[DeviceInfo],
    current: &IoConfig,
    default_playback_name: Option<&str>,
    default_recording_name: Option<&str>,
) -> ImportPlan {
    let mut params = EngineParams::default();
    let mut io = current.clone();
    let mut notes = vec![format!("Read your {} setup.", state.edition.name())];

    // --- strips ---
    let slots = strip_slots(&state.strips);
    let mut dropped_strips = 0;
    for (strip, slot) in state.strips.iter().zip(&slots) {
        let Some(i) = *slot else {
            dropped_strips += 1;
            continue;
        };
        let p = &mut params.strips[i];
        p.gain_db = strip.gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        p.mute = strip.mute;
        p.solo = strip.solo;
        p.mono = strip.mono;
        p.pan = (strip.pan_x * 2.0).clamp(-1.0, 1.0);
        p.sends = [false; BUSES];
        // A1-A3 map straight across; B1-B2 follow them.
        for (k, on) in strip.sends_a.iter().take(3).enumerate() {
            p.sends[k] = *on;
        }
        for (k, on) in strip.sends_b.iter().take(2).enumerate() {
            p.sends[3 + k] = *on;
        }
        let [bass, mid, treble] = strip.eq;
        p.eq = EqParams {
            enabled: bass != 0.0 || mid != 0.0 || treble != 0.0,
            // Voicemeeter's bass / mid / treble sit at roughly 100 Hz, 1 kHz and 10 kHz.
            low_freq: 100.0,
            low_gain_db: bass.clamp(-18.0, 18.0),
            mid_freq: 1000.0,
            mid_gain_db: mid.clamp(-18.0, 18.0),
            mid_q: 1.0,
            high_freq: 10_000.0,
            high_gain_db: treble.clamp(-18.0, 18.0),
        };
        p.comp = comp_from_knob(strip.comp);
        p.gate = gate_from_knob(strip.gate);

        if !strip.is_virtual {
            if let Some(wanted) = strip.device.as_deref().filter(|d| !d.trim().is_empty()) {
                match find_device(wanted, inputs) {
                    Some(d) => io.inputs[i] = Some(d.name.clone()),
                    None => notes.push(format!(
                        "Input {}: couldn't find \"{}\" here; choose its device in the mixer.",
                        i + 1,
                        strip_driver(wanted)
                    )),
                }
            }
        }
    }
    if dropped_strips > 0 {
        notes.push(format!(
            "{dropped_strips} of your strips don't fit (Wavemix has 3 hardware and 2 virtual inputs) and were left out."
        ));
    }
    if state.strips.iter().any(|s| s.is_virtual) {
        notes.push(
            "Voicemeeter's virtual inputs (VAIO) can't be captured by other programs. wavemix's \
             Virtual inputs use a virtual cable such as VB-Cable: send an app to the cable, and \
             it shows up there."
                .to_string(),
        );
    }

    // --- buses ---
    let bus_map = bus_slots(&state.buses);
    let mut dropped_buses = 0;
    for (bus, slot) in state.buses.iter().zip(&bus_map) {
        let Some(b) = *slot else {
            dropped_buses += 1;
            continue;
        };
        let p = &mut params.buses[b];
        p.gain_db = bus.gain_db.clamp(GAIN_MIN_DB, GAIN_MAX_DB);
        p.mute = bus.mute;
        if !bus.is_b {
            if let Some(wanted) = bus.device.as_deref().filter(|d| !d.trim().is_empty()) {
                match find_device(wanted, outputs) {
                    Some(d) => io.outputs[b] = Some(d.name.clone()),
                    None => notes.push(format!(
                        "Output {}: couldn't find \"{}\" here; choose its device in the mixer.",
                        crate::engine::params::bus_name(b),
                        strip_driver(wanted)
                    )),
                }
            }
        }
    }
    if dropped_buses > 0 {
        notes.push(format!(
            "{dropped_buses} of your output buses don't fit (Wavemix has A1-A3 and B1-B2) and were left out."
        ));
    }
    if state.buses.iter().any(|b| b.is_b) {
        notes.push(
            "Voicemeeter's B outputs are virtual. wavemix's B1 / B2 play into a virtual cable: \
             programs that recorded from \"VoiceMeeter Output\" should record from the cable's \
             recording end instead."
                .to_string(),
        );
    }
    notes.push(
        "Imported: gain, mute, solo, mono, balance, routing, bass/mid/treble, compressor and gate \
         (approximated from Voicemeeter's knobs). Not imported: bus EQ, delays, macros, 3D."
            .to_string(),
    );

    // --- autoswap: don't leave Windows pointing at devices that are about to disappear ---
    let a1 = io.outputs[0].clone();
    let default_playback = match (default_playback_name, &a1) {
        (Some(d), Some(a1)) if is_voicemeeter_device(d) => {
            notes.push(format!(
                "Windows plays sound to Voicemeeter's virtual device; it will switch to \"{a1}\" so audio carries on."
            ));
            Some(a1.clone())
        }
        (Some(d), None) if is_voicemeeter_device(d) => {
            // No A1 device was named: fall back to the first real speaker.
            let fallback = outputs
                .iter()
                .find(|o| !o.is_virtual && !is_voicemeeter_device(&o.name))
                .map(|o| o.name.clone());
            if let Some(f) = &fallback {
                notes.push(format!(
                    "Windows plays sound to Voicemeeter's virtual device; it will switch to \"{f}\"."
                ));
            }
            fallback
        }
        _ => None,
    };
    let default_recording = match default_recording_name {
        Some(d) if is_voicemeeter_device(d) => {
            // Prefer the cable carrying wavemix's mix; failing that, the real microphone.
            let cable = inputs
                .iter()
                .find(|i| {
                    i.is_virtual
                        && !is_voicemeeter_device(&i.name)
                        && i.name.contains("CABLE Output")
                })
                .map(|i| i.name.clone());
            let target = cable.or_else(|| io.inputs[0].clone());
            if let Some(t) = &target {
                notes.push(format!(
                    "Windows records from Voicemeeter's virtual output; it will switch to \"{t}\"."
                ));
            }
            target
        }
        _ => None,
    };

    // Virtual strips: if a cable exists, listen to it.
    if io.inputs[HARDWARE_STRIPS].is_none() {
        io.inputs[HARDWARE_STRIPS] = inputs
            .iter()
            .find(|i| {
                i.is_virtual && !is_voicemeeter_device(&i.name) && i.name.contains("CABLE Output")
            })
            .map(|i| i.name.clone());
    }
    if io.outputs[3].is_none() {
        io.outputs[3] = outputs
            .iter()
            .find(|o| {
                o.is_virtual && !is_voicemeeter_device(&o.name) && o.name.contains("CABLE Input")
            })
            .map(|o| o.name.clone());
    }

    ImportPlan {
        params,
        io,
        default_playback,
        default_recording,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, virt: bool, default: bool) -> DeviceInfo {
        DeviceInfo {
            name: name.into(),
            is_default: default,
            is_virtual: virt,
            is_loopback: false,
        }
    }

    fn strip(is_virtual: bool, device: Option<&str>) -> VmStrip {
        VmStrip {
            label: String::new(),
            is_virtual,
            device: device.map(str::to_string),
            gain_db: 0.0,
            mute: false,
            solo: false,
            mono: false,
            pan_x: 0.0,
            sends_a: vec![false; 3],
            sends_b: vec![false; 2],
            eq: [0.0; 3],
            comp: 0.0,
            gate: 0.0,
        }
    }

    fn bus(is_b: bool, device: Option<&str>) -> VmBus {
        VmBus {
            label: String::new(),
            is_b,
            device: device.map(str::to_string),
            gain_db: 0.0,
            mute: false,
        }
    }

    fn banana() -> VmState {
        VmState {
            edition: Edition::Banana,
            strips: vec![
                strip(false, Some("wdm: Microphone (Blue Yeti)")),
                strip(false, None),
                strip(false, None),
                strip(true, None),
                strip(true, None),
            ],
            buses: vec![
                bus(false, Some("wdm: Speakers (Realtek High Definition Audio)")),
                bus(false, None),
                bus(false, None),
                bus(true, None),
                bus(true, None),
            ],
        }
    }

    fn inputs() -> Vec<DeviceInfo> {
        vec![
            dev("Microphone (Blue Yeti)", false, true),
            dev("CABLE Output (VB-Audio Virtual Cable)", true, false),
            dev(
                "VoiceMeeter Output (VB-Audio VoiceMeeter VAIO)",
                true,
                false,
            ),
        ]
    }

    fn outputs() -> Vec<DeviceInfo> {
        vec![
            dev("Speakers (Realtek High Definition Audio)", false, false),
            dev("CABLE Input (VB-Audio Virtual Cable)", true, false),
            dev("VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)", true, true),
        ]
    }

    fn plan(state: &VmState) -> ImportPlan {
        plan_import(
            state,
            &inputs(),
            &outputs(),
            &IoConfig::default(),
            Some("VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"),
            Some("VoiceMeeter Output (VB-Audio VoiceMeeter VAIO)"),
        )
    }

    #[test]
    fn faders_mutes_and_routing_carry_over() {
        let mut s = banana();
        s.strips[0].gain_db = -6.5;
        s.strips[0].mute = true;
        s.strips[0].sends_a = vec![true, false, true];
        s.strips[0].sends_b = vec![false, true];
        s.strips[3].solo = true;
        s.buses[0].gain_db = -3.0;
        s.buses[3].mute = true;
        let p = plan(&s);
        let a = &p.params.strips[0];
        assert_eq!(a.gain_db, -6.5);
        assert!(a.mute);
        assert_eq!(a.sends, [true, false, true, false, true]);
        assert!(
            p.params.strips[3].solo,
            "virtual strip 1 lands in our Virtual 1"
        );
        assert_eq!(p.params.buses[0].gain_db, -3.0);
        assert!(p.params.buses[3].mute, "first B bus is our B1");
    }

    #[test]
    fn balance_is_scaled_to_our_range() {
        let mut s = banana();
        s.strips[1].pan_x = -0.25;
        s.strips[2].pan_x = 0.5;
        let p = plan(&s);
        assert_eq!(p.params.strips[1].pan, -0.5);
        assert_eq!(p.params.strips[2].pan, 1.0);
    }

    #[test]
    fn hardware_devices_are_matched_by_name_dropping_the_driver_prefix() {
        let p = plan(&banana());
        assert_eq!(p.io.inputs[0].as_deref(), Some("Microphone (Blue Yeti)"));
        assert_eq!(
            p.io.outputs[0].as_deref(),
            Some("Speakers (Realtek High Definition Audio)")
        );
    }

    #[test]
    fn names_truncated_by_the_old_mme_driver_still_match() {
        let mut s = banana();
        s.strips[0].device = Some("mme: Microphone (Blue Yeti".into());
        let p = plan(&s);
        assert_eq!(p.io.inputs[0].as_deref(), Some("Microphone (Blue Yeti)"));
    }

    #[test]
    fn an_unknown_device_is_reported_and_left_unset() {
        let mut s = banana();
        s.strips[1].device = Some("wdm: Gone Mic (Old USB)".into());
        let p = plan(&s);
        assert_eq!(p.io.inputs[1], None);
        assert!(
            p.notes.iter().any(|n| n.contains("Gone Mic")),
            "{:?}",
            p.notes
        );
    }

    #[test]
    fn knobs_become_eq_compressor_and_gate() {
        let mut s = banana();
        s.strips[0].eq = [3.0, 0.0, -2.0];
        s.strips[0].comp = 5.0;
        s.strips[0].gate = 4.0;
        let p = plan(&s);
        let st = &p.params.strips[0];
        assert!(st.eq.enabled);
        assert_eq!((st.eq.low_gain_db, st.eq.high_gain_db), (3.0, -2.0));
        assert!(st.comp.enabled && st.comp.ratio > 3.0 && st.comp.threshold_db < -20.0);
        assert!(st.gate.enabled && st.gate.threshold_db > -60.0);
        // Untouched strips keep everything off.
        let other = &p.params.strips[1];
        assert!(!other.eq.enabled && !other.comp.enabled && !other.gate.enabled);
    }

    #[test]
    fn stronger_knobs_compress_harder() {
        let (lo, hi) = (comp_from_knob(2.0), comp_from_knob(8.0));
        assert!(hi.ratio > lo.ratio && hi.threshold_db < lo.threshold_db);
        assert!(!comp_from_knob(0.0).enabled);
        assert!(!gate_from_knob(0.0).enabled);
    }

    #[test]
    fn standard_voicemeeter_maps_its_single_a_and_b() {
        let mut s = VmState {
            edition: Edition::Standard,
            strips: vec![strip(false, None), strip(false, None), strip(true, None)],
            buses: vec![bus(false, None), bus(true, None)],
        };
        s.strips[0].sends_a = vec![true];
        s.strips[0].sends_b = vec![true];
        let p = plan(&s);
        assert_eq!(p.params.strips[0].sends, [true, false, false, true, false]);
        assert!(!p.params.strips[2].mute);
        // The lone virtual strip becomes our Virtual 1.
        assert_eq!(p.params.strips[3].sends, [false; 5]);
    }

    #[test]
    fn potato_has_more_than_fits_and_says_so() {
        let mut s = VmState {
            edition: Edition::Potato,
            strips: (0..5)
                .map(|_| strip(false, None))
                .chain((0..3).map(|_| strip(true, None)))
                .collect(),
            buses: (0..5)
                .map(|_| bus(false, None))
                .chain((0..3).map(|_| bus(true, None)))
                .collect(),
        };
        s.strips[4].gain_db = -9.0; // fifth hardware strip: no room
        s.buses[4].gain_db = -9.0; // A5: no room
        let p = plan(&s);
        assert!(p.params.strips.iter().all(|st| st.gain_db != -9.0));
        assert!(p.params.buses.iter().all(|b| b.gain_db != -9.0));
        assert!(p.notes.iter().any(|n| n.contains("strips don't fit")));
        assert!(p.notes.iter().any(|n| n.contains("buses don't fit")));
    }

    #[test]
    fn virtual_strips_and_b_buses_use_a_cable_when_one_is_installed() {
        let p = plan(&banana());
        assert_eq!(
            p.io.inputs[HARDWARE_STRIPS].as_deref(),
            Some("CABLE Output (VB-Audio Virtual Cable)")
        );
        assert_eq!(
            p.io.outputs[3].as_deref(),
            Some("CABLE Input (VB-Audio Virtual Cable)")
        );
        assert!(p.notes.iter().any(|n| n.contains("VB-Cable")));
    }

    #[test]
    fn windows_defaults_on_voicemeeter_devices_are_swapped_for_real_ones() {
        let p = plan(&banana());
        assert_eq!(
            p.default_playback.as_deref(),
            Some("Speakers (Realtek High Definition Audio)")
        );
        assert_eq!(
            p.default_recording.as_deref(),
            Some("CABLE Output (VB-Audio Virtual Cable)"),
            "the cable carries Wavemix's mix, like Voicemeeter's B1 did"
        );
    }

    #[test]
    fn defaults_that_are_already_real_devices_are_left_alone() {
        let p = plan_import(
            &banana(),
            &inputs(),
            &outputs(),
            &IoConfig::default(),
            Some("Speakers (Realtek High Definition Audio)"),
            Some("Microphone (Blue Yeti)"),
        );
        assert_eq!(p.default_playback, None);
        assert_eq!(p.default_recording, None);
    }

    #[test]
    fn without_a_cable_the_default_recording_falls_back_to_the_microphone() {
        let no_cable: Vec<DeviceInfo> = inputs()
            .into_iter()
            .filter(|d| !d.name.contains("CABLE"))
            .collect();
        let p = plan_import(
            &banana(),
            &no_cable,
            &outputs(),
            &IoConfig::default(),
            None,
            Some("VoiceMeeter Output (VB-Audio VoiceMeeter VAIO)"),
        );
        assert_eq!(
            p.default_recording.as_deref(),
            Some("Microphone (Blue Yeti)")
        );
    }

    #[test]
    fn existing_assignments_survive_where_voicemeeter_had_no_opinion() {
        let mut current = IoConfig::default();
        current.inputs[4] = Some("System audio: Speakers (Realtek High Definition Audio)".into());
        let p = plan_import(&banana(), &inputs(), &outputs(), &current, None, None);
        assert_eq!(
            p.io.inputs[4].as_deref(),
            Some("System audio: Speakers (Realtek High Definition Audio)")
        );
    }

    #[test]
    fn recognises_voicemeeters_own_devices() {
        assert!(is_voicemeeter_device(
            "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"
        ));
        assert!(is_voicemeeter_device(
            "VoiceMeeter Aux Output (VB-Audio VoiceMeeter AUX VAIO)"
        ));
        assert!(!is_voicemeeter_device(
            "CABLE Input (VB-Audio Virtual Cable)"
        ));
        assert!(!is_voicemeeter_device("Speakers (Realtek)"));
    }
}
