//! Clock-drift handling between audio devices.
//!
//! Every sound card runs on its own crystal, so two devices that both claim 48 000 Hz really
//! differ by tens of parts per million. Left alone, a buffer between them slowly fills or drains
//! until it overflows or runs dry and clicks. [`RateReader`] sits on the reading end of a ring
//! buffer, resamples with a cubic interpolator, and nudges its ratio with a slow PI controller so
//! the buffer stays at a steady fill. The same type converts between sample rates (44.1 -> 48 kHz).

use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

/// A ring buffer for interleaved stereo audio, sized in frames.
pub fn ring(frames: usize) -> (HeapProd<f32>, HeapCons<f32>) {
    HeapRb::<f32>::new(frames.max(16) * 2).split()
}

/// Largest correction the controller may apply: 1% is far beyond any real crystal error, but
/// leaves headroom to recover from a hiccup.
const MAX_ADJUST: f64 = 0.01;
/// Controller gains on the smoothed fill error (a fraction of the target). `KP` is the correction
/// per unit of error; `KI` accumulates per 10 ms block. Tuned for critical damping with the loop
/// settling in ~40 s: crystal drift is constant, so slow is fine, and a fast loop would wobble
/// the pitch.
///
/// Loop dynamics, with N = rate / target: s^2 + N*KP s + N*KI*100 = 0. At 48 kHz and a 960-frame
/// target N = 50, giving w = 0.1 rad/s and damping 1.
const KP: f64 = 4.0e-3;
const KI: f64 = 2.0e-6;
/// Smoothing of the fill reading, per block (~0.5 s). Device callbacks deliver audio in bursts, so
/// the raw fill jumps by a whole buffer; reacting to that would modulate the pitch.
const FILL_SMOOTH: f64 = 0.02;
/// A device that delivers audio in big or uneven chunks runs the buffer dry at the target fill.
/// Each such dropout makes the link buffer half as much again, up to this many seconds, so a
/// stuttering link settles into a steady one instead of glitching for good.
const MAX_GROWN_LATENCY_S: f64 = 0.15;

pub struct RateReader {
    cons: HeapCons<f32>,
    /// Input frames consumed per output frame when the clocks agree exactly (in/out rate).
    nominal: f64,
    /// Fill, in frames, the controller steers toward. This is the latency this link adds.
    target: f64,
    /// Four most recent input frames, oldest first. The output lies between `hist[1]` and
    /// `hist[2]`.
    hist: [[f32; 2]; 4],
    frac: f64,
    integral: f64,
    adjust: f64,
    /// Smoothed fill in frames.
    fill_avg: f64,
    primed: bool,
    /// Turn the drift controller off (tests measuring the interpolator on its own).
    #[cfg(test)]
    adaptive: bool,
    /// A source that sends nothing while it is silent (WASAPI loopback). An empty ring is then
    /// normal, not a dropout.
    tolerant: bool,
    /// The most latency a link may grow to while recovering from dropouts.
    max_target: f64,
    pub underruns: u64,
    scratch: [f32; 2],
}

impl RateReader {
    /// `in_rate`: the rate the writer fills at; `out_rate`: the rate this reader is called at.
    pub fn new(cons: HeapCons<f32>, in_rate: f64, out_rate: f64, target_frames: usize) -> Self {
        Self {
            cons,
            nominal: in_rate / out_rate,
            target: target_frames.max(1) as f64,
            hist: [[0.0; 2]; 4],
            frac: 0.0,
            integral: 0.0,
            adjust: 0.0,
            fill_avg: target_frames.max(1) as f64,
            primed: false,
            #[cfg(test)]
            adaptive: true,
            tolerant: false,
            max_target: (in_rate * MAX_GROWN_LATENCY_S).max(target_frames as f64),
            underruns: 0,
            scratch: [0.0; 2],
        }
    }

    /// For sources that go quiet by sending nothing: starvation plays silence without counting a
    /// dropout.
    pub fn tolerate_silence(mut self) -> Self {
        self.tolerant = true;
        self
    }

    /// Frames currently waiting in the ring.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn fill_frames(&self) -> usize {
        self.cons.occupied_len() / 2
    }

    /// Current correction factor (0 = clocks agree), for diagnostics.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn adjust(&self) -> f64 {
        self.adjust
    }

    fn pop_frame(&mut self) -> [f32; 2] {
        if self.cons.pop_slice(&mut self.scratch) == 2 {
            [self.scratch[0], self.scratch[1]]
        } else {
            // Ran dry: hold the last sample rather than clicking to zero, and re-buffer.
            if !self.tolerant && self.primed {
                self.underruns += 1;
                self.target = (self.target * 1.5).min(self.max_target);
            }
            self.primed = false;
            self.hist[3]
        }
    }

    /// Fill `out` (interleaved stereo) with resampled audio. Plays silence until the buffer has
    /// reached its target fill, so start-up and recovery from a dropout are clean.
    pub fn read(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        if !self.primed {
            if (self.cons.occupied_len() / 2) as f64 >= self.target {
                self.primed = true;
                self.fill_avg = (self.cons.occupied_len() / 2) as f64;
                // Start with real audio in the interpolator's history.
                for _ in 0..4 {
                    let f = self.pop_frame();
                    self.hist.rotate_left(1);
                    self.hist[3] = f;
                }
                self.frac = 0.0;
            } else {
                out.fill(0.0);
                return;
            }
        }

        let ratio = self.nominal * (1.0 + self.adjust);
        for frame in out.chunks_exact_mut(2) {
            if !self.primed {
                // Dropped out mid-block: finish the block with silence.
                frame.fill(0.0);
                continue;
            }
            let t = self.frac as f32;
            for (ch, sample) in frame.iter_mut().enumerate() {
                *sample = hermite(
                    self.hist[0][ch],
                    self.hist[1][ch],
                    self.hist[2][ch],
                    self.hist[3][ch],
                    t,
                );
            }
            self.frac += ratio;
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                let f = self.pop_frame();
                self.hist.rotate_left(1);
                self.hist[3] = f;
            }
        }

        // One controller step per block, driven by how far the fill is from where we want it.
        #[cfg(test)]
        let adaptive = self.adaptive;
        #[cfg(not(test))]
        let adaptive = true;
        if adaptive && self.primed && frames > 0 {
            let fill = self.cons.occupied_len() as f64 / 2.0;
            self.fill_avg += (fill - self.fill_avg) * FILL_SMOOTH;
            let err = (self.fill_avg - self.target) / self.target;
            self.integral = (self.integral + err * KI).clamp(-MAX_ADJUST, MAX_ADJUST);
            self.adjust = (err * KP + self.integral).clamp(-MAX_ADJUST, MAX_ADJUST);
        }
    }
}

/// Catmull-Rom cubic through `p1` (t = 0) and `p2` (t = 1), using `p0` and `p3` as neighbours.
#[inline]
fn hermite(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    p1 + 0.5
        * t
        * (p2 - p0 + t * (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3 + t * (3.0 * (p1 - p2) + p3 - p0)))
}

/// Push as many whole frames of `data` as fit. Returns frames written; the rest are dropped,
/// which only happens if the reader has stalled.
pub fn write_frames(prod: &mut HeapProd<f32>, data: &[f32]) -> usize {
    let room = prod.vacant_len() / 2;
    let n = (data.len() / 2).min(room);
    prod.push_slice(&data[..n * 2]);
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Drive a writer at `in_rate` and a reader at `out_rate` for `secs` of simulated time, in
    /// 10 ms reader blocks, feeding a sine of `freq`. Returns (reader, output samples, min fill,
    /// max fill) with the first `settle` seconds left out of the fill stats.
    fn simulate(
        in_rate: f64,
        out_rate: f64,
        freq: f32,
        secs: f64,
        target: usize,
    ) -> (RateReader, Vec<f32>, usize, usize) {
        simulate_drift(in_rate, in_rate, out_rate, freq, secs, target, true)
    }

    /// Like `simulate`, but the writer really runs at `actual_in` while the reader has only been
    /// told `declared_in`: that gap is the drift the controller has to find.
    fn simulate_drift(
        declared_in: f64,
        actual_in: f64,
        out_rate: f64,
        freq: f32,
        secs: f64,
        target: usize,
        adaptive: bool,
    ) -> (RateReader, Vec<f32>, usize, usize) {
        let in_rate = actual_in;
        let (mut prod, cons) = ring(8192);
        let mut reader = RateReader::new(cons, declared_in, out_rate, target);
        reader.adaptive = adaptive;
        let block = (out_rate / 100.0) as usize;
        let mut written = 0u64; // input frames produced so far
        let mut owed = 0.0f64;
        let mut out = Vec::new();
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        let blocks = (secs * 100.0) as usize;
        let mut buf = vec![0.0f32; block * 2];
        for b in 0..blocks {
            // The writer produces in_rate/100 frames per 10 ms of real time.
            owed += in_rate / 100.0;
            let n = owed as usize;
            owed -= n as f64;
            let data: Vec<f32> = (0..n)
                .flat_map(|i| {
                    let t = (written + i as u64) as f32 / in_rate as f32;
                    let v = 0.5 * (2.0 * PI * freq * t).sin();
                    [v, v]
                })
                .collect();
            written += n as u64;
            write_frames(&mut prod, &data);
            reader.read(&mut buf);
            out.extend_from_slice(&buf);
            if b as f64 > secs * 100.0 * 0.2 {
                lo = lo.min(reader.fill_frames());
                hi = hi.max(reader.fill_frames());
            }
        }
        (reader, out, lo, hi)
    }

    #[test]
    fn a_bursty_device_makes_the_buffer_grow_until_it_stops_dropping_out() {
        // The writer delivers 40 ms in one go every 40 ms, far lumpier than a 10 ms target allows.
        let (mut prod, cons) = ring(32_768);
        let mut reader = RateReader::new(cons, 48_000.0, 48_000.0, 480);
        let mut buf = vec![0.0f32; 960];
        let burst = vec![0.1f32; 1920 * 2];
        let mut late = 0;
        for b in 0..3000 {
            if b % 4 == 0 {
                write_frames(&mut prod, &burst);
            }
            let before = reader.underruns;
            reader.read(&mut buf);
            if b >= 2000 && reader.underruns > before {
                late += 1;
            }
        }
        assert!(reader.underruns > 0, "the test should start out glitching");
        assert_eq!(late, 0, "still dropping out after settling");
        assert!(reader.target > 1920.0);
    }

    #[test]
    fn matched_clocks_stay_steady_and_never_underrun() {
        let (reader, _, lo, hi) = simulate(48_000.0, 48_000.0, 440.0, 20.0, 960);
        assert_eq!(reader.underruns, 0);
        assert!(lo > 400 && hi < 1600, "fill range {lo}..{hi}");
    }

    #[test]
    fn a_fast_writer_clock_is_tracked_without_overflow() {
        // Writer 500 ppm fast: an uncorrected buffer would grow ~24 frames every second.
        let (reader, _, lo, hi) =
            simulate_drift(48_000.0, 48_024.0, 48_000.0, 440.0, 90.0, 960, true);
        assert_eq!(reader.underruns, 0);
        assert!(hi < 2400, "fill grew to {hi}");
        assert!(lo > 200, "fill fell to {lo}");
        assert!(
            (reader.adjust() - 0.0005).abs() < 0.0003,
            "adjust {}",
            reader.adjust()
        );
    }

    #[test]
    fn a_slow_writer_clock_is_tracked_without_underrun() {
        let (reader, _, lo, hi) =
            simulate_drift(48_000.0, 47_976.0, 48_000.0, 440.0, 90.0, 960, true);
        assert_eq!(reader.underruns, 0);
        assert!(lo > 200 && hi < 2400, "fill range {lo}..{hi}");
        assert!(
            (reader.adjust() + 0.0005).abs() < 0.0001,
            "adjust {}",
            reader.adjust()
        );
    }

    #[test]
    fn sample_rate_conversion_preserves_pitch_and_level() {
        let (reader, out, _, _) = simulate(44_100.0, 48_000.0, 1000.0, 5.0, 960);
        assert_eq!(reader.underruns, 0);
        // Look at the last second, well after settling. Left channel only.
        let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let tail = &left[left.len() - 48_000..];
        let crossings = tail
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!(
            (crossings as i32 - 1000).abs() <= 2,
            "{crossings} cycles in 1 s"
        );
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.02, "peak {peak}");
    }

    #[test]
    fn resampled_output_is_clean() {
        // 44.1 -> 48 kHz of a 1 kHz tone: compare against the ideal tone. The residual (aliasing
        // and interpolation error) should sit far below the signal.
        let (_, out, _, _) = simulate_drift(44_100.0, 44_100.0, 48_000.0, 1000.0, 3.0, 960, false);
        let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let tail = &left[left.len() - 24_000..];
        // Fit amplitude and phase by correlating with sin/cos at 1 kHz, then measure what's left.
        let (mut s, mut c) = (0.0f64, 0.0f64);
        for (i, v) in tail.iter().enumerate() {
            let w = 2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0;
            s += *v as f64 * w.sin();
            c += *v as f64 * w.cos();
        }
        let n = tail.len() as f64;
        let (a, b) = (2.0 * s / n, 2.0 * c / n);
        let mut err = 0.0f64;
        let mut sig = 0.0f64;
        for (i, v) in tail.iter().enumerate() {
            let w = 2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0;
            let fit = a * w.sin() + b * w.cos();
            err += (*v as f64 - fit).powi(2);
            sig += fit.powi(2);
        }
        let snr_db = 10.0 * (sig / err.max(1e-30)).log10();
        assert!(snr_db > 55.0, "SNR {snr_db:.1} dB");
    }

    #[test]
    fn a_silent_loopback_source_is_not_a_dropout() {
        let (mut prod, cons) = ring(4096);
        let mut reader = RateReader::new(cons, 48_000.0, 48_000.0, 480).tolerate_silence();
        let mut buf = vec![0.0f32; 960];
        write_frames(&mut prod, &vec![0.25f32; 1920]);
        reader.read(&mut buf);
        // The source goes quiet: nothing more arrives.
        for _ in 0..20 {
            reader.read(&mut buf);
        }
        assert_eq!(reader.underruns, 0, "silence is not an underrun");
        assert!(buf.iter().all(|v| *v == 0.0), "and it plays as silence");
        // Audio returns and plays again.
        write_frames(&mut prod, &vec![0.25f32; 1920]);
        reader.read(&mut buf);
        assert!(buf.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn a_normal_reader_that_starves_counts_it() {
        let (mut prod, cons) = ring(4096);
        let mut reader = RateReader::new(cons, 48_000.0, 48_000.0, 480);
        let mut buf = vec![0.0f32; 960];
        write_frames(&mut prod, &vec![0.25f32; 1920]);
        for _ in 0..10 {
            reader.read(&mut buf);
        }
        assert!(reader.underruns > 0);
    }

    #[test]
    fn a_starved_reader_plays_silence_then_recovers() {
        let (mut prod, cons) = ring(4096);
        let mut reader = RateReader::new(cons, 48_000.0, 48_000.0, 480);
        let mut buf = vec![1.0f32; 960];
        reader.read(&mut buf);
        assert!(buf.iter().all(|v| *v == 0.0), "silent until primed");
        write_frames(&mut prod, &vec![0.25f32; 1920]);
        reader.read(&mut buf);
        assert!(buf.iter().any(|v| *v != 0.0), "plays once primed");
    }
}
