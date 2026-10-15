pub mod mixer;
pub mod probe;
pub mod resampler;

#[cfg(test)]
mod allocation_tests;

/// Offline PCM primitive. No device transport, resampling or drift correction yet.
/// One gain step per frame keeps the interleaved channels at identical gain.
pub struct GainRamp {
    current: f32,
    target: f32,
    step: f32,
    remaining: usize,
}
impl GainRamp {
    pub fn new(gain: f32) -> Result<Self, &'static str> {
        validate(gain)?;
        Ok(Self {
            current: gain,
            target: gain,
            step: 0.0,
            remaining: 0,
        })
    }
    pub fn set_target(&mut self, gain: f32, frames: usize) -> Result<(), &'static str> {
        validate(gain)?;
        self.target = gain;
        self.remaining = frames;
        if frames == 0 {
            self.current = gain;
            self.step = 0.0;
        } else {
            self.step = (gain - self.current) / frames as f32;
        }
        Ok(())
    }
    pub fn apply(&mut self, interleaved: &mut [f32], channels: usize) -> Result<(), &'static str> {
        if channels == 0 || !interleaved.len().is_multiple_of(channels) {
            return Err("invalid frame layout");
        }
        for frame in interleaved.chunks_exact_mut(channels) {
            if self.remaining > 0 {
                self.current += self.step;
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.current = self.target;
                }
            }
            for sample in frame {
                *sample = if sample.is_finite() {
                    *sample * self.current
                } else {
                    0.0
                };
            }
        }
        Ok(())
    }
}
fn validate(gain: f32) -> Result<(), &'static str> {
    if gain.is_finite() && (0.0..=1.0).contains(&gain) {
        Ok(())
    } else {
        Err("gain must be finite and within 0..=1")
    }
}

/// Sum into the caller-owned output; clamp ONCE after all groups have been added.
pub fn mix_add(output: &mut [f32], input: &[f32]) -> Result<(), &'static str> {
    if output.len() != input.len() {
        return Err("block lengths differ");
    }
    for (out, sample) in output.iter_mut().zip(input) {
        if sample.is_finite() {
            *out += sample;
        }
    }
    Ok(())
}
pub fn limit(output: &mut [f32]) {
    for sample in output {
        *sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
    }
}

/// Bounded, single-thread PCM queue; allocation occurs only in the constructor.
/// Packets are accepted/dropped in whole frames, preserving channel alignment.
pub struct FrameQueue {
    samples: Vec<f32>,
    channels: usize,
    head: usize,
    length: usize,
}
impl FrameQueue {
    pub fn new(capacity_frames: usize, channels: usize) -> Result<Self, &'static str> {
        let capacity = capacity_frames
            .checked_mul(channels)
            .filter(|n| *n > 0)
            .ok_or("invalid queue capacity")?;
        Ok(Self {
            samples: vec![0.0; capacity],
            channels,
            head: 0,
            length: 0,
        })
    }
    pub fn available_frames(&self) -> usize {
        self.length / self.channels
    }
    pub(crate) fn peek_sample(&self, frame: usize, channel: usize) -> Option<f32> {
        if frame >= self.available_frames() || channel >= self.channels {
            return None;
        }
        Some(self.samples[(self.head + frame * self.channels + channel) % self.samples.len()])
    }
    pub(crate) fn discard_frames(&mut self, frames: usize) {
        let samples = frames.min(self.available_frames()) * self.channels;
        self.head = (self.head + samples) % self.samples.len();
        self.length -= samples;
    }
    pub(crate) fn clear(&mut self) {
        self.head = 0;
        self.length = 0;
    }
    pub fn push(&mut self, pcm: &[f32]) -> Result<usize, &'static str> {
        if !pcm.len().is_multiple_of(self.channels) {
            return Err("partial frame");
        }
        let accepted = pcm.len().min(self.samples.len() - self.length);
        for sample in &pcm[..accepted] {
            let tail = (self.head + self.length) % self.samples.len();
            self.samples[tail] = if sample.is_finite() { *sample } else { 0.0 };
            self.length += 1;
        }
        Ok(accepted / self.channels)
    }
    pub fn push_silence(&mut self, frames: usize) -> usize {
        let accepted = frames.min((self.samples.len() - self.length) / self.channels);
        for _ in 0..accepted * self.channels {
            let tail = (self.head + self.length) % self.samples.len();
            self.samples[tail] = 0.0;
            self.length += 1;
        }
        accepted
    }
    /// Fill missing frames with silence; returns the number of real queued frames.
    pub fn pop(&mut self, output: &mut [f32]) -> Result<usize, &'static str> {
        if !output.len().is_multiple_of(self.channels) {
            return Err("partial frame");
        }
        let consumed = output.len().min(self.length);
        for sample in &mut output[..consumed] {
            *sample = self.samples[self.head];
            self.head = (self.head + 1) % self.samples.len();
            self.length -= 1;
        }
        output[consumed..].fill(0.0);
        Ok(consumed / self.channels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_wrap_overflow_and_underflow_preserve_stereo_frames() {
        let mut q = FrameQueue::new(2, 2).unwrap();
        assert_eq!(q.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap(), 2);
        let mut frame = [0.0; 2];
        q.pop(&mut frame).unwrap();
        assert_eq!(frame, [1.0, 2.0]);
        q.push(&[5.0, 6.0]).unwrap();
        let mut out = [9.0; 6];
        assert_eq!(q.pop(&mut out).unwrap(), 2);
        assert_eq!(out, [3.0, 4.0, 5.0, 6.0, 0.0, 0.0]);
        assert!(q.push(&[1.0]).is_err());
        assert_eq!(q.available_frames(), 0);
    }
    #[test]
    fn silence_packet_preserves_existing_audio() {
        let mut q = FrameQueue::new(3, 1).unwrap();
        q.push(&[0.5]).unwrap();
        assert_eq!(q.push_silence(3), 2);
        let mut out = [1.0; 3];
        q.pop(&mut out).unwrap();
        assert_eq!(out, [0.5, 0.0, 0.0]);
    }
    #[test]
    fn ramp_is_frame_aligned_and_ends_at_zero() {
        let mut gain = GainRamp::new(1.0).unwrap();
        gain.set_target(0.0, 4).unwrap();
        let mut pcm = [1.0; 8];
        gain.apply(&mut pcm, 2).unwrap();
        assert_eq!(pcm, [0.75, 0.75, 0.5, 0.5, 0.25, 0.25, 0.0, 0.0]);
    }
    #[test]
    fn ramp_continues_across_packets() {
        let mut gain = GainRamp::new(0.0).unwrap();
        gain.set_target(1.0, 4).unwrap();
        let mut first = [1.0; 2];
        let mut next = [1.0; 2];
        gain.apply(&mut first, 1).unwrap();
        gain.apply(&mut next, 1).unwrap();
        assert_eq!(first, [0.25, 0.5]);
        assert_eq!(next, [0.75, 1.0]);
    }
    #[test]
    fn sum_before_clipping_preserves_cancellation() {
        let mut output = [0.0];
        mix_add(&mut output, &[0.8]).unwrap();
        mix_add(&mut output, &[0.8]).unwrap();
        mix_add(&mut output, &[-0.8]).unwrap();
        limit(&mut output);
        assert!((output[0] - 0.8).abs() < 0.0001);
    }
    #[test]
    fn invalid_gain_frame_layout_and_samples_are_safe() {
        assert!(GainRamp::new(f32::NAN).is_err());
        let mut gain = GainRamp::new(1.0).unwrap();
        assert!(gain.apply(&mut [1.0], 2).is_err());
        let mut pcm = [f32::NAN, 4.0];
        gain.apply(&mut pcm, 1).unwrap();
        limit(&mut pcm);
        assert_eq!(pcm, [0.0, 1.0]);
    }
}
