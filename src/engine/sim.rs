//! A simulated audio system: fake devices with slightly different clock rates, driven by real
//! time. It lets the whole engine (threads, rings, drift control, mixer) run on any machine, in
//! tests and in `--demo`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ringbuf::{HeapCons, HeapProd};

use super::io::{looks_virtual, AudioIo, DeviceInfo, Opened};
use super::rate::{write_frames, RateReader};

/// What a simulated input produces.
#[derive(Clone, Copy)]
pub enum Signal {
    Sine { freq: f32, amp: f32 },
    Silence,
}

struct SimDevice {
    name: &'static str,
    rate: u32,
    /// Real clock error: the device actually runs at `rate * (1 + ppm / 1e6)`.
    ppm: f64,
    signal: Signal,
}

const INPUTS: &[SimDevice] = &[
    SimDevice {
        name: "Microphone (Sim USB)",
        rate: 48_000,
        ppm: 30.0,
        signal: Signal::Sine {
            freq: 220.0,
            amp: 0.30,
        },
    },
    SimDevice {
        name: "Line In (Sim Audio)",
        rate: 44_100,
        ppm: -45.0,
        signal: Signal::Sine {
            freq: 330.0,
            amp: 0.20,
        },
    },
    SimDevice {
        name: "CABLE Output (VB-Audio Virtual Cable)",
        rate: 48_000,
        ppm: 5.0,
        signal: Signal::Sine {
            freq: 523.0,
            amp: 0.25,
        },
    },
    SimDevice {
        name: "CABLE-A Output (VB-Audio Cable A)",
        rate: 48_000,
        ppm: -8.0,
        signal: Signal::Silence,
    },
];

const OUTPUTS: &[SimDevice] = &[
    SimDevice {
        name: "Speakers (Sim Audio)",
        rate: 48_000,
        ppm: -20.0,
        signal: Signal::Silence,
    },
    SimDevice {
        name: "Headphones (Sim DAC)",
        rate: 44_100,
        ppm: 60.0,
        signal: Signal::Silence,
    },
    SimDevice {
        name: "CABLE Input (VB-Audio Virtual Cable)",
        rate: 48_000,
        ppm: 5.0,
        signal: Signal::Silence,
    },
];

/// Everything played to a simulated output, for tests to inspect.
pub type Tap = Arc<Mutex<Vec<f32>>>;

pub struct SimIo {
    taps: Arc<Mutex<Vec<(String, Tap)>>>,
}

impl SimIo {
    pub fn new() -> Self {
        Self {
            taps: Arc::default(),
        }
    }

    /// Handle for reading back what an output received. Valid after `open_output`.
    pub fn taps(&self) -> Arc<Mutex<Vec<(String, Tap)>>> {
        self.taps.clone()
    }

    fn infos(list: &[SimDevice], default: usize) -> Vec<DeviceInfo> {
        list.iter()
            .enumerate()
            .map(|(i, d)| DeviceInfo {
                name: d.name.to_string(),
                is_default: i == default,
                is_virtual: looks_virtual(d.name),
            })
            .collect()
    }
}

impl Default for SimIo {
    fn default() -> Self {
        Self::new()
    }
}

/// Stops a simulated device's thread when dropped.
struct Stop(Arc<AtomicU64>);

impl Drop for Stop {
    fn drop(&mut self) {
        self.0.store(1, Ordering::Relaxed);
    }
}

impl AudioIo for SimIo {
    fn inputs(&mut self) -> Vec<DeviceInfo> {
        Self::infos(INPUTS, 0)
    }

    fn outputs(&mut self) -> Vec<DeviceInfo> {
        Self::infos(OUTPUTS, 0)
    }

    fn open_input(&mut self, name: &str, mut prod: HeapProd<f32>) -> Result<Opened, String> {
        let dev = INPUTS
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| format!("no such device: {name}"))?;
        let stop = Arc::new(AtomicU64::new(0));
        let flag = stop.clone();
        let (rate, ppm, signal) = (dev.rate, dev.ppm, dev.signal);
        thread::spawn(move || {
            let actual = rate as f64 * (1.0 + ppm / 1e6);
            let started = Instant::now();
            let mut produced: u64 = 0;
            while flag.load(Ordering::Relaxed) == 0 {
                // Produce whatever the (drifting) hardware clock says has elapsed, in bursts the
                // way a real driver delivers them.
                let due = (started.elapsed().as_secs_f64() * actual) as u64;
                if due > produced {
                    let n = (due - produced) as usize;
                    let mut buf = Vec::with_capacity(n * 2);
                    for i in 0..n {
                        let t = (produced + i as u64) as f32 / rate as f32;
                        let v = match signal {
                            Signal::Sine { freq, amp } => {
                                amp * (2.0 * std::f32::consts::PI * freq * t).sin()
                            }
                            Signal::Silence => 0.0,
                        };
                        buf.push(v);
                        buf.push(v);
                    }
                    write_frames(&mut prod, &buf);
                    produced = due;
                }
                thread::sleep(Duration::from_millis(4));
            }
        });
        Ok(Opened {
            rate,
            keepalive: Box::new(Stop(stop)),
        })
    }

    fn open_output(
        &mut self,
        name: &str,
        cons: HeapCons<f32>,
        engine_rate: u32,
        target_ms: u32,
        underruns: Arc<AtomicU64>,
    ) -> Result<Opened, String> {
        let dev = OUTPUTS
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| format!("no such device: {name}"))?;
        let stop = Arc::new(AtomicU64::new(0));
        let flag = stop.clone();
        let (rate, ppm) = (dev.rate, dev.ppm);
        let tap: Tap = Arc::default();
        if let Ok(mut taps) = self.taps.lock() {
            taps.retain(|(n, _)| n != name);
            taps.push((name.to_string(), tap.clone()));
        }
        let target = (engine_rate as u64 * target_ms as u64 / 1000) as usize;
        thread::spawn(move || {
            let mut reader = RateReader::new(cons, engine_rate as f64, rate as f64, target);
            let actual = rate as f64 * (1.0 + ppm / 1e6);
            let started = Instant::now();
            let mut consumed: u64 = 0;
            let mut buf = Vec::new();
            while flag.load(Ordering::Relaxed) == 0 {
                let due = (started.elapsed().as_secs_f64() * actual) as u64;
                if due > consumed {
                    let n = (due - consumed) as usize;
                    buf.resize(n * 2, 0.0);
                    reader.read(&mut buf);
                    if let Ok(mut t) = tap.lock() {
                        t.extend_from_slice(&buf);
                    }
                    consumed = due;
                    underruns.store(reader.underruns, Ordering::Relaxed);
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        Ok(Opened {
            rate,
            keepalive: Box::new(Stop(stop)),
        })
    }
}
