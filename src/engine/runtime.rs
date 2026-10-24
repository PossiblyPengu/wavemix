//! Threads and plumbing. Two threads cooperate:
//!
//! * the **mix thread** wakes every 10 ms, pulls a block from each input, runs the [`Mixer`], and
//!   pushes a block to each output bus. It never blocks on anything but its own timer.
//! * the **I/O thread** opens and closes device streams as the user picks devices, notices when
//!   devices appear or vanish, and hands finished ring-buffer endpoints to the mix thread.
//!
//! Devices talk to the mix thread only through lock-free ring buffers with drift correction, so a
//! slow device can glitch itself but never stalls the mix.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ringbuf::HeapProd;

use super::io::{AudioIo, DeviceInfo, Opened};
use super::mixer::{Levels, Mixer};
use super::params::{EngineParams, IoConfig, BUSES, STRIPS};
use super::rate::{ring, write_frames, RateReader};

/// Internal sample rate. Every device is converted to and from this.
pub const ENGINE_RATE: u32 = 48_000;
/// Frames per mix block (10 ms).
pub const BLOCK: usize = 480;
const BLOCK_DURATION: Duration = Duration::from_millis(10);
/// Ring capacity per device, in frames: generous, since the drift controller keeps the real fill
/// near the latency target.
const RING_FRAMES: usize = 32_768;
/// After a failed open, wait this long before trying the same device again.
const RETRY_AFTER: Duration = Duration::from_secs(3);
const DEVICE_REFRESH: Duration = Duration::from_secs(2);

pub const DEFAULT_LATENCY_MS: u32 = 30;

#[derive(Clone, Debug, PartialEq)]
pub enum SlotStatus {
    /// Nothing assigned, or the engine is off.
    Off,
    Running {
        rate: u32,
    },
    /// Assigned, but the device isn't present (unplugged, or the cable driver isn't installed).
    Missing,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct Status {
    pub inputs: [SlotStatus; STRIPS],
    pub outputs: [SlotStatus; BUSES],
}

impl Default for Status {
    fn default() -> Self {
        Self {
            inputs: std::array::from_fn(|_| SlotStatus::Off),
            outputs: std::array::from_fn(|_| SlotStatus::Off),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Devices {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

/// State shared between the UI, the mix thread and the I/O thread.
pub struct Shared {
    params: Mutex<EngineParams>,
    /// Bumped on every parameter change so the mix thread only copies when something changed.
    version: AtomicU64,
    pub levels: Levels,
    running: AtomicBool,
    /// Mix load, in tenths of a percent of the 10 ms block budget.
    cpu_permille: AtomicU32,
    input_underruns: [AtomicU64; STRIPS],
    output_underruns: [Arc<AtomicU64>; BUSES],
    output_overruns: [AtomicU64; BUSES],
    /// Times the mix thread fell more than a few blocks behind its clock.
    late_blocks: AtomicU64,
    devices: Mutex<Devices>,
    status: Mutex<Status>,
}

/// Counters for the status display.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub cpu_percent: f32,
    pub late_blocks: u64,
    pub input_underruns: [u64; STRIPS],
    pub output_underruns: [u64; BUSES],
    pub output_overruns: [u64; BUSES],
}

enum IoCmd {
    SetInput(usize, Option<String>),
    SetOutput(usize, Option<String>),
    SetRunning(bool),
    SetLatency(u32),
    Refresh,
    Shutdown,
}

enum MixMsg {
    Feed(usize, Option<RateReader>),
    Sink(usize, Option<HeapProd<f32>>),
}

pub struct Engine {
    shared: Arc<Shared>,
    io_tx: Sender<IoCmd>,
    shutdown: Arc<AtomicBool>,
}

impl Engine {
    /// Start the engine. `make_io` runs on the I/O thread, because device handles often can't
    /// move between threads.
    pub fn start(
        make_io: impl FnOnce() -> Box<dyn AudioIo> + Send + 'static,
        config: IoConfig,
        params: EngineParams,
    ) -> Engine {
        let shared = Arc::new(Shared {
            params: Mutex::new(params),
            version: AtomicU64::new(1),
            levels: Levels::default(),
            running: AtomicBool::new(config.engine_on),
            cpu_permille: AtomicU32::new(0),
            input_underruns: Default::default(),
            output_underruns: Default::default(),
            output_overruns: Default::default(),
            late_blocks: AtomicU64::new(0),
            devices: Mutex::default(),
            status: Mutex::default(),
        });
        let shutdown = Arc::new(AtomicBool::new(false));
        let (mix_tx, mix_rx) = mpsc::channel();
        let (io_tx, io_rx) = mpsc::channel();

        let s = shared.clone();
        let sd = shutdown.clone();
        thread::Builder::new()
            .name("mix".into())
            .spawn(move || mix_thread(s, mix_rx, sd))
            .expect("spawn mix thread");

        let s = shared.clone();
        thread::Builder::new()
            .name("audio-io".into())
            .spawn(move || io_thread(Box::new(make_io), s, io_rx, mix_tx, config))
            .expect("spawn io thread");

        Engine {
            shared,
            io_tx,
            shutdown,
        }
    }

    pub fn params(&self) -> EngineParams {
        lock(&self.shared.params).clone()
    }

    pub fn set_params(&self, params: EngineParams) {
        *lock(&self.shared.params) = params;
        self.shared.version.fetch_add(1, Ordering::Release);
    }

    /// Edit the parameters in place.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn update_params(&self, f: impl FnOnce(&mut EngineParams)) {
        f(&mut lock(&self.shared.params));
        self.shared.version.fetch_add(1, Ordering::Release);
    }

    pub fn levels(&self) -> &Levels {
        &self.shared.levels
    }

    pub fn devices(&self) -> Devices {
        lock(&self.shared.devices).clone()
    }

    pub fn status(&self) -> Status {
        lock(&self.shared.status).clone()
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> Stats {
        let s = &self.shared;
        Stats {
            cpu_percent: s.cpu_permille.load(Ordering::Relaxed) as f32 / 10.0,
            late_blocks: s.late_blocks.load(Ordering::Relaxed),
            input_underruns: std::array::from_fn(|i| s.input_underruns[i].load(Ordering::Relaxed)),
            output_underruns: std::array::from_fn(|i| {
                s.output_underruns[i].load(Ordering::Relaxed)
            }),
            output_overruns: std::array::from_fn(|i| s.output_overruns[i].load(Ordering::Relaxed)),
        }
    }

    pub fn set_input(&self, slot: usize, device: Option<String>) {
        let _ = self.io_tx.send(IoCmd::SetInput(slot, device));
    }

    pub fn set_output(&self, bus: usize, device: Option<String>) {
        let _ = self.io_tx.send(IoCmd::SetOutput(bus, device));
    }

    pub fn set_running(&self, on: bool) {
        self.shared.running.store(on, Ordering::Relaxed);
        let _ = self.io_tx.send(IoCmd::SetRunning(on));
    }

    /// Latency target per device link in milliseconds. Applies the next time a stream opens.
    pub fn set_latency_ms(&self, ms: u32) {
        let _ = self.io_tx.send(IoCmd::SetLatency(ms));
    }

    pub fn refresh_devices(&self) {
        let _ = self.io_tx.send(IoCmd::Refresh);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.io_tx.send(IoCmd::Shutdown);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// --- mix thread -------------------------------------------------------------------------------

fn mix_thread(shared: Arc<Shared>, rx: Receiver<MixMsg>, shutdown: Arc<AtomicBool>) {
    #[cfg(windows)]
    unsafe {
        // Without this Windows rounds sleeps up to ~15 ms and a 10 ms block clock can't hold.
        windows_sys::Win32::Media::timeBeginPeriod(1);
    }
    let mut mixer = Mixer::new(ENGINE_RATE as f32);
    let mut feeds: [Option<RateReader>; STRIPS] = std::array::from_fn(|_| None);
    let mut sinks: [Option<HeapProd<f32>>; BUSES] = std::array::from_fn(|_| None);
    let mut params = lock(&shared.params).clone();
    let mut seen = shared.version.load(Ordering::Acquire);
    let mut ins = vec![vec![0.0f32; BLOCK * 2]; STRIPS];
    let mut outs = vec![vec![0.0f32; BLOCK * 2]; BUSES];
    let sleeper = spin_sleep::SpinSleeper::default();
    let mut next = Instant::now();
    let mut cpu = 0.0f32;

    while !shutdown.load(Ordering::Relaxed) {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                MixMsg::Feed(i, f) => feeds[i] = f,
                MixMsg::Sink(b, s) => sinks[b] = s,
            }
        }
        if !shared.running.load(Ordering::Relaxed) {
            sleeper.sleep(BLOCK_DURATION);
            next = Instant::now();
            continue;
        }

        let started = Instant::now();
        let v = shared.version.load(Ordering::Acquire);
        if v != seen {
            // Never wait for the UI: if it's mid-edit, use the old settings for one more block.
            if let Ok(g) = shared.params.try_lock() {
                params = g.clone();
                seen = v;
            }
        }

        for i in 0..STRIPS {
            match &mut feeds[i] {
                Some(reader) => {
                    reader.read(&mut ins[i]);
                    shared.input_underruns[i].store(reader.underruns, Ordering::Relaxed);
                }
                None => ins[i].fill(0.0),
            }
        }
        mixer.process(&params, &mut ins, &mut outs, &shared.levels);
        for b in 0..BUSES {
            if let Some(prod) = &mut sinks[b] {
                if write_frames(prod, &outs[b]) < BLOCK {
                    shared.output_overruns[b].fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        // Smoothed load as a share of the block budget, for the status bar.
        let load = started.elapsed().as_secs_f32() / BLOCK_DURATION.as_secs_f32();
        cpu += (load - cpu) * 0.05;
        shared
            .cpu_permille
            .store((cpu * 1000.0) as u32, Ordering::Relaxed);

        next += BLOCK_DURATION;
        let now = Instant::now();
        if next > now {
            sleeper.sleep(next - now);
        } else if now - next > BLOCK_DURATION * 5 {
            // Fell badly behind (machine stalled): resync rather than racing to catch up.
            shared.late_blocks.fetch_add(1, Ordering::Relaxed);
            next = now;
        }
    }
}

// --- I/O thread -------------------------------------------------------------------------------

struct OpenSlot {
    name: String,
    rate: u32,
    _stream: Opened,
}

struct IoState {
    io: Box<dyn AudioIo>,
    shared: Arc<Shared>,
    mix_tx: Sender<MixMsg>,
    want_in: [Option<String>; STRIPS],
    want_out: [Option<String>; BUSES],
    running: bool,
    latency_ms: u32,
    open_in: [Option<OpenSlot>; STRIPS],
    open_out: [Option<OpenSlot>; BUSES],
    /// When the last failed attempt happened, per slot, to rate-limit retries.
    retry_in: [Option<Instant>; STRIPS],
    retry_out: [Option<Instant>; BUSES],
    status: Status,
}

fn io_thread(
    make: Box<dyn FnOnce() -> Box<dyn AudioIo> + Send>,
    shared: Arc<Shared>,
    rx: Receiver<IoCmd>,
    mix_tx: Sender<MixMsg>,
    config: IoConfig,
) {
    let mut st = IoState {
        io: make(),
        shared,
        mix_tx,
        want_in: config.inputs,
        want_out: config.outputs,
        running: config.engine_on,
        latency_ms: DEFAULT_LATENCY_MS,
        open_in: std::array::from_fn(|_| None),
        open_out: std::array::from_fn(|_| None),
        retry_in: [None; STRIPS],
        retry_out: [None; BUSES],
        status: Status::default(),
    };
    let mut last_refresh = Instant::now() - DEVICE_REFRESH;
    let mut devices = Devices::default();

    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(IoCmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(IoCmd::SetInput(i, d)) => {
                st.want_in[i] = d;
                st.retry_in[i] = None;
            }
            Ok(IoCmd::SetOutput(b, d)) => {
                st.want_out[b] = d;
                st.retry_out[b] = None;
            }
            Ok(IoCmd::SetRunning(on)) => st.running = on,
            Ok(IoCmd::SetLatency(ms)) => {
                let ms = ms.clamp(10, 200);
                if ms != st.latency_ms {
                    st.latency_ms = ms;
                    // Targets are fixed when a stream opens, so reopen everything.
                    st.close_all();
                }
            }
            Ok(IoCmd::Refresh) => last_refresh = Instant::now() - DEVICE_REFRESH,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if last_refresh.elapsed() >= DEVICE_REFRESH {
            devices = Devices {
                inputs: st.io.inputs(),
                outputs: st.io.outputs(),
            };
            *lock(&st.shared.devices) = devices.clone();
            last_refresh = Instant::now();
        }
        st.reconcile(&devices);
        *lock(&st.shared.status) = st.status.clone();
    }
    // Dropping `st` closes every stream.
}

impl IoState {
    /// Make the open streams match what the user wants and what's actually plugged in.
    fn reconcile(&mut self, devices: &Devices) {
        for i in 0..STRIPS {
            let want = if self.running {
                self.want_in[i].clone()
            } else {
                None
            };
            if self.open_in[i].as_ref().map(|o| &o.name) != want.as_ref() {
                self.close_input(i);
            }
            let Some(name) = want else {
                self.status.inputs[i] = SlotStatus::Off;
                continue;
            };
            if let Some(o) = &self.open_in[i] {
                // Still open; if the device vanished, the stream will be dead, so reopen.
                if devices.inputs.iter().any(|d| d.name == name) {
                    self.status.inputs[i] = SlotStatus::Running { rate: o.rate };
                    continue;
                }
                self.close_input(i);
            }
            if !devices.inputs.iter().any(|d| d.name == name) {
                self.status.inputs[i] = SlotStatus::Missing;
                continue;
            }
            if self.retry_in[i].is_some_and(|t| t.elapsed() < RETRY_AFTER) {
                continue;
            }
            self.status.inputs[i] = match self.open_input(i, &name) {
                Ok(rate) => {
                    self.retry_in[i] = None;
                    SlotStatus::Running { rate }
                }
                Err(e) => {
                    self.retry_in[i] = Some(Instant::now());
                    SlotStatus::Error(e)
                }
            };
        }

        for b in 0..BUSES {
            let want = if self.running {
                self.want_out[b].clone()
            } else {
                None
            };
            if self.open_out[b].as_ref().map(|o| &o.name) != want.as_ref() {
                self.close_output(b);
            }
            let Some(name) = want else {
                self.status.outputs[b] = SlotStatus::Off;
                continue;
            };
            if let Some(o) = &self.open_out[b] {
                if devices.outputs.iter().any(|d| d.name == name) {
                    self.status.outputs[b] = SlotStatus::Running { rate: o.rate };
                    continue;
                }
                self.close_output(b);
            }
            if !devices.outputs.iter().any(|d| d.name == name) {
                self.status.outputs[b] = SlotStatus::Missing;
                continue;
            }
            if self.retry_out[b].is_some_and(|t| t.elapsed() < RETRY_AFTER) {
                continue;
            }
            self.status.outputs[b] = match self.open_output(b, &name) {
                Ok(rate) => {
                    self.retry_out[b] = None;
                    SlotStatus::Running { rate }
                }
                Err(e) => {
                    self.retry_out[b] = Some(Instant::now());
                    SlotStatus::Error(e)
                }
            };
        }
    }

    fn open_input(&mut self, slot: usize, name: &str) -> Result<u32, String> {
        let (prod, cons) = ring(RING_FRAMES);
        let opened = self.io.open_input(name, prod)?;
        let rate = opened.rate;
        let target = (rate as u64 * self.latency_ms as u64 / 1000) as usize;
        let mut reader = RateReader::new(cons, rate as f64, ENGINE_RATE as f64, target);
        if opened.loopback || super::io::looks_virtual(name) {
            // Windows (and some virtual cables) send nothing at all while silent; that is not a
            // dropout.
            reader = reader.tolerate_silence();
        }
        let _ = self.mix_tx.send(MixMsg::Feed(slot, Some(reader)));
        self.open_in[slot] = Some(OpenSlot {
            name: name.to_string(),
            rate,
            _stream: opened,
        });
        Ok(rate)
    }

    fn close_all(&mut self) {
        for i in 0..STRIPS {
            self.close_input(i);
        }
        for b in 0..BUSES {
            self.close_output(b);
        }
    }

    fn close_input(&mut self, slot: usize) {
        let _ = self.mix_tx.send(MixMsg::Feed(slot, None));
        self.open_in[slot] = None;
    }

    fn open_output(&mut self, bus: usize, name: &str) -> Result<u32, String> {
        let (prod, cons) = ring(RING_FRAMES);
        let opened = self.io.open_output(
            name,
            cons,
            ENGINE_RATE,
            self.latency_ms,
            self.shared.output_underruns[bus].clone(),
        )?;
        let rate = opened.rate;
        let _ = self.mix_tx.send(MixMsg::Sink(bus, Some(prod)));
        self.open_out[bus] = Some(OpenSlot {
            name: name.to_string(),
            rate,
            _stream: opened,
        });
        Ok(rate)
    }

    fn close_output(&mut self, bus: usize) {
        let _ = self.mix_tx.send(MixMsg::Sink(bus, None));
        self.open_out[bus] = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::sim::{SimIo, Taps};

    const MIC: &str = "Microphone (Sim USB)";
    const LINE: &str = "Line In (Sim Audio)";
    const SPEAKERS: &str = "Speakers (Sim Audio)";
    const HEADPHONES: &str = "Headphones (Sim DAC)";

    fn start(
        inputs: &[(usize, &str)],
        outputs: &[(usize, &str)],
        params: EngineParams,
    ) -> (Engine, Taps) {
        let sim = SimIo::new();
        let taps = sim.taps();
        let mut cfg = IoConfig {
            engine_on: true,
            ..IoConfig::default()
        };
        for (i, n) in inputs {
            cfg.inputs[*i] = Some(n.to_string());
        }
        for (b, n) in outputs {
            cfg.outputs[*b] = Some(n.to_string());
        }
        (Engine::start(move || Box::new(sim), cfg, params), taps)
    }

    fn tap_of(taps: &Taps, name: &str) -> Vec<f32> {
        let t = lock(taps);
        let tap = t.iter().find(|(n, _)| n == name).expect("tap").1.clone();
        let v = lock(&tap).clone();
        v
    }

    /// Cycles per second and peak of the left channel over the last `secs` of `samples`.
    fn measure(samples: &[f32], rate: f32, secs: f32) -> (f32, f32) {
        let left: Vec<f32> = samples.chunks_exact(2).map(|f| f[0]).collect();
        let n = (rate * secs) as usize;
        let tail = &left[left.len() - n..];
        let cycles = tail
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        let peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        (cycles as f32 / secs, peak)
    }

    fn params_one_strip(strip: usize, sends: [bool; BUSES]) -> EngineParams {
        let mut p = EngineParams::default();
        for (i, s) in p.strips.iter_mut().enumerate() {
            s.sends = if i == strip { sends } else { [false; BUSES] };
        }
        p
    }

    #[test]
    fn audio_flows_from_a_drifting_input_to_a_drifting_output() {
        let p = params_one_strip(0, [true, false, false, false, false]);
        let (engine, taps) = start(&[(0, MIC)], &[(0, SPEAKERS)], p);
        thread::sleep(Duration::from_millis(3500));
        let out = tap_of(&taps, SPEAKERS);
        assert!(out.len() > 48_000 * 2, "only {} samples played", out.len());
        let (freq, peak) = measure(&out, 48_000.0, 0.8);
        assert!((freq - 220.0).abs() < 3.0, "{freq} Hz");
        assert!((peak - 0.30).abs() < 0.03, "peak {peak}");
        let st = engine.status();
        assert_eq!(st.inputs[0], SlotStatus::Running { rate: 48_000 });
        assert_eq!(st.outputs[0], SlotStatus::Running { rate: 48_000 });
        assert_eq!(engine.stats().input_underruns[0], 0);
    }

    #[test]
    fn different_device_sample_rates_are_converted_both_ways() {
        // 44.1 kHz line-in -> engine (48 kHz) -> 44.1 kHz headphones.
        let p = params_one_strip(1, [false, true, false, false, false]);
        let (_engine, taps) = start(&[(1, LINE)], &[(1, HEADPHONES)], p);
        thread::sleep(Duration::from_millis(3500));
        let out = tap_of(&taps, HEADPHONES);
        let (freq, peak) = measure(&out, 44_100.0, 0.8);
        assert!((freq - 330.0).abs() < 3.0, "{freq} Hz");
        assert!((peak - 0.20).abs() < 0.03, "peak {peak}");
    }

    #[test]
    fn fader_and_routing_changes_apply_while_running() {
        let p = params_one_strip(0, [true, false, false, false, false]);
        let (engine, taps) = start(&[(0, MIC)], &[(0, SPEAKERS), (1, HEADPHONES)], p);
        thread::sleep(Duration::from_millis(2000));

        engine.update_params(|p| {
            p.strips[0].gain_db = -6.0206;
            p.strips[0].sends = [false, true, false, false, false];
        });
        thread::sleep(Duration::from_millis(2500));

        // Headphones now carry the mic at half level; speakers have gone quiet.
        let hp = tap_of(&taps, HEADPHONES);
        let (_, peak) = measure(&hp, 44_100.0, 0.5);
        assert!((peak - 0.15).abs() < 0.03, "headphones peak {peak}");
        let sp = tap_of(&taps, SPEAKERS);
        let (_, quiet) = measure(&sp, 48_000.0, 0.3);
        assert!(quiet < 0.01, "speakers still at {quiet}");
    }

    #[test]
    fn a_missing_device_is_reported_not_fatal() {
        let p = EngineParams::default();
        let (engine, _) = start(&[(0, "Nonexistent Mic")], &[(0, SPEAKERS)], p);
        thread::sleep(Duration::from_millis(700));
        let st = engine.status();
        assert_eq!(st.inputs[0], SlotStatus::Missing);
        assert_eq!(st.outputs[0], SlotStatus::Running { rate: 48_000 });
        // The device list is still published for the UI to offer.
        assert!(engine.devices().inputs.iter().any(|d| d.name == MIC));
    }

    #[test]
    fn turning_the_engine_off_closes_streams() {
        let p = EngineParams::default();
        let (engine, _) = start(&[(0, MIC)], &[(0, SPEAKERS)], p);
        thread::sleep(Duration::from_millis(700));
        assert!(matches!(
            engine.status().inputs[0],
            SlotStatus::Running { .. }
        ));
        engine.set_running(false);
        thread::sleep(Duration::from_millis(700));
        assert_eq!(engine.status().inputs[0], SlotStatus::Off);
        engine.set_running(true);
        thread::sleep(Duration::from_millis(700));
        assert!(matches!(
            engine.status().inputs[0],
            SlotStatus::Running { .. }
        ));
    }

    #[test]
    fn the_mix_thread_keeps_time() {
        let p = EngineParams::default();
        let (engine, _) = start(&[(0, MIC)], &[(0, SPEAKERS)], p);
        thread::sleep(Duration::from_millis(1500));
        let s = engine.stats();
        assert_eq!(s.late_blocks, 0);
        assert!(s.cpu_percent < 50.0, "cpu {}%", s.cpu_percent);
    }
}
