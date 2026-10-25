//! VBAN: audio between computers over the network, the protocol Voicemeeter and VB-Audio's apps
//! use. A receiver turns packets arriving on a UDP port into an input; a sender turns an output
//! into packets for another PC. Both show up in the device lists like any sound card, so another
//! computer's audio can be a strip and a bus can play on another computer.
//!
//! Wire format (VB-Audio's published spec): a 28-byte header, then the samples.
//!
//! ```text
//! "VBAN" | rate index (low 5 bits) + protocol (high 3) | frames-1 | channels-1
//!        | sample format (low 4 bits) + codec (high 4)  | stream name (16 bytes) | counter (u32)
//! ```
//!
//! Only plain audio (protocol 0, PCM) is handled; text, serial and compressed streams are ignored.

use std::any::Any;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ringbuf::{HeapCons, HeapProd};
use serde::{Deserialize, Serialize};

use super::io::{AudioIo, DeviceInfo, Opened};
use super::rate::{hermite, write_frames, RateReader};
use super::runtime::ENGINE_RATE;

pub const HEADER_LEN: usize = 28;
/// Most sample bytes one packet may carry.
const MAX_DATA: usize = 1436;
/// What VBAN uses when nothing else is said.
pub const DEFAULT_PORT: u16 = 6980;
/// Frames per packet we send: 128 stereo f32 frames is 1 KiB, a little under 3 ms of audio.
const SEND_FRAMES: usize = 128;

const RATES: [u32; 21] = [
    6000, 12000, 24000, 48000, 96000, 192000, 384000, 8000, 16000, 32000, 64000, 128000, 256000,
    512000, 11025, 22050, 44100, 88200, 176400, 352800, 705600,
];

fn rate_index(rate: u32) -> Option<u8> {
    RATES.iter().position(|r| *r == rate).map(|i| i as u8)
}

/// What a packet says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub stream: String,
    pub rate: u32,
    pub channels: usize,
    pub frames: usize,
    pub counter: u32,
}

/// Build one packet of stereo `f32` audio (at most 179 frames, which is what fits). `None` if
/// `rate` isn't one VBAN can name or there's too much audio.
pub fn encode(stream: &str, counter: u32, rate: u32, stereo: &[f32]) -> Option<Vec<u8>> {
    let frames = stereo.len() / 2;
    if frames == 0 || frames > 256 || frames * 8 > MAX_DATA {
        return None;
    }
    let mut p = Vec::with_capacity(HEADER_LEN + frames * 8);
    p.extend_from_slice(b"VBAN");
    p.push(rate_index(rate)?); // protocol 0 (audio) in the top bits
    p.push((frames - 1) as u8);
    p.push(1); // two channels
    p.push(4); // 32-bit float, PCM
    let mut name = [0u8; 16];
    for (dst, src) in name.iter_mut().zip(stream.bytes().take(15)) {
        *dst = src;
    }
    p.extend_from_slice(&name);
    p.extend_from_slice(&counter.to_le_bytes());
    for s in &stereo[..frames * 2] {
        p.extend_from_slice(&s.to_le_bytes());
    }
    Some(p)
}

/// Read a packet into `out` as interleaved stereo `f32` (cleared first). Mono is copied to both
/// sides and channels past the second are dropped. `None` for anything that isn't plain audio
/// we understand.
pub fn decode(packet: &[u8], out: &mut Vec<f32>) -> Option<Header> {
    out.clear();
    if packet.len() <= HEADER_LEN || &packet[..4] != b"VBAN" {
        return None;
    }
    let (sr, nbs, nbc, bits) = (packet[4], packet[5], packet[6], packet[7]);
    if sr & 0xe0 != 0 || bits & 0xf0 != 0 {
        return None; // not audio, or not PCM
    }
    let rate = *RATES.get((sr & 0x1f) as usize)?;
    let channels = nbc as usize + 1;
    let width = match bits & 0x0f {
        0 => 1,
        1 => 2,
        2 => 3,
        3 | 4 => 4,
        5 => 8,
        _ => return None, // 10 and 12 bit
    };
    let data = &packet[HEADER_LEN..];
    // Trust the bytes we actually got over what the header claims.
    let frames = (nbs as usize + 1).min(data.len() / (channels * width));
    if frames == 0 {
        return None;
    }
    let sample = |at: usize| -> f32 {
        let b = &data[at * width..(at + 1) * width];
        match bits & 0x0f {
            0 => (f32::from(b[0]) - 128.0) / 128.0,
            1 => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
            2 => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
            3 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            4 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            _ => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32,
        }
    };
    out.reserve(frames * 2);
    for f in 0..frames {
        let left = sample(f * channels);
        let right = if channels > 1 {
            sample(f * channels + 1)
        } else {
            left
        };
        out.push(left);
        out.push(right);
    }
    let name_bytes = &packet[8..24];
    let end = name_bytes.iter().position(|b| *b == 0).unwrap_or(16);
    Some(Header {
        stream: String::from_utf8_lossy(&name_bytes[..end]).into_owned(),
        rate,
        channels,
        frames,
        counter: u32::from_le_bytes([packet[24], packet[25], packet[26], packet[27]]),
    })
}

/// Streaming sample-rate converter for packets that arrive at a rate other than the engine's.
/// Cubic interpolation, same as the rest of the engine.
struct Converter {
    /// Input frames per output frame.
    step: f64,
    frac: f64,
    hist: [[f32; 2]; 4],
}

impl Converter {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self {
            step: f64::from(in_rate) / f64::from(out_rate),
            frac: 0.0,
            hist: [[0.0; 2]; 4],
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        for frame in input.chunks_exact(2) {
            self.hist.rotate_left(1);
            self.hist[3] = [frame[0], frame[1]];
            while self.frac < 1.0 {
                let t = self.frac as f32;
                for ch in 0..2 {
                    out.push(hermite(
                        self.hist[0][ch],
                        self.hist[1][ch],
                        self.hist[2][ch],
                        self.hist[3][ch],
                        t,
                    ));
                }
                self.frac += self.step;
            }
            self.frac -= 1.0;
        }
    }
}

/// Listen on a port for a stream. Appears as an input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VbanIn {
    /// Only take packets with this stream name; empty takes any.
    pub stream: String,
    pub port: u16,
}

impl Default for VbanIn {
    fn default() -> Self {
        Self {
            stream: String::new(),
            port: DEFAULT_PORT,
        }
    }
}

/// Send an output to another PC. Appears as an output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VbanOut {
    pub stream: String,
    /// The other PC's name or address.
    pub host: String,
    pub port: u16,
}

impl Default for VbanOut {
    fn default() -> Self {
        Self {
            stream: "Wavemix".into(),
            host: String::new(),
            port: DEFAULT_PORT,
        }
    }
}

pub const IN_PREFIX: &str = "VBAN in: ";
pub const OUT_PREFIX: &str = "VBAN out: ";

impl VbanIn {
    pub fn device_name(&self) -> String {
        if self.stream.is_empty() {
            format!("{IN_PREFIX}any stream on port {}", self.port)
        } else {
            format!("{IN_PREFIX}{} on port {}", self.stream, self.port)
        }
    }
}

impl VbanOut {
    pub fn device_name(&self) -> String {
        format!("{OUT_PREFIX}{} to {}:{}", self.stream, self.host, self.port)
    }
}

/// The network endpoints the user has set up.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VbanConfig {
    pub receivers: Vec<VbanIn>,
    pub senders: Vec<VbanOut>,
}

/// Shared between the settings and the engine's I/O thread.
pub type Shared = Arc<Mutex<VbanConfig>>;

/// Adds the network endpoints to whatever devices `inner` provides.
pub struct VbanIo {
    inner: Box<dyn AudioIo>,
    config: Shared,
}

impl VbanIo {
    pub fn new(inner: Box<dyn AudioIo>, config: Shared) -> Self {
        Self { inner, config }
    }

    fn config(&self) -> VbanConfig {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

fn info(name: String) -> DeviceInfo {
    DeviceInfo {
        name,
        is_default: false,
        is_virtual: false,
        is_loopback: false,
    }
}

/// A background thread that stops (and is waited for) when dropped.
struct Worker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl AudioIo for VbanIo {
    fn inputs(&mut self) -> Vec<DeviceInfo> {
        let mut list = self.inner.inputs();
        list.extend(
            self.config()
                .receivers
                .iter()
                .map(|r| info(r.device_name())),
        );
        list
    }

    fn outputs(&mut self) -> Vec<DeviceInfo> {
        let mut list = self.inner.outputs();
        list.extend(
            self.config()
                .senders
                .iter()
                .filter(|s| !s.host.trim().is_empty())
                .map(|s| info(s.device_name())),
        );
        list
    }

    fn open_input(&mut self, name: &str, prod: HeapProd<f32>) -> Result<Opened, String> {
        let Some(r) = self
            .config()
            .receivers
            .into_iter()
            .find(|r| r.device_name() == name)
        else {
            return self.inner.open_input(name, prod);
        };
        let socket = UdpSocket::bind(("0.0.0.0", r.port))
            .map_err(|e| format!("Couldn't listen on port {}: {e}", r.port))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("vban-in".into())
                .spawn(move || receive(socket, &r.stream, prod, &stop))
                .map_err(|e| e.to_string())?
        };
        Ok(Opened {
            rate: ENGINE_RATE,
            loopback: false,
            keepalive: Box::new(Worker {
                stop,
                handle: Some(handle),
            }) as Box<dyn Any>,
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
        let Some(s) = self
            .config()
            .senders
            .into_iter()
            .find(|s| s.device_name() == name)
        else {
            return self
                .inner
                .open_output(name, cons, engine_rate, target_ms, underruns);
        };
        let addr = (s.host.trim(), s.port)
            .to_socket_addrs()
            .map_err(|e| format!("Can't find {}: {e}", s.host))?
            .next()
            .ok_or_else(|| format!("Can't find {}", s.host))?;
        let socket = UdpSocket::bind(if addr.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .map_err(|e| e.to_string())?;
        socket.connect(addr).map_err(|e| e.to_string())?;
        let target = (u64::from(engine_rate) * u64::from(target_ms) / 1000) as usize;
        let reader = RateReader::new(cons, f64::from(engine_rate), f64::from(engine_rate), target);
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("vban-out".into())
                .spawn(move || send(socket, &s.stream, engine_rate, reader, &underruns, &stop))
                .map_err(|e| e.to_string())?
        };
        Ok(Opened {
            rate: engine_rate,
            loopback: false,
            keepalive: Box::new(Worker {
                stop,
                handle: Some(handle),
            }),
        })
    }
}

fn receive(socket: UdpSocket, stream: &str, mut prod: HeapProd<f32>, stop: &AtomicBool) {
    let mut buf = vec![0u8; 2048];
    let mut samples = Vec::with_capacity(512);
    let mut converted = Vec::with_capacity(1024);
    let mut converter: Option<(u32, Converter)> = None;
    while !stop.load(Ordering::Acquire) {
        let n = match socket.recv(&mut buf) {
            Ok(n) => n,
            // Windows reports a read timeout as TimedOut, other systems as WouldBlock.
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
        };
        let Some(h) = decode(&buf[..n], &mut samples) else {
            continue;
        };
        if !stream.is_empty() && !h.stream.eq_ignore_ascii_case(stream) {
            continue;
        }
        if h.rate == ENGINE_RATE {
            write_frames(&mut prod, &samples);
            continue;
        }
        if converter.as_ref().map(|(r, _)| *r) != Some(h.rate) {
            converter = Some((h.rate, Converter::new(h.rate, ENGINE_RATE)));
        }
        if let Some((_, c)) = converter.as_mut() {
            c.process(&samples, &mut converted);
            write_frames(&mut prod, &converted);
        }
    }
}

fn send(
    socket: UdpSocket,
    stream: &str,
    rate: u32,
    mut reader: RateReader,
    underruns: &AtomicU64,
    stop: &AtomicBool,
) {
    let period = Duration::from_secs_f64(SEND_FRAMES as f64 / f64::from(rate));
    let sleeper = spin_sleep::SpinSleeper::default();
    let mut block = vec![0.0f32; SEND_FRAMES * 2];
    let mut counter = 0u32;
    let mut next = Instant::now();
    while !stop.load(Ordering::Acquire) {
        reader.read(&mut block);
        underruns.store(reader.underruns, Ordering::Relaxed);
        if let Some(packet) = encode(stream, counter, rate, &block) {
            // The other PC being away is normal; there is nobody to tell.
            let _ = socket.send(&packet);
        }
        counter = counter.wrapping_add(1);
        next += period;
        let now = Instant::now();
        if next > now {
            sleeper.sleep_until(next);
        } else if now - next > Duration::from_millis(50) {
            // Stalled for a while: carry on from now rather than sending a burst to catch up.
            next = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::rate::ring;
    use crate::engine::sim::SimIo;
    use ringbuf::traits::{Consumer, Observer};

    fn sine(frames: usize, hz: f32, rate: f32, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|n| {
                let v = amp * (2.0 * std::f32::consts::PI * hz * n as f32 / rate).sin();
                [v, v]
            })
            .collect()
    }

    #[test]
    fn a_packet_survives_encoding_and_decoding() {
        let audio = sine(100, 440.0, 48000.0, 0.5);
        let packet = encode("Mic", 7, 48000, &audio).unwrap();
        assert_eq!(&packet[..4], b"VBAN");
        assert_eq!(packet.len(), HEADER_LEN + 100 * 8);
        let mut out = Vec::new();
        let h = decode(&packet, &mut out).unwrap();
        assert_eq!(
            h,
            Header {
                stream: "Mic".into(),
                rate: 48000,
                channels: 2,
                frames: 100,
                counter: 7
            }
        );
        assert_eq!(out, audio);
    }

    #[test]
    fn packets_that_are_not_plain_audio_are_ignored() {
        let mut out = Vec::new();
        assert!(decode(b"nope", &mut out).is_none());
        assert!(decode(&[0u8; 100], &mut out).is_none(), "no VBAN marker");
        let mut p = encode("x", 0, 48000, &sine(10, 1000.0, 48000.0, 0.1)).unwrap();
        p[4] |= 0x20; // a serial or text protocol
        assert!(decode(&p, &mut out).is_none());
        let mut p = encode("x", 0, 48000, &sine(10, 1000.0, 48000.0, 0.1)).unwrap();
        p[7] |= 0x10; // a compressed codec
        assert!(decode(&p, &mut out).is_none());
        let mut p = encode("x", 0, 48000, &sine(10, 1000.0, 48000.0, 0.1)).unwrap();
        p[4] = 31; // a rate index past the table
        assert!(decode(&p, &mut out).is_none());
        assert!(encode("x", 0, 12345, &[0.0, 0.0]).is_none(), "unnamed rate");
        assert!(
            encode("x", 0, 48000, &vec![0.0; 2 * 200]).is_none(),
            "too big for a packet"
        );
    }

    #[test]
    fn other_sample_formats_and_channel_counts_are_read() {
        let header = |bits: u8, channels: u8, frames: u8| {
            let mut h = b"VBAN".to_vec();
            h.extend([3, frames - 1, channels - 1, bits]);
            h.extend([b'S', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            h.extend(0u32.to_le_bytes());
            h
        };
        let mut out = Vec::new();

        // 16-bit stereo.
        let mut p = header(1, 2, 2);
        for v in [16384i16, -16384, 32767, 0] {
            p.extend(v.to_le_bytes());
        }
        decode(&p, &mut out).unwrap();
        assert_eq!(&out[..2], &[0.5, -0.5]);
        assert!((out[2] - 1.0).abs() < 0.001 && out[3] == 0.0);

        // 24-bit mono is copied to both sides, and the sign is kept.
        let mut p = header(2, 1, 1);
        p.extend([0x00, 0x00, 0xc0]); // -0x400000
        decode(&p, &mut out).unwrap();
        assert_eq!(out, [-0.5, -0.5]);

        // 8-bit is unsigned around 128.
        let mut p = header(0, 2, 1);
        p.extend([128, 255]);
        decode(&p, &mut out).unwrap();
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 0.992).abs() < 0.001);

        // 32-bit integer and 64-bit float.
        let mut p = header(3, 2, 1);
        p.extend((i32::MAX / 2).to_le_bytes());
        p.extend(i32::MIN.to_le_bytes());
        decode(&p, &mut out).unwrap();
        assert!((out[0] - 0.5).abs() < 0.001 && out[1] == -1.0);
        let mut p = header(5, 2, 1);
        p.extend(0.25f64.to_le_bytes());
        p.extend((-0.75f64).to_le_bytes());
        decode(&p, &mut out).unwrap();
        assert_eq!(out, [0.25, -0.75]);

        // Surround: the first two channels are the front pair.
        let mut p = header(4, 6, 1);
        for v in [0.1f32, 0.2, 0.9, 0.9, 0.9, 0.9] {
            p.extend(v.to_le_bytes());
        }
        decode(&p, &mut out).unwrap();
        assert_eq!(out, [0.1, 0.2]);

        // A packet that is shorter than it claims gives only the frames it holds.
        let mut p = header(4, 2, 100);
        p.extend(0.5f32.to_le_bytes());
        p.extend(0.5f32.to_le_bytes());
        assert_eq!(decode(&p, &mut out).unwrap().frames, 1);
    }

    #[test]
    fn a_stream_at_another_rate_comes_out_at_the_same_pitch() {
        // 1 kHz at 44.1 kHz, converted to 48 kHz: should still be 1 kHz at the same level.
        let input = sine(44_100, 1000.0, 44_100.0, 0.5);
        let mut c = Converter::new(44_100, 48_000);
        let mut out = Vec::new();
        c.process(&input, &mut out);
        let frames = out.len() / 2;
        assert!(
            (frames as i64 - 48_000).abs() <= 2,
            "a second in is a second out, got {frames}"
        );
        let left: Vec<f32> = out.chunks_exact(2).skip(100).map(|f| f[0]).collect();
        let crossings = left
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        let seconds = left.len() as f32 / 48_000.0;
        assert!(
            (crossings as f32 / seconds - 1000.0).abs() < 5.0,
            "pitch changed: {} Hz",
            crossings as f32 / seconds
        );
        let peak = left.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.02, "level changed: {peak}");
    }

    #[test]
    fn names_say_where_the_audio_comes_from_and_goes() {
        let r = VbanIn {
            stream: "Mic".into(),
            port: 6981,
        };
        assert_eq!(r.device_name(), "VBAN in: Mic on port 6981");
        assert_eq!(
            VbanIn::default().device_name(),
            "VBAN in: any stream on port 6980"
        );
        let s = VbanOut {
            stream: "Wavemix".into(),
            host: "192.168.1.20".into(),
            port: 6980,
        };
        assert_eq!(s.device_name(), "VBAN out: Wavemix to 192.168.1.20:6980");
    }

    #[test]
    fn sound_sent_to_a_port_arrives_from_it() {
        // Find a free port, then let both ends of the link use it on this machine.
        let port = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let recv = VbanIn {
            stream: "test".into(),
            port,
        };
        let send = VbanOut {
            stream: "test".into(),
            host: "127.0.0.1".into(),
            port,
        };
        let (rx_name, tx_name) = (recv.device_name(), send.device_name());
        let config: Shared = Arc::new(Mutex::new(VbanConfig {
            receivers: vec![recv],
            senders: vec![send],
        }));
        let mut io = VbanIo::new(Box::new(SimIo::new()), config);
        assert!(io.inputs().iter().any(|d| d.name == rx_name));
        assert!(io.outputs().iter().any(|d| d.name == tx_name));

        let (in_prod, mut in_cons) = ring(48_000);
        let _rx = io.open_input(&rx_name, in_prod).unwrap();
        let (mut out_prod, out_cons) = ring(48_000);
        let _tx = io
            .open_output(&tx_name, out_cons, 48_000, 20, Arc::new(AtomicU64::new(0)))
            .unwrap();

        // Half a second of a quiet 1 kHz tone.
        let tone = sine(24_000, 1000.0, 48_000.0, 0.5);
        write_frames(&mut out_prod, &tone);

        let started = Instant::now();
        while in_cons.occupied_len() < 2 * 20_000 && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut got = vec![0.0f32; in_cons.occupied_len()];
        let n = in_cons.pop_slice(&mut got);
        got.truncate(n);
        assert!(
            got.len() >= 2 * 20_000,
            "only {} frames arrived",
            got.len() / 2
        );
        // Skip the silence the sender plays while it fills its buffer, then expect the tone.
        let loud: Vec<f32> = got
            .chunks_exact(2)
            .map(|f| f[0])
            .filter(|v| v.abs() > 0.01)
            .collect();
        assert!(
            loud.len() > 10_000,
            "the tone didn't arrive: {} loud samples",
            loud.len()
        );
        let rms = (loud.iter().map(|v| v * v).sum::<f32>() / loud.len() as f32).sqrt();
        assert!((rms - 0.5 / 2f32.sqrt()).abs() < 0.05, "level off: {rms}");
    }

    #[test]
    fn another_streams_packets_are_not_let_in() {
        let port = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let recv = VbanIn {
            stream: "wanted".into(),
            port,
        };
        let name = recv.device_name();
        let config: Shared = Arc::new(Mutex::new(VbanConfig {
            receivers: vec![recv],
            senders: vec![],
        }));
        let mut io = VbanIo::new(Box::new(SimIo::new()), config);
        let (prod, cons) = ring(48_000);
        let _rx = io.open_input(&name, prod).unwrap();

        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let tone = sine(128, 1000.0, 48_000.0, 0.5);
        for _ in 0..5 {
            let other = encode("unwanted", 0, 48_000, &tone).unwrap();
            socket.send_to(&other, ("127.0.0.1", port)).unwrap();
        }
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(cons.occupied_len(), 0);

        let mine = encode("WANTED", 0, 48_000, &tone).unwrap();
        socket.send_to(&mine, ("127.0.0.1", port)).unwrap();
        let started = Instant::now();
        while cons.occupied_len() == 0 && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            cons.occupied_len(),
            128 * 2,
            "names match regardless of case"
        );
    }

    #[test]
    fn a_port_in_use_is_reported_not_hidden() {
        let taken = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let recv = VbanIn {
            stream: String::new(),
            port,
        };
        let name = recv.device_name();
        let config: Shared = Arc::new(Mutex::new(VbanConfig {
            receivers: vec![recv],
            senders: vec![],
        }));
        let mut io = VbanIo::new(Box::new(SimIo::new()), config);
        let (prod, _cons) = ring(1024);
        let err = io.open_input(&name, prod).err().expect("should fail");
        assert!(err.contains(&port.to_string()), "{err}");
    }
}
