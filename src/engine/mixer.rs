//! The mixing core: one call turns a block of input audio into a block for each output bus. It is
//! pure computation (no devices, no threads), which is what makes it testable.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::denoise::Denoiser;
use super::dsp::{db_to_gain, gain_to_db, Compressor, Eq, Gate, Limiter, ParamEq, Spatial};
use super::params::{EngineParams, StripParams, BUSES, CHATMIX_RANGE_DB, HARDWARE_STRIPS, STRIPS};

/// Meters the audio thread publishes and the UI reads. Peak values are linear amplitude, stored
/// as `f32` bits in atomics so neither side ever waits on the other.
#[derive(Default)]
pub struct Levels {
    strips: [[AtomicU32; 2]; STRIPS],
    buses: [[AtomicU32; 2]; BUSES],
    comp_gr: [AtomicU32; STRIPS],
    gate_open: [AtomicBool; STRIPS],
}

fn store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

impl Levels {
    /// Post-fader peak of strip `i`, left and right.
    pub fn strip(&self, i: usize) -> [f32; 2] {
        [load(&self.strips[i][0]), load(&self.strips[i][1])]
    }

    pub fn bus(&self, i: usize) -> [f32; 2] {
        [load(&self.buses[i][0]), load(&self.buses[i][1])]
    }

    /// Compressor gain reduction of strip `i` in dB (<= 0).
    pub fn comp_gain_reduction_db(&self, i: usize) -> f32 {
        load(&self.comp_gr[i])
    }

    pub fn gate_open(&self, i: usize) -> bool {
        self.gate_open[i].load(Ordering::Relaxed)
    }
}

struct StripDsp {
    gate: Gate,
    eq: Eq,
    comp: Compressor,
    /// Created when noise suppression is switched on, dropped when it is switched off, so it
    /// starts fresh each time.
    denoiser: Option<Box<Denoiser>>,
    /// Linear gain the last block ended on; the next block ramps from here.
    gain: f32,
}

struct BusDsp {
    eq: ParamEq,
    spatial: Spatial,
    limiter: Limiter,
    gain: f32,
    delay: DelayLine,
}

/// Longest bus delay the mixer supports.
pub const MAX_BUS_DELAY_MS: f32 = 500.0;

/// Fixed-length delay for interleaved stereo. Allocated once, so the audio thread never allocates.
struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
    rate: f32,
}

impl DelayLine {
    fn new(rate: f32) -> Self {
        let frames = (rate * MAX_BUS_DELAY_MS / 1000.0) as usize + 1;
        Self {
            buf: vec![0.0; frames * 2],
            pos: 0,
            rate,
        }
    }

    fn process(&mut self, ms: f32, block: &mut [f32]) {
        let frames = self.buf.len() / 2;
        let delay =
            ((ms.clamp(0.0, MAX_BUS_DELAY_MS) * self.rate / 1000.0) as usize).min(frames - 1);
        if delay == 0 {
            // Keep the history current so turning a delay on doesn't replay stale audio.
            for pair in block.chunks_exact(2) {
                self.buf[self.pos * 2] = pair[0];
                self.buf[self.pos * 2 + 1] = pair[1];
                self.pos = (self.pos + 1) % frames;
            }
            return;
        }
        for pair in block.chunks_exact_mut(2) {
            let read = (self.pos + frames - delay) % frames;
            let (l, r) = (self.buf[read * 2], self.buf[read * 2 + 1]);
            self.buf[self.pos * 2] = pair[0];
            self.buf[self.pos * 2 + 1] = pair[1];
            pair[0] = l;
            pair[1] = r;
            self.pos = (self.pos + 1) % frames;
        }
    }
}

pub struct Mixer {
    strips: Vec<StripDsp>,
    buses: Vec<BusDsp>,
    /// How far ducking has turned the virtual strips down right now, in dB (0 or negative).
    duck_db: f32,
}

impl Mixer {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            strips: (0..STRIPS)
                .map(|_| StripDsp {
                    gate: Gate::new(sample_rate),
                    eq: Eq::new(sample_rate),
                    comp: Compressor::new(sample_rate),
                    denoiser: None,
                    gain: 0.0,
                })
                .collect(),
            buses: (0..BUSES)
                .map(|_| BusDsp {
                    eq: ParamEq::new(sample_rate),
                    spatial: Spatial::new(sample_rate),
                    limiter: Limiter::new(sample_rate),
                    gain: 0.0,
                    delay: DelayLine::new(sample_rate),
                })
                .collect(),
            duck_db: 0.0,
        }
    }

    /// Mix one block. `inputs[i]` is strip `i`'s interleaved stereo audio (processed in place);
    /// `outputs[b]` receives bus `b`. All buffers hold the same number of frames.
    pub fn process(
        &mut self,
        params: &EngineParams,
        inputs: &mut [Vec<f32>],
        outputs: &mut [Vec<f32>],
        levels: &Levels,
    ) {
        for out in outputs.iter_mut() {
            out.fill(0.0);
        }
        let any_solo = params.strips.iter().any(|s| s.solo);

        for (i, (dsp, p)) in self.strips.iter_mut().zip(&params.strips).enumerate() {
            let buf = &mut inputs[i];
            // Noise suppression comes first, so the gate and EQ work on a clean voice.
            if p.denoise {
                dsp.denoiser
                    .get_or_insert_with(|| Box::new(Denoiser::new()))
                    .process(buf);
            } else if dsp.denoiser.is_some() {
                dsp.denoiser = None;
            }
            if !params.bypass_fx {
                dsp.gate.process(&p.gate, buf);
                dsp.eq.process(&p.eq, buf);
                dsp.comp.process(&p.comp, buf);
            }
            if p.mono {
                for f in buf.chunks_exact_mut(2) {
                    let m = (f[0] + f[1]) * 0.5;
                    f[0] = m;
                    f[1] = m;
                }
            }
            let silenced = p.mute || (any_solo && !p.solo);
            let mut target = if silenced { 0.0 } else { db_to_gain(p.gain_db) };
            // The game / chat dial turns the side it isn't favouring down.
            if params.chatmix.enabled {
                let b = params.chatmix.balance.clamp(-1.0, 1.0);
                if i == params.chatmix.game {
                    target *= db_to_gain(-b.max(0.0) * CHATMIX_RANGE_DB);
                } else if i == params.chatmix.chat {
                    target *= db_to_gain(b.min(0.0) * CHATMIX_RANGE_DB);
                }
            }
            // Ducking turns the app strips down while the voice strip is talking.
            if i >= HARDWARE_STRIPS && params.duck.enabled {
                target *= db_to_gain(self.duck_db);
            }
            apply_gain_and_balance(buf, &mut dsp.gain, target, p);
            if params.duck.enabled && i == params.duck.source {
                // Judged on what the voice strip actually puts out (so muting it stops ducking).
                let peaks = peak_of(buf);
                let level_db = gain_to_db(peaks[0].max(peaks[1]));
                let goal = if level_db > params.duck.threshold_db {
                    -params.duck.reduction_db.abs()
                } else {
                    0.0
                };
                let ms = if goal < self.duck_db {
                    params.duck.attack_ms
                } else {
                    params.duck.release_ms
                };
                // One step per 10 ms block.
                let coef = 1.0 - (-10.0 / ms.max(1.0)).exp();
                self.duck_db += (goal - self.duck_db) * coef;
            } else if !params.duck.enabled {
                self.duck_db = 0.0;
            }

            let peaks = peak_of(buf);
            store(&levels.strips[i][0], peaks[0]);
            store(&levels.strips[i][1], peaks[1]);
            store(&levels.comp_gr[i], dsp.comp.gain_reduction_db());
            levels.gate_open[i].store(dsp.gate.is_open(), Ordering::Relaxed);

            for (b, send) in p.sends.iter().enumerate() {
                if *send {
                    for (o, s) in outputs[b].iter_mut().zip(buf.iter()) {
                        *o += *s;
                    }
                }
            }
        }

        for (b, (dsp, p)) in self.buses.iter_mut().zip(&params.buses).enumerate() {
            let buf = &mut outputs[b];
            let target = if p.mute { 0.0 } else { db_to_gain(p.gain_db) };
            ramp_gain(buf, &mut dsp.gain, target);
            dsp.eq.process(&p.eq, buf);
            dsp.spatial.process(p.crossfeed, p.width, buf);
            if p.limiter {
                dsp.limiter.process(p.limiter_ceiling_db, buf);
            }
            dsp.delay.process(p.delay_ms, buf);
            let peaks = peak_of(buf);
            store(&levels.buses[b][0], peaks[0]);
            store(&levels.buses[b][1], peaks[1]);
        }
    }
}

/// Ramp from `*current` to `target` linearly across the block, so a fader move or a mute click
/// is a smooth slope instead of a step (which is audible as a click or zipper noise).
fn ramp_gain(buf: &mut [f32], current: &mut f32, target: f32) {
    let frames = (buf.len() / 2).max(1);
    let step = (target - *current) / frames as f32;
    let mut g = *current;
    for f in buf.chunks_exact_mut(2) {
        g += step;
        f[0] *= g;
        f[1] *= g;
    }
    *current = target;
}

fn apply_gain_and_balance(buf: &mut [f32], current: &mut f32, target: f32, p: &StripParams) {
    // Balance: attenuate the far side, leave the near side alone, so centre is unity.
    let pan = p.pan.clamp(-1.0, 1.0);
    let (bl, br) = (1.0 - pan.max(0.0), 1.0 + pan.min(0.0));
    let frames = (buf.len() / 2).max(1);
    let step = (target - *current) / frames as f32;
    let mut g = *current;
    for f in buf.chunks_exact_mut(2) {
        g += step;
        f[0] *= g * bl;
        f[1] *= g * br;
    }
    *current = target;
}

fn peak_of(buf: &[f32]) -> [f32; 2] {
    let mut p = [0.0f32; 2];
    for f in buf.chunks_exact(2) {
        p[0] = p[0].max(f[0].abs());
        p[1] = p[1].max(f[1].abs());
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::dsp::gain_to_db;

    const FS: f32 = 48_000.0;
    const FRAMES: usize = 480;

    fn blocks(value: f32) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        (
            vec![vec![value; FRAMES * 2]; STRIPS],
            vec![vec![0.0; FRAMES * 2]; BUSES],
        )
    }

    /// A strip that is silent unless a test says otherwise.
    fn only(params: &mut EngineParams, strip: usize, sends: [bool; BUSES]) {
        for (i, s) in params.strips.iter_mut().enumerate() {
            s.sends = if i == strip { sends } else { [false; BUSES] };
        }
    }

    fn run(mixer: &mut Mixer, params: &EngineParams, ins: &mut [Vec<f32>], outs: &mut [Vec<f32>]) {
        // Two blocks so gain ramps have settled by the time we look.
        mixer.process(params, ins, outs, &Levels::default());
    }

    fn settle(mixer: &mut Mixer, params: &EngineParams, value: f32) -> (Vec<Vec<f32>>, Levels) {
        let levels = Levels::default();
        let (mut ins, mut outs) = blocks(value);
        mixer.process(params, &mut ins, &mut outs, &levels);
        let (mut ins, mut outs2) = blocks(value);
        mixer.process(params, &mut ins, &mut outs2, &levels);
        (outs2, levels)
    }

    /// Run `blocks` blocks with a 1 kHz sine of the given level on each listed strip, and return
    /// the RMS of the last block on bus A1.
    fn rms_after(params: &EngineParams, loud: &[(usize, f32)], blocks: usize) -> f32 {
        let mut m = Mixer::new(48_000.0);
        let levels = Levels::default();
        let mut ins = vec![vec![0.0f32; 960]; STRIPS];
        let mut outs = vec![vec![0.0f32; 960]; BUSES];
        let mut t = 0usize;
        for _ in 0..blocks {
            for &(strip, amp) in loud {
                for (n, f) in ins[strip].chunks_exact_mut(2).enumerate() {
                    let v = amp
                        * (2.0 * std::f32::consts::PI * 1000.0 * (t + n) as f32 / 48_000.0).sin();
                    f[0] = v;
                    f[1] = v;
                }
            }
            t += 480;
            m.process(params, &mut ins, &mut outs, &levels);
        }
        (outs[0].iter().map(|v| v * v).sum::<f32>() / outs[0].len() as f32).sqrt()
    }

    #[test]
    fn a_bus_equaliser_boosts_the_band_it_is_set_to() {
        let flat = rms_after(&EngineParams::default(), &[(0, 0.1)], 40);
        let mut params = EngineParams::default();
        params.buses[0].eq.enabled = true;
        params.buses[0].eq.bands[2].freq = 1000.0;
        params.buses[0].eq.bands[2].gain_db = 12.0;
        let boosted = rms_after(&params, &[(0, 0.1)], 40);
        let gain_db = 20.0 * (boosted / flat).log10();
        assert!((gain_db - 12.0).abs() < 1.5, "boost was {gain_db} dB");
        // A disabled equaliser changes nothing even with gains set.
        params.buses[0].eq.enabled = false;
        let off = rms_after(&params, &[(0, 0.1)], 40);
        assert!((off - flat).abs() < 1e-4);
    }

    #[test]
    fn ducking_turns_app_strips_down_while_the_voice_is_loud() {
        // Strip 0 is the voice (sent nowhere); strip 3 is an app, sent to A1.
        let mut params = EngineParams::default();
        params.strips[0].sends = [false; BUSES];
        params.strips[HARDWARE_STRIPS].sends = [true, false, false, false, false];
        let app_only = rms_after(&params, &[(HARDWARE_STRIPS, 0.2)], 150);
        params.duck.enabled = true;
        params.duck.source = 0;
        params.duck.reduction_db = 12.0;
        params.duck.threshold_db = -40.0;
        // Voice quiet: nothing ducks.
        let quiet = rms_after(&params, &[(HARDWARE_STRIPS, 0.2), (0, 0.001)], 150);
        assert!(
            (quiet - app_only).abs() / app_only < 0.05,
            "ducked with no voice"
        );
        // Voice loud: the app drops by about the set amount.
        let ducked = rms_after(&params, &[(HARDWARE_STRIPS, 0.2), (0, 0.5)], 150);
        let drop_db = 20.0 * (app_only / ducked).log10();
        assert!((drop_db - 12.0).abs() < 1.5, "dropped {drop_db} dB");
    }

    #[test]
    fn the_game_chat_dial_turns_the_other_side_down() {
        let mut params = EngineParams::default();
        // Strips 3 (chat) and 4 (game) both feed A1 with the same tone.
        for i in [3, 4] {
            params.strips[i].sends = [true, false, false, false, false];
        }
        params.strips[0].sends = [false; BUSES];
        let both = |p: &EngineParams| rms_after(p, &[(3, 0.1), (4, 0.1)], 30);
        let even = both(&params);
        params.chatmix.enabled = true;
        params.chatmix.balance = 0.0;
        assert!(
            (both(&params) - even).abs() / even < 0.01,
            "an even dial changed the mix"
        );
        // All the way to chat: the game strip is 30 dB down, so the sum is just the chat strip.
        params.chatmix.balance = 1.0;
        let chat_only = rms_after(&params, &[(3, 0.1)], 30);
        let with_game = both(&params);
        assert!(
            (with_game - chat_only).abs() / chat_only < 0.05,
            "{with_game} vs {chat_only}"
        );
    }

    #[test]
    fn width_zero_makes_mono_and_crossfeed_leaks_across() {
        let mut sp = super::super::dsp::Spatial::new(48_000.0);
        // Left only.
        let mut buf: Vec<f32> = (0..960)
            .flat_map(|i| [(i as f32 * 0.05).sin(), 0.0])
            .collect();
        let mut mono = buf.clone();
        sp.process(0.0, 0.0, &mut mono);
        assert!(mono.chunks_exact(2).all(|f| (f[0] - f[1]).abs() < 1e-6));
        let mut sp = super::super::dsp::Spatial::new(48_000.0);
        sp.process(1.0, 1.0, &mut buf);
        let right: f32 = buf.chunks_exact(2).map(|f| f[1].abs()).sum();
        assert!(right > 1.0, "nothing crossed to the right side");
        // Off and unchanged width is a no-op.
        let mut sp = super::super::dsp::Spatial::new(48_000.0);
        let mut same = vec![0.3f32, -0.2, 0.1, 0.4];
        let before = same.clone();
        sp.process(0.0, 1.0, &mut same);
        assert_eq!(same, before);
    }

    #[test]
    fn noise_suppression_leaves_silence_silent() {
        let mut params = EngineParams::default();
        params.strips[0].denoise = true;
        assert!(rms_after(&params, &[], 20) < 1e-4);
    }

    #[test]
    fn bypassing_fx_skips_the_processors() {
        let mut m = Mixer::new(48_000.0);
        let mut params = EngineParams::default();
        // A closed gate would silence the strip; bypass must let it through.
        params.strips[0].gate.enabled = true;
        params.strips[0].gate.threshold_db = 0.0;
        params.bypass_fx = true;
        let levels = Levels::default();
        let mut ins = vec![vec![0.0f32; 960]; STRIPS];
        let mut outs = vec![vec![0.0f32; 960]; BUSES];
        // Run long enough for a gate to close.
        for _ in 0..100 {
            for f in ins[0].iter_mut() {
                *f = 0.1;
            }
            m.process(&params, &mut ins, &mut outs, &levels);
        }
        assert!(
            outs[0].iter().any(|v| v.abs() > 0.05),
            "bypassed gate still muted the strip"
        );
    }

    #[test]
    fn a_bus_delay_shifts_audio_by_that_many_frames() {
        let mut d = DelayLine::new(48_000.0);
        let mut block = vec![0.0f32; 960];
        block[0] = 1.0; // left channel, frame 0
        block[1] = 0.5; // right channel, frame 0
        d.process(1.0, &mut block); // 1 ms = 48 frames
        assert_eq!(block[96], 1.0);
        assert_eq!(block[97], 0.5);
        assert_eq!(block.iter().filter(|v| **v != 0.0).count(), 2);
    }

    #[test]
    fn a_bus_delay_carries_over_between_blocks() {
        let mut d = DelayLine::new(48_000.0);
        let mut first = vec![0.0f32; 960];
        first[958] = 1.0; // last frame of the block
        d.process(1.0, &mut first);
        assert!(first.iter().all(|v| *v == 0.0));
        let mut second = vec![0.0f32; 960];
        d.process(1.0, &mut second);
        assert_eq!(second[958 + 96 - 960], 1.0);
    }

    #[test]
    fn a_strip_only_feeds_the_buses_it_is_sent_to() {
        let mut p = EngineParams::default();
        only(&mut p, 0, [true, false, false, true, false]);
        let mut m = Mixer::new(FS);
        let (outs, _) = settle(&mut m, &p, 0.25);
        assert!((outs[0][100] - 0.25).abs() < 1e-5, "A1 gets it");
        assert_eq!(outs[1][100], 0.0, "A2 does not");
        assert!((outs[3][100] - 0.25).abs() < 1e-5, "B1 gets it");
        assert_eq!(outs[4][100], 0.0);
    }

    #[test]
    fn strip_gain_in_db_scales_the_signal() {
        let mut p = EngineParams::default();
        only(&mut p, 0, [true, false, false, false, false]);
        p.strips[0].gain_db = -6.0;
        let mut m = Mixer::new(FS);
        let (outs, _) = settle(&mut m, &p, 0.5);
        assert!((gain_to_db(outs[0][100] / 0.5) + 6.0).abs() < 0.01);
    }

    #[test]
    fn mute_silences_a_strip() {
        let mut p = EngineParams::default();
        only(&mut p, 0, [true, false, false, false, false]);
        p.strips[0].mute = true;
        let mut m = Mixer::new(FS);
        let (outs, levels) = settle(&mut m, &p, 0.5);
        assert_eq!(outs[0][100], 0.0);
        assert_eq!(levels.strip(0), [0.0, 0.0]);
    }

    #[test]
    fn solo_silences_every_other_strip() {
        let mut p = EngineParams::default();
        for s in p.strips.iter_mut() {
            s.sends = [true, false, false, false, false];
        }
        p.strips[2].solo = true;
        let mut m = Mixer::new(FS);
        // Each strip carries 0.1, so with all five audible A1 would be 0.5; solo leaves one.
        let (outs, levels) = settle(&mut m, &p, 0.1);
        assert!((outs[0][100] - 0.1).abs() < 1e-5, "got {}", outs[0][100]);
        assert!(levels.strip(0)[0] == 0.0 && levels.strip(2)[0] > 0.09);
    }

    #[test]
    fn strips_sum_on_a_shared_bus() {
        let mut p = EngineParams::default();
        for s in p.strips.iter_mut().take(3) {
            s.sends = [true, false, false, false, false];
        }
        for s in p.strips.iter_mut().skip(3) {
            s.sends = [false; BUSES];
        }
        let mut m = Mixer::new(FS);
        let (outs, _) = settle(&mut m, &p, 0.1);
        assert!((outs[0][100] - 0.3).abs() < 1e-5);
    }

    #[test]
    fn balance_attenuates_only_the_far_channel() {
        let mut p = EngineParams::default();
        only(&mut p, 0, [true, false, false, false, false]);
        p.strips[0].pan = -1.0;
        let mut m = Mixer::new(FS);
        let (outs, _) = settle(&mut m, &p, 0.5);
        assert!((outs[0][100] - 0.5).abs() < 1e-5, "left untouched");
        assert!(outs[0][101].abs() < 1e-6, "right gone");

        p.strips[0].pan = 0.5;
        let mut m = Mixer::new(FS);
        let (outs, _) = settle(&mut m, &p, 0.5);
        assert!((outs[0][100] - 0.25).abs() < 1e-5, "left halved");
        assert!((outs[0][101] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn mono_sums_both_channels() {
        let mut p = EngineParams::default();
        only(&mut p, 0, [true, false, false, false, false]);
        p.strips[0].mono = true;
        let mut m = Mixer::new(FS);
        let levels = Levels::default();
        let (mut ins, mut outs) = blocks(0.0);
        for f in ins[0].chunks_exact_mut(2) {
            f[0] = 0.8;
            f[1] = 0.0;
        }
        run(&mut m, &p, &mut ins, &mut outs);
        let _ = levels;
        // After the first block's ramp-in, both channels carry the average.
        assert!((outs[0][FRAMES * 2 - 2] - 0.4).abs() < 1e-3);
        assert!((outs[0][FRAMES * 2 - 1] - 0.4).abs() < 1e-3);
    }

    #[test]
    fn the_bus_limiter_stops_a_hot_mix_from_clipping() {
        let mut p = EngineParams::default();
        for s in p.strips.iter_mut() {
            s.sends = [true, false, false, false, false];
        }
        p.buses[0].limiter = true;
        p.buses[0].limiter_ceiling_db = -1.0;
        let mut m = Mixer::new(FS);
        // Five strips at 0.5 would sum to 2.5 on A1.
        let (outs, _) = settle(&mut m, &p, 0.5);
        let ceiling = db_to_gain(-1.0);
        assert!(outs[0].iter().all(|v| v.abs() <= ceiling + 1e-4));
    }

    #[test]
    fn a_gain_change_ramps_instead_of_stepping() {
        let mut p = EngineParams::default();
        only(&mut p, 0, [true, false, false, false, false]);
        let mut m = Mixer::new(FS);
        let levels = Levels::default();
        let (mut ins, mut outs) = blocks(0.5);
        m.process(&p, &mut ins, &mut outs, &levels);
        let last_of_first = outs[0][FRAMES * 2 - 2];
        p.strips[0].gain_db = -20.0;
        let (mut ins, mut outs) = blocks(0.5);
        m.process(&p, &mut ins, &mut outs, &levels);
        let first_of_second = outs[0][0];
        // A step from 0.5 to 0.05 would be 0.45; a ramp over 480 frames moves ~0.001 per frame.
        assert!(
            (last_of_first - first_of_second).abs() < 0.01,
            "jump {}",
            (last_of_first - first_of_second).abs()
        );
        let end = outs[0][FRAMES * 2 - 2];
        assert!(
            (end - 0.05).abs() < 1e-3,
            "settles at the target, got {end}"
        );
    }

    #[test]
    fn meters_report_post_fader_peaks() {
        let mut p = EngineParams::default();
        only(&mut p, 1, [false, true, false, false, false]);
        p.strips[1].gain_db = -6.0206; // exactly half
        let mut m = Mixer::new(FS);
        let (_, levels) = settle(&mut m, &p, 0.8);
        assert!((levels.strip(1)[0] - 0.4).abs() < 1e-3);
        assert!((levels.bus(1)[0] - 0.4).abs() < 1e-3);
        assert_eq!(levels.bus(0), [0.0, 0.0]);
    }
}
