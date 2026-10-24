//! Real audio devices on Windows (WASAPI shared mode, through cpal).
//!
//! Devices are matched by their display name. Whatever format a device reports is converted to
//! and from `f32` stereo: extra channels are ignored on input and silenced on output, and mono
//! devices get the left/right average or the same signal on both sides.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, FromSample, SampleFormat, SizedSample, StreamConfig};
use ringbuf::{HeapCons, HeapProd};

use super::io::{looks_virtual, loopback_name, loopback_target, AudioIo, DeviceInfo, Opened};
use super::rate::{write_frames, RateReader};

pub struct CpalIo {
    host: cpal::Host,
}

impl CpalIo {
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    fn name_of(device: &Device) -> Option<String> {
        device.description().ok().map(|d| d.name().to_string())
    }

    /// Resolve an input name to a device: a microphone, or (for "System audio: X") the playback
    /// device X, which cpal captures in loopback mode.
    fn find_input(&self, name: &str) -> Option<(Device, bool)> {
        match loopback_target(name) {
            Some(output) => self.find(output, false).map(|d| (d, true)),
            None => self.find(name, true).map(|d| (d, false)),
        }
    }

    fn find(&self, name: &str, input: bool) -> Option<Device> {
        let mut devices: Box<dyn Iterator<Item = Device>> = if input {
            Box::new(self.host.input_devices().ok()?)
        } else {
            Box::new(self.host.output_devices().ok()?)
        };
        devices.find(|d| Self::name_of(d).as_deref() == Some(name))
    }

    fn list(&self, input: bool) -> Vec<DeviceInfo> {
        let default = if input {
            self.host.default_input_device()
        } else {
            self.host.default_output_device()
        }
        .and_then(|d| Self::name_of(&d));
        let devices: Vec<Device> = if input {
            self.host
                .input_devices()
                .map(|d| d.collect())
                .unwrap_or_default()
        } else {
            self.host
                .output_devices()
                .map(|d| d.collect())
                .unwrap_or_default()
        };
        let mut list: Vec<DeviceInfo> = devices
            .iter()
            .filter_map(Self::name_of)
            .map(|name| DeviceInfo {
                is_default: default.as_deref() == Some(name.as_str()),
                is_virtual: looks_virtual(&name),
                is_loopback: false,
                name,
            })
            .collect();
        // Every playback device can also be listened to as an input: "what you hear".
        if input {
            let outputs: Vec<DeviceInfo> = self.list(false);
            list.extend(outputs.into_iter().map(|o| DeviceInfo {
                name: loopback_name(&o.name),
                is_default: o.is_default,
                is_virtual: false,
                is_loopback: true,
            }));
        }
        list
    }
}

impl Default for CpalIo {
    fn default() -> Self {
        Self::new()
    }
}

fn describe(e: cpal::Error) -> String {
    e.to_string()
}

impl AudioIo for CpalIo {
    fn inputs(&mut self) -> Vec<DeviceInfo> {
        self.list(true)
    }

    fn outputs(&mut self) -> Vec<DeviceInfo> {
        self.list(false)
    }

    fn open_input(&mut self, name: &str, prod: HeapProd<f32>) -> Result<Opened, String> {
        let (device, loopback) = self
            .find_input(name)
            .ok_or_else(|| format!("{name} isn't available"))?;
        // A playback device has no "input" format of its own; its mix format is what loopback
        // delivers.
        let supported = if loopback {
            device.default_output_config()
        } else {
            device.default_input_config()
        }
        .map_err(describe)?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_input::<f32>(&device, config, channels, prod)?,
            SampleFormat::I16 => build_input::<i16>(&device, config, channels, prod)?,
            SampleFormat::I32 => build_input::<i32>(&device, config, channels, prod)?,
            SampleFormat::U16 => build_input::<u16>(&device, config, channels, prod)?,
            other => return Err(format!("unsupported sample format {other:?}")),
        };
        stream.play().map_err(describe)?;
        Ok(Opened {
            rate,
            loopback,
            keepalive: Box::new(stream),
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
        let device = self
            .find(name, false)
            .ok_or_else(|| format!("{name} isn't available"))?;
        let supported = device.default_output_config().map_err(describe)?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let config = supported.config();
        let target = (engine_rate as u64 * target_ms as u64 / 1000) as usize;
        let reader = RateReader::new(cons, engine_rate as f64, rate as f64, target);
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build_output::<f32>(&device, config, channels, reader, underruns)?,
            SampleFormat::I16 => build_output::<i16>(&device, config, channels, reader, underruns)?,
            SampleFormat::I32 => build_output::<i32>(&device, config, channels, reader, underruns)?,
            SampleFormat::U16 => build_output::<u16>(&device, config, channels, reader, underruns)?,
            other => return Err(format!("unsupported sample format {other:?}")),
        };
        stream.play().map_err(describe)?;
        Ok(Opened {
            rate,
            loopback: false,
            keepalive: Box::new(stream),
        })
    }
}

fn build_input<T>(
    device: &Device,
    config: StreamConfig,
    channels: usize,
    mut prod: HeapProd<f32>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let channels = channels.max(1);
    // Sized up front so the audio callback never allocates.
    let mut scratch: Vec<f32> = Vec::with_capacity(16_384);
    device
        .build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                scratch.clear();
                for frame in data.chunks_exact(channels) {
                    let left: f32 = frame[0].to_sample();
                    let right: f32 = if channels > 1 {
                        frame[1].to_sample()
                    } else {
                        left
                    };
                    scratch.push(left);
                    scratch.push(right);
                }
                write_frames(&mut prod, &scratch);
            },
            |_err| {},
            None,
        )
        .map_err(describe)
}

fn build_output<T>(
    device: &Device,
    config: StreamConfig,
    channels: usize,
    mut reader: RateReader,
    underruns: Arc<AtomicU64>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + Send + 'static + FromSample<f32>,
{
    let channels = channels.max(1);
    let mut scratch: Vec<f32> = vec![0.0; 16_384];
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                let frames = data.len() / channels;
                if scratch.len() < frames * 2 {
                    scratch.resize(frames * 2, 0.0);
                }
                reader.read(&mut scratch[..frames * 2]);
                for (i, frame) in data.chunks_exact_mut(channels).enumerate() {
                    let (l, r) = (scratch[i * 2], scratch[i * 2 + 1]);
                    if channels == 1 {
                        frame[0] = T::from_sample((l + r) * 0.5);
                    } else {
                        frame[0] = T::from_sample(l);
                        frame[1] = T::from_sample(r);
                        for extra in &mut frame[2..] {
                            *extra = T::EQUILIBRIUM;
                        }
                    }
                }
                underruns.store(reader.underruns, Ordering::Relaxed);
            },
            |_err| {},
            None,
        )
        .map_err(describe)
}
