//! Recording a bus to a WAV file. The mixer hands blocks to a ring; a writer thread drains the
//! ring to disk, so the audio thread never touches a file.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ringbuf::traits::Consumer;
use ringbuf::HeapProd;

use super::rate::ring;
use super::runtime::ENGINE_RATE;

const CHANNELS: u16 = 2;
const BITS: u16 = 24;

/// A stereo 24-bit PCM WAV file being written.
pub struct WavWriter {
    out: BufWriter<File>,
    frames: u64,
}

impl WavWriter {
    pub fn create(path: &Path, rate: u32) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        Self::write_header(&mut out, rate, 0)?;
        Ok(Self { out, frames: 0 })
    }

    fn write_header(out: &mut impl Write, rate: u32, frames: u64) -> io::Result<()> {
        let block_align = CHANNELS * BITS / 8;
        let data_len = (frames * block_align as u64).min(u32::MAX as u64 - 36) as u32;
        out.write_all(b"RIFF")?;
        out.write_all(&(36 + data_len).to_le_bytes())?;
        out.write_all(b"WAVEfmt ")?;
        out.write_all(&16u32.to_le_bytes())?;
        out.write_all(&1u16.to_le_bytes())?; // PCM
        out.write_all(&CHANNELS.to_le_bytes())?;
        out.write_all(&rate.to_le_bytes())?;
        out.write_all(&(rate * block_align as u32).to_le_bytes())?;
        out.write_all(&block_align.to_le_bytes())?;
        out.write_all(&BITS.to_le_bytes())?;
        out.write_all(b"data")?;
        out.write_all(&data_len.to_le_bytes())
    }

    /// Append interleaved stereo samples in -1.0..=1.0 (louder is clipped).
    pub fn write(&mut self, samples: &[f32]) -> io::Result<()> {
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            self.out.write_all(&v.to_le_bytes()[..3])?;
        }
        self.frames += (samples.len() / CHANNELS as usize) as u64;
        Ok(())
    }

    /// Fix the sizes in the header and close the file. Returns the number of frames written.
    pub fn finish(mut self, rate: u32) -> io::Result<u64> {
        self.out.flush()?;
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(0))?;
        Self::write_header(file, rate, self.frames)?;
        file.flush()?;
        Ok(self.frames)
    }
}

/// A recording in progress.
pub struct Recording {
    pub bus: usize,
    pub path: PathBuf,
    frames: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<io::Result<u64>>>,
}

impl Recording {
    /// Open `path` and start the writer. The returned producer is what the mixer fills.
    pub fn start(bus: usize, path: PathBuf) -> io::Result<(Recording, HeapProd<f32>)> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut writer = WavWriter::create(&path, ENGINE_RATE)?;
        let (prod, mut cons) = ring(ENGINE_RATE as usize);
        let frames = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (f, s) = (frames.clone(), stop.clone());
        let join = thread::Builder::new()
            .name("record".into())
            .spawn(move || {
                let mut chunk = vec![0.0f32; 4800];
                loop {
                    let n = cons.pop_slice(&mut chunk) / 2 * 2;
                    if n > 0 {
                        writer.write(&chunk[..n])?;
                        f.store(
                            f.load(Ordering::Relaxed) + (n / 2) as u64,
                            Ordering::Relaxed,
                        );
                    } else if s.load(Ordering::Acquire) {
                        break;
                    } else {
                        thread::sleep(Duration::from_millis(20));
                    }
                }
                writer.finish(ENGINE_RATE)
            })?;
        Ok((
            Recording {
                bus,
                path,
                frames,
                stop,
                join: Some(join),
            },
            prod,
        ))
    }

    pub fn seconds(&self) -> f64 {
        self.frames.load(Ordering::Relaxed) as f64 / ENGINE_RATE as f64
    }

    /// Stop, write out what is left, and close the file. Returns the length in seconds.
    pub fn finish(mut self) -> io::Result<f64> {
        self.stop.store(true, Ordering::Release);
        let frames = match self.join.take() {
            Some(j) => j
                .join()
                .map_err(|_| io::Error::other("the recording thread failed"))??,
            None => 0,
        };
        Ok(frames as f64 / ENGINE_RATE as f64)
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        // Signal the writer thread to drain and finish the file. This covers process exit and
        // panics: the thread calls `writer.finish()` before it returns, so the WAV header always
        // gets the correct size even if `Recording::finish` was never called explicitly.
        self.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::Producer;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wavemix-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn a_wav_file_has_a_valid_header_and_round_trips_samples() {
        let path = temp("a.wav");
        let mut w = WavWriter::create(&path, 48_000).unwrap();
        let samples: Vec<f32> = (0..2000).map(|i| (i as f32 / 2000.0) * 0.8 - 0.4).collect();
        w.write(&samples).unwrap();
        assert_eq!(w.finish(48_000).unwrap(), 1000);

        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2); // channels
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(u16::from_le_bytes([bytes[34], bytes[35]]), 24); // bits
        let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(data_len, 1000 * 6);
        assert_eq!(bytes.len(), 44 + data_len);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize,
            36 + data_len
        );
        // The first sample: -0.4 in 24-bit, little-endian signed.
        let first = i32::from_le_bytes([0, bytes[44], bytes[45], bytes[46]]) >> 8;
        assert!((first as f32 / 8_388_607.0 + 0.4).abs() < 1e-4);
    }

    #[test]
    fn a_recording_collects_what_the_mixer_pushes() {
        let path = temp("b.wav");
        let (rec, mut prod) = Recording::start(0, path.clone()).unwrap();
        for _ in 0..10 {
            let block = vec![0.25f32; 960];
            assert_eq!(prod.push_slice(&block), 960);
        }
        // Give the writer a moment to drain.
        thread::sleep(Duration::from_millis(150));
        assert!(rec.seconds() > 0.0);
        let secs = rec.finish().unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let _ = std::fs::remove_file(&path);
        assert!((secs - 0.1).abs() < 1e-6, "{secs}");
        assert_eq!(size, 44 + 4800 * 6);
    }
}
