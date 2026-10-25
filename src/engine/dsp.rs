//! Per-strip signal processing: 3-band EQ, noise gate, compressor and bus limiter. All of it works
//! on interleaved stereo `f32` blocks and allocates nothing while processing.

use std::f32::consts::PI;

use super::params::{
    BandKind, BandParams, CompParams, EqParams, GateParams, ParamEqParams, BUS_EQ_BANDS,
};

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub fn gain_to_db(g: f32) -> f32 {
    20.0 * g.max(1e-9).log10()
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
fn time_coef(ms: f32, fs: f32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * fs)).exp()
    }
}

// --- biquad EQ ----------------------------------------------------------------------------------

/// Normalised biquad coefficients (a0 = 1), from the RBJ audio-EQ cookbook.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coeffs {
    pub const IDENTITY: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalise(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Coeffs {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    pub fn peaking(fs: f32, f0: f32, gain_db: f32, q: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0.clamp(10.0, fs * 0.45) / fs;
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let c = w0.cos();
        Self::normalise(
            1.0 + alpha * a,
            -2.0 * c,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * c,
            1.0 - alpha / a,
        )
    }

    pub fn low_shelf(fs: f32, f0: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0.clamp(10.0, fs * 0.45) / fs;
        let (s, c) = (w0.sin(), w0.cos());
        // Shelf slope S = 1.
        let alpha = s / 2.0 * 2f32.sqrt();
        let t = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) - (a - 1.0) * c + t),
            2.0 * a * ((a - 1.0) - (a + 1.0) * c),
            a * ((a + 1.0) - (a - 1.0) * c - t),
            (a + 1.0) + (a - 1.0) * c + t,
            -2.0 * ((a - 1.0) + (a + 1.0) * c),
            (a + 1.0) + (a - 1.0) * c - t,
        )
    }

    pub fn high_shelf(fs: f32, f0: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f0.clamp(10.0, fs * 0.45) / fs;
        let (s, c) = (w0.sin(), w0.cos());
        let alpha = s / 2.0 * 2f32.sqrt();
        let t = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) + (a - 1.0) * c + t),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
            a * ((a + 1.0) + (a - 1.0) * c - t),
            (a + 1.0) - (a - 1.0) * c + t,
            2.0 * ((a - 1.0) - (a + 1.0) * c),
            (a + 1.0) - (a - 1.0) * c - t,
        )
    }

    /// Second-order high-pass at `f0` (Butterworth-like at q = 0.707).
    pub fn high_pass(fs: f32, f0: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * f0.clamp(10.0, fs * 0.45) / fs;
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let c = w0.cos();
        Self::normalise(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }

    pub fn low_pass(fs: f32, f0: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * f0.clamp(10.0, fs * 0.45) / fs;
        let alpha = w0.sin() / (2.0 * q.max(0.1));
        let c = w0.cos();
        Self::normalise(
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }

    /// Magnitude response in dB at `freq` Hz.
    pub fn magnitude_db(&self, fs: f32, freq: f32) -> f32 {
        let w = 2.0 * std::f64::consts::PI * freq as f64 / fs as f64;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let (b0, b1, b2) = (self.b0 as f64, self.b1 as f64, self.b2 as f64);
        let (a1, a2) = (self.a1 as f64, self.a2 as f64);
        let nr = b0 + b1 * c1 + b2 * c2;
        let ni = -(b1 * s1 + b2 * s2);
        let dr = 1.0 + a1 * c1 + a2 * c2;
        let di = -(a1 * s1 + a2 * s2);
        let mag = ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt();
        (20.0 * mag.max(1e-12).log10()) as f32
    }
}

/// The three bands as coefficients: low shelf, mid peak, high shelf.
pub fn eq_coeffs(p: &EqParams, fs: f32) -> [Coeffs; 3] {
    [
        Coeffs::low_shelf(fs, p.low_freq, p.low_gain_db),
        Coeffs::peaking(fs, p.mid_freq, p.mid_gain_db, p.mid_q),
        Coeffs::high_shelf(fs, p.high_freq, p.high_gain_db),
    ]
}

/// Combined response of the whole EQ at `freq`, in dB. Used by the UI to draw the curve.
pub fn eq_response_db(p: &EqParams, fs: f32, freq: f32) -> f32 {
    eq_coeffs(p, fs)
        .iter()
        .map(|c| c.magnitude_db(fs, freq))
        .sum()
}

/// One band of the bus equaliser as filter coefficients (flat if the band is off).
pub fn band_coeffs(b: &BandParams, fs: f32) -> Coeffs {
    if !b.on {
        return Coeffs::IDENTITY;
    }
    match b.kind {
        BandKind::Peak => Coeffs::peaking(fs, b.freq, b.gain_db, b.q),
        BandKind::LowShelf => Coeffs::low_shelf(fs, b.freq, b.gain_db),
        BandKind::HighShelf => Coeffs::high_shelf(fs, b.freq, b.gain_db),
        BandKind::HighPass => Coeffs::high_pass(fs, b.freq, b.q),
        BandKind::LowPass => Coeffs::low_pass(fs, b.freq, b.q),
    }
}

/// Combined response of the bus equaliser at `freq`, in dB. Used by the UI to draw the curve.
pub fn param_eq_response_db(p: &ParamEqParams, fs: f32, freq: f32) -> f32 {
    p.bands
        .iter()
        .map(|b| band_coeffs(b, fs).magnitude_db(fs, freq))
        .sum()
}

/// A six-band parametric equaliser for stereo audio.
pub struct ParamEq {
    fs: f32,
    applied: Option<ParamEqParams>,
    coeffs: [Coeffs; BUS_EQ_BANDS],
    state: [[BiquadState; BUS_EQ_BANDS]; 2],
}

impl ParamEq {
    pub fn new(fs: f32) -> Self {
        Self {
            fs,
            applied: None,
            coeffs: [Coeffs::IDENTITY; BUS_EQ_BANDS],
            state: [[BiquadState::default(); BUS_EQ_BANDS]; 2],
        }
    }

    /// `buf` is interleaved stereo.
    pub fn process(&mut self, p: &ParamEqParams, buf: &mut [f32]) {
        if !p.enabled {
            return;
        }
        if self.applied != Some(*p) {
            for (c, b) in self.coeffs.iter_mut().zip(&p.bands) {
                *c = band_coeffs(b, self.fs);
            }
            self.applied = Some(*p);
        }
        for frame in buf.chunks_exact_mut(2) {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let mut x = *sample;
                for band in 0..BUS_EQ_BANDS {
                    x = self.state[ch][band].tick(&self.coeffs[band], x);
                }
                *sample = x;
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct BiquadState {
    z1: f32,
    z2: f32,
}

impl BiquadState {
    #[inline]
    fn tick(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        // Keep denormals out of the feedback path.
        if self.z1.abs() < 1e-20 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-20 {
            self.z2 = 0.0;
        }
        y
    }
}

pub struct Eq {
    fs: f32,
    applied: Option<EqParams>,
    coeffs: [Coeffs; 3],
    state: [[BiquadState; 3]; 2],
}

impl Eq {
    pub fn new(fs: f32) -> Self {
        Self {
            fs,
            applied: None,
            coeffs: [Coeffs::IDENTITY; 3],
            state: [[BiquadState::default(); 3]; 2],
        }
    }

    /// `buf` is interleaved stereo.
    pub fn process(&mut self, p: &EqParams, buf: &mut [f32]) {
        if !p.enabled {
            return;
        }
        if self.applied != Some(*p) {
            self.coeffs = eq_coeffs(p, self.fs);
            self.applied = Some(*p);
        }
        for frame in buf.chunks_exact_mut(2) {
            for (ch, sample) in frame.iter_mut().enumerate() {
                let mut x = *sample;
                for band in 0..3 {
                    x = self.state[ch][band].tick(&self.coeffs[band], x);
                }
                *sample = x;
            }
        }
    }
}

// --- gate ---------------------------------------------------------------------------------------

/// How far a closed gate pulls the signal down.
const GATE_FLOOR_DB: f32 = -80.0;
/// The gate closes this far below its threshold, so a level hovering at the threshold doesn't
/// chatter.
const GATE_HYSTERESIS_DB: f32 = 5.0;

pub struct Gate {
    fs: f32,
    env: f32,
    gain: f32,
    open: bool,
}

impl Gate {
    pub fn new(fs: f32) -> Self {
        Self {
            fs,
            env: 0.0,
            gain: 1.0,
            open: true,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn process(&mut self, p: &GateParams, buf: &mut [f32]) {
        if !p.enabled {
            self.gain = 1.0;
            self.open = true;
            return;
        }
        let env_release = time_coef(40.0, self.fs);
        let open_coef = time_coef(p.attack_ms, self.fs);
        let close_coef = time_coef(p.release_ms, self.fs);
        let floor = db_to_gain(GATE_FLOOR_DB);
        for frame in buf.chunks_exact_mut(2) {
            let peak = frame[0].abs().max(frame[1].abs());
            self.env = if peak > self.env {
                peak
            } else {
                self.env * env_release
            };
            let level_db = gain_to_db(self.env);
            if self.open {
                if level_db < p.threshold_db - GATE_HYSTERESIS_DB {
                    self.open = false;
                }
            } else if level_db > p.threshold_db {
                self.open = true;
            }
            let (target, coef) = if self.open {
                (1.0, open_coef)
            } else {
                (floor, close_coef)
            };
            self.gain = target + (self.gain - target) * coef;
            frame[0] *= self.gain;
            frame[1] *= self.gain;
        }
    }
}

// --- compressor ---------------------------------------------------------------------------------

/// Width of the soft knee around the threshold, in dB.
const KNEE_DB: f32 = 6.0;

pub struct Compressor {
    fs: f32,
    env: f32,
    /// Current gain reduction in dB (<= 0).
    gr_db: f32,
}

impl Compressor {
    pub fn new(fs: f32) -> Self {
        Self {
            fs,
            env: 0.0,
            gr_db: 0.0,
        }
    }

    /// Gain reduction in dB at the end of the last block (<= 0), for the meter.
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }

    pub fn process(&mut self, p: &CompParams, buf: &mut [f32]) {
        if !p.enabled {
            self.gr_db = 0.0;
            return;
        }
        let attack = time_coef(p.attack_ms, self.fs);
        let release = time_coef(p.release_ms, self.fs);
        // The detector needs to bridge the zero crossings of a waveform, or the gain would ripple
        // at audio rate and distort. 40 ms does that without making release sluggish.
        let env_release = time_coef(40.0, self.fs);
        let ratio = p.ratio.max(1.0);
        let makeup = p.makeup_db;
        for frame in buf.chunks_exact_mut(2) {
            let peak = frame[0].abs().max(frame[1].abs());
            self.env = if peak > self.env {
                peak
            } else {
                self.env * env_release
            };
            let x = gain_to_db(self.env);
            let over = x - p.threshold_db;
            let y = if 2.0 * over < -KNEE_DB {
                x
            } else if 2.0 * over.abs() <= KNEE_DB {
                x + (1.0 / ratio - 1.0) * (over + KNEE_DB / 2.0).powi(2) / (2.0 * KNEE_DB)
            } else {
                p.threshold_db + over / ratio
            };
            let target = y - x; // <= 0
            let coef = if target < self.gr_db { attack } else { release };
            self.gr_db = target + (self.gr_db - target) * coef;
            let g = db_to_gain(self.gr_db + makeup);
            frame[0] *= g;
            frame[1] *= g;
        }
    }
}

// --- limiter ------------------------------------------------------------------------------------

/// Instant-attack peak limiter: it never lets a sample above the ceiling through, then recovers
/// smoothly. Used on the output buses so summing several strips can't clip.
pub struct Limiter {
    fs: f32,
    gain: f32,
}

impl Limiter {
    pub fn new(fs: f32) -> Self {
        Self { fs, gain: 1.0 }
    }

    pub fn process(&mut self, ceiling_db: f32, buf: &mut [f32]) {
        let ceiling = db_to_gain(ceiling_db.min(0.0));
        let release = time_coef(80.0, self.fs);
        for frame in buf.chunks_exact_mut(2) {
            let peak = frame[0].abs().max(frame[1].abs());
            let needed = if peak > ceiling { ceiling / peak } else { 1.0 };
            self.gain = if needed < self.gain {
                needed
            } else {
                needed + (self.gain - needed) * release
            };
            frame[0] = (frame[0] * self.gain).clamp(-1.0, 1.0);
            frame[1] = (frame[1] * self.gain).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f32 = 48_000.0;

    /// Interleaved stereo sine, `amp` peak, `secs` long.
    fn sine(freq: f32, amp: f32, secs: f32) -> Vec<f32> {
        let n = (FS * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = amp * (2.0 * PI * freq * i as f32 / FS).sin();
                [v, v]
            })
            .collect()
    }

    /// Peak of the last `tail` seconds, to measure after the processor has settled.
    fn tail_peak(buf: &[f32], tail: f32) -> f32 {
        let n = (FS * tail) as usize * 2;
        buf[buf.len() - n..]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn peaking_eq_hits_its_gain_at_the_centre_and_leaves_far_frequencies_alone() {
        let c = Coeffs::peaking(FS, 1000.0, 6.0, 1.0);
        assert!((c.magnitude_db(FS, 1000.0) - 6.0).abs() < 0.05);
        assert!(c.magnitude_db(FS, 30.0).abs() < 0.2);
        assert!(c.magnitude_db(FS, 15_000.0).abs() < 0.5);
        let cut = Coeffs::peaking(FS, 1000.0, -9.0, 2.0);
        assert!((cut.magnitude_db(FS, 1000.0) + 9.0).abs() < 0.05);
    }

    #[test]
    fn shelves_apply_gain_on_their_own_side_only() {
        let lo = Coeffs::low_shelf(FS, 200.0, 8.0);
        assert!((lo.magnitude_db(FS, 20.0) - 8.0).abs() < 0.3);
        assert!(lo.magnitude_db(FS, 10_000.0).abs() < 0.3);
        let hi = Coeffs::high_shelf(FS, 6000.0, -6.0);
        assert!((hi.magnitude_db(FS, 18_000.0) + 6.0).abs() < 0.4);
        assert!(hi.magnitude_db(FS, 100.0).abs() < 0.3);
    }

    #[test]
    fn eq_processing_matches_the_computed_response() {
        let p = EqParams {
            enabled: true,
            mid_freq: 1000.0,
            mid_gain_db: 6.0,
            ..EqParams::default()
        };
        let mut eq = Eq::new(FS);
        let mut buf = sine(1000.0, 0.25, 0.5);
        eq.process(&p, &mut buf);
        let measured = gain_to_db(tail_peak(&buf, 0.1) / 0.25);
        assert!((measured - 6.0).abs() < 0.3, "measured {measured}");
    }

    #[test]
    fn disabled_eq_is_transparent() {
        let mut eq = Eq::new(FS);
        let original = sine(440.0, 0.5, 0.05);
        let mut buf = original.clone();
        eq.process(&EqParams::default(), &mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn compressor_follows_its_ratio_above_the_threshold() {
        // -10 dBFS in, threshold -20, 4:1 => -20 + 10/4 = -17.5 dBFS out.
        let p = CompParams {
            enabled: true,
            threshold_db: -20.0,
            ratio: 4.0,
            attack_ms: 1.0,
            release_ms: 50.0,
            makeup_db: 0.0,
        };
        let mut comp = Compressor::new(FS);
        let amp = db_to_gain(-10.0);
        let mut buf = sine(1000.0, amp, 1.0);
        comp.process(&p, &mut buf);
        let out_db = gain_to_db(tail_peak(&buf, 0.1));
        assert!((out_db + 17.5).abs() < 0.8, "out {out_db} dB");
        assert!(comp.gain_reduction_db() < -6.0);
    }

    #[test]
    fn compressor_leaves_quiet_signal_alone_and_applies_makeup() {
        let mut p = CompParams {
            enabled: true,
            threshold_db: -20.0,
            ratio: 4.0,
            ..CompParams::default()
        };
        let amp = db_to_gain(-40.0);
        let mut comp = Compressor::new(FS);
        let mut buf = sine(1000.0, amp, 0.5);
        comp.process(&p, &mut buf);
        assert!((gain_to_db(tail_peak(&buf, 0.1)) + 40.0).abs() < 0.5);

        p.makeup_db = 6.0;
        let mut comp = Compressor::new(FS);
        let mut buf = sine(1000.0, amp, 0.5);
        comp.process(&p, &mut buf);
        assert!((gain_to_db(tail_peak(&buf, 0.1)) + 34.0).abs() < 0.5);
    }

    #[test]
    fn compressor_does_not_distort_a_steady_tone() {
        // The detector must not ripple with the waveform: output stays a clean sine, so its
        // sample-to-sample peaks are constant once settled.
        let p = CompParams {
            enabled: true,
            threshold_db: -30.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            makeup_db: 0.0,
        };
        let mut comp = Compressor::new(FS);
        let mut buf = sine(200.0, 0.5, 1.0);
        comp.process(&p, &mut buf);
        let a = tail_peak(&buf[buf.len() - 9_600..], 0.05);
        let b = tail_peak(&buf[buf.len() - 19_200..buf.len() - 9_600], 0.05);
        assert!((gain_to_db(a) - gain_to_db(b)).abs() < 0.3);
    }

    #[test]
    fn gate_silences_quiet_signal_and_passes_loud_signal() {
        let p = GateParams {
            enabled: true,
            threshold_db: -40.0,
            attack_ms: 1.0,
            release_ms: 20.0,
        };
        let mut gate = Gate::new(FS);
        let mut quiet = sine(500.0, db_to_gain(-60.0), 0.5);
        gate.process(&p, &mut quiet);
        assert!(gain_to_db(tail_peak(&quiet, 0.1)) < -100.0);
        assert!(!gate.is_open());

        let mut loud = sine(500.0, db_to_gain(-20.0), 0.5);
        gate.process(&p, &mut loud);
        assert!((gain_to_db(tail_peak(&loud, 0.1)) + 20.0).abs() < 0.5);
        assert!(gate.is_open());
    }

    #[test]
    fn limiter_never_exceeds_its_ceiling() {
        let mut lim = Limiter::new(FS);
        let mut buf = sine(100.0, 3.0, 0.5); // +9.5 dBFS: wildly over
        lim.process(-3.0, &mut buf);
        let ceiling = db_to_gain(-3.0);
        assert!(buf.iter().all(|v| v.abs() <= ceiling + 1e-4));
        // And it really does pass signal, not just silence.
        assert!(tail_peak(&buf, 0.1) > ceiling * 0.9);
    }

    #[test]
    fn limiter_passes_quiet_audio_untouched() {
        let mut lim = Limiter::new(FS);
        let original = sine(440.0, 0.2, 0.1);
        let mut buf = original.clone();
        lim.process(-0.5, &mut buf);
        assert_eq!(buf, original);
    }
}
