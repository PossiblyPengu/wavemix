//! Noise suppression for microphones, using RNNoise (a small neural network trained to tell
//! speech from noise) through the pure-Rust `nnnoiseless` port. It works on 10 ms mono frames at
//! 48 kHz, which is exactly the size of one mixer block.

use nnnoiseless::DenoiseState;

/// Samples per frame RNNoise works on.
pub const FRAME: usize = DenoiseState::FRAME_SIZE;

pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    /// The first frame out of RNNoise is a fade-in artefact, so it is replaced with silence.
    first: bool,
    input: [f32; FRAME],
    output: [f32; FRAME],
}

impl Denoiser {
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            first: true,
            input: [0.0; FRAME],
            output: [0.0; FRAME],
        }
    }

    /// Clean `buf` (interleaved stereo, exactly one frame long) in place. The stereo pair is
    /// mixed to mono first, and the cleaned voice is sent to both sides. Any other length is left
    /// alone.
    pub fn process(&mut self, buf: &mut [f32]) {
        if buf.len() != FRAME * 2 {
            return;
        }
        // RNNoise expects samples on a 16-bit scale.
        for (i, f) in buf.chunks_exact(2).enumerate() {
            self.input[i] = (f[0] + f[1]) * 0.5 * 32768.0;
        }
        self.state.process_frame(&mut self.output, &self.input);
        if self.first {
            self.first = false;
            buf.fill(0.0);
            return;
        }
        for (i, f) in buf.chunks_exact_mut(2).enumerate() {
            let v = self.output[i] / 32768.0;
            f[0] = v;
            f[1] = v;
        }
    }
}

impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap deterministic white noise.
    fn noise(seed: &mut u32) -> f32 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        (*seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|v| v * v).sum::<f32>() / buf.len() as f32).sqrt()
    }

    #[test]
    fn steady_noise_is_turned_down() {
        let mut d = Denoiser::new();
        let mut seed = 0x1234_5678u32;
        let mut last_in = 0.0;
        let mut last_out = 0.0;
        // Two seconds of hiss; judge the last frames, once the network has settled.
        for n in 0..200 {
            let mut buf: Vec<f32> = (0..FRAME * 2).map(|_| 0.05 * noise(&mut seed)).collect();
            let before = rms(&buf);
            d.process(&mut buf);
            if n >= 180 {
                last_in += before;
                last_out += rms(&buf);
            }
        }
        assert!(
            last_out < last_in * 0.5,
            "noise only fell from {last_in} to {last_out}"
        );
    }

    #[test]
    fn other_block_sizes_pass_through_untouched() {
        let mut d = Denoiser::new();
        let mut buf = vec![0.25f32; 100];
        d.process(&mut buf);
        assert!(buf.iter().all(|v| *v == 0.25));
    }
}
