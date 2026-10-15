//! Bounded streaming linear interpolation. Channel order is retained.
//! This diagnostic converter has no anti-alias filter; production quality is pending.
use crate::FrameQueue;
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct ResampleStats {
    pub received_frames: u64,
    pub dropped_frames: u64,
    pub produced_frames: u64,
}
pub struct StreamingResampler {
    input: FrameQueue,
    channels: usize,
    nominal_step: f64,
    step: f64,
    phase: f64,
    stats: ResampleStats,
}
impl StreamingResampler {
    pub fn new(
        source_rate: u32,
        output_rate: u32,
        channels: usize,
        capacity_frames: usize,
    ) -> Result<Self, &'static str> {
        if !(8000..=384000).contains(&source_rate)
            || !(8000..=384000).contains(&output_rate)
            || !(1..=32).contains(&channels)
            || capacity_frames < 2
        {
            return Err("invalid resampler format/capacity");
        }
        let nominal_step = f64::from(source_rate) / f64::from(output_rate);
        Ok(Self {
            input: FrameQueue::new(capacity_frames, channels)?,
            channels,
            nominal_step,
            step: nominal_step,
            phase: 0.0,
            stats: ResampleStats::default(),
        })
    }
    pub fn queued_frames(&self) -> usize {
        self.input.available_frames()
    }
    pub fn stats(&self) -> ResampleStats {
        self.stats
    }
    pub fn push(&mut self, pcm: &[f32]) -> Result<usize, &'static str> {
        if !pcm.len().is_multiple_of(self.channels) {
            return Err("partial resampler input frame");
        }
        let requested = pcm.len() / self.channels;
        let accepted = self.input.push(pcm)?;
        self.account(requested, accepted);
        Ok(accepted)
    }
    pub fn push_silence(&mut self, frames: usize) -> usize {
        let accepted = self.input.push_silence(frames);
        self.account(frames, accepted);
        accepted
    }
    fn account(&mut self, requested: usize, accepted: usize) {
        self.stats.received_frames += requested as u64;
        self.stats.dropped_frames += (requested - accepted) as u64;
    }
    /// Positive ppm consumes input faster; phase remains continuous across changes.
    pub fn set_correction_ppm(&mut self, ppm: f64) -> Result<(), &'static str> {
        if !ppm.is_finite() || ppm.abs() > 1000.0 {
            return Err("clock correction exceeds diagnostic range");
        }
        self.step = self.nominal_step * (1.0 + ppm / 1_000_000.0);
        Ok(())
    }
    pub fn reset(&mut self) {
        self.input.clear();
        self.phase = 0.0;
        self.step = self.nominal_step;
    }
    /// Writes converted frames followed by silence. Underflow does not advance phase.
    /// Returns converted frame count; no allocation or packet-size dependence.
    pub fn process(&mut self, output: &mut [f32]) -> Result<usize, &'static str> {
        if !output.len().is_multiple_of(self.channels) {
            return Err("partial resampler output frame");
        }
        if output.is_empty() {
            return Ok(0);
        }
        let mut produced = 0;
        for frame in output.chunks_exact_mut(self.channels) {
            let skip = (self.phase.floor() as usize).min(self.input.available_frames());
            self.input.discard_frames(skip);
            self.phase -= skip as f64;
            if self.phase >= 1.0 || self.input.available_frames() == 0 {
                break;
            }
            let fraction = self.phase;
            if fraction > 0.0 && self.input.available_frames() < 2 {
                break;
            }
            for (channel, sample) in frame.iter_mut().enumerate() {
                let first = f64::from(self.input.peek_sample(0, channel).expect("available input"));
                let value = if fraction == 0.0 {
                    first
                } else {
                    let second =
                        f64::from(self.input.peek_sample(1, channel).expect("lookahead input"));
                    first + (second - first) * fraction
                };
                *sample = value as f32;
            }
            self.phase += self.step;
            produced += 1;
        }
        // Normalize even when the caller's block ends exactly on a source sample.
        let skip = (self.phase.floor() as usize).min(self.input.available_frames());
        self.input.discard_frames(skip);
        self.phase -= skip as f64;
        output[produced * self.channels..].fill(0.0);
        self.stats.produced_frames += produced as u64;
        Ok(produced)
    }
}
/// Optional queue-fill feedback for small independent device clock errors.
/// Not enabled by the bridge until real drift/hotplug tests establish operating bounds.
pub struct DriftController {
    target: usize,
    limit: f64,
    ppm: f64,
}
impl DriftController {
    pub fn new(target_frames: usize, max_ppm: f64) -> Result<Self, &'static str> {
        if target_frames == 0 || !max_ppm.is_finite() || !(0.0..=1000.0).contains(&max_ppm) {
            return Err("invalid drift controller");
        }
        Ok(Self {
            target: target_frames,
            limit: max_ppm,
            ppm: 0.0,
        })
    }
    pub fn update(
        &mut self,
        queued_frames: usize,
        elapsed_frames: usize,
        output_rate: u32,
    ) -> Result<f64, &'static str> {
        if !(8000..=384000).contains(&output_rate) {
            return Err("invalid output clock");
        }
        let error = (queued_frames as f64 - self.target as f64) / self.target as f64;
        let requested = (error * self.limit).clamp(-self.limit, self.limit);
        // Two-second smoothing; responds consistently to different output block sizes.
        let alpha = (elapsed_frames as f64 / (f64::from(output_rate) * 2.0)).clamp(0.0, 1.0);
        self.ppm += (requested - self.ppm) * alpha;
        Ok(self.ppm)
    }
    pub fn reset(&mut self) {
        self.ppm = 0.0;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Vec<f32> {
        (0..64)
            .flat_map(|f| [f as f32 / 64.0, -(f as f32) / 64.0])
            .collect()
    }
    #[test]
    fn equal_rate_is_exact_with_single_frame_packets() {
        let mut r = StreamingResampler::new(48000, 48000, 2, 4).unwrap();
        for frame in input().chunks_exact(2) {
            r.push(frame).unwrap();
            let mut out = [0.0; 2];
            assert_eq!(r.process(&mut out).unwrap(), 1);
            assert_eq!(&out, frame);
        }
    }
    #[test]
    fn conversion_is_independent_of_packet_and_output_block_sizes() {
        let source = input();
        let mut full = StreamingResampler::new(44100, 48000, 2, 128).unwrap();
        full.push(&source).unwrap();
        let mut reference = [0.0; 160];
        let n = full.process(&mut reference).unwrap();
        let mut split = StreamingResampler::new(44100, 48000, 2, 128).unwrap();
        let mut result = vec![];
        for packet in source.chunks(6) {
            split.push(packet).unwrap();
            loop {
                let mut block = [0.0; 10];
                let produced = split.process(&mut block).unwrap();
                result.extend_from_slice(&block[..produced * 2]);
                if produced < 5 {
                    break;
                }
            }
        }
        assert_eq!(result, &reference[..n * 2]);
        assert!(result.chunks_exact(2).all(|f| f[0] == -f[1]));
    }
    #[test]
    fn ratio_and_linear_interpolation_match_known_ramp() {
        let mut up = StreamingResampler::new(24000, 48000, 1, 8).unwrap();
        up.push(&[0.0, 0.25, 0.5, 0.75]).unwrap();
        let mut out = [0.0; 8];
        assert_eq!(up.process(&mut out).unwrap(), 7);
        assert_eq!(out, [0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.0]);
        let mut down = StreamingResampler::new(48000, 24000, 1, 8).unwrap();
        down.push(&[0.0, 0.25, 0.5, 0.75]).unwrap();
        assert_eq!(down.process(&mut out).unwrap(), 2);
        assert_eq!(&out[..2], &[0.0, 0.5]);
    }
    #[test]
    fn underflow_holds_lookahead_and_reset_discards_old_stream() {
        let mut r = StreamingResampler::new(24000, 48000, 1, 4).unwrap();
        r.push(&[0.25]).unwrap();
        let mut out = [0.0; 2];
        assert_eq!(r.process(&mut out).unwrap(), 1);
        r.push(&[0.75]).unwrap();
        assert_eq!(r.process(&mut out).unwrap(), 2);
        assert_eq!(out, [0.5, 0.75]);
        r.reset();
        r.push(&[-0.25]).unwrap();
        assert_eq!(r.process(&mut out).unwrap(), 1);
        assert_eq!(out, [-0.25, 0.0]);
    }
    #[test]
    fn bounded_overflow_invalid_frames_and_correction_do_not_corrupt_state() {
        let mut r = StreamingResampler::new(48000, 48000, 2, 2).unwrap();
        assert!(r.push(&[0.0]).is_err());
        assert_eq!(r.push(&[0.1; 6]).unwrap(), 2);
        assert_eq!(r.stats().dropped_frames, 1);
        assert!(r.process(&mut [0.0; 3]).is_err());
        assert!(r.set_correction_ppm(f64::NAN).is_err());
        assert!(r.set_correction_ppm(1001.0).is_err());
        let mut out = [0.0; 4];
        assert_eq!(r.process(&mut out).unwrap(), 2);
        assert_eq!(out, [0.1; 4]);
    }
    #[test]
    fn drift_feedback_is_bounded_and_corrects_in_the_right_direction() {
        let mut d = DriftController::new(4800, 300.0).unwrap();
        for _ in 0..1000 {
            assert!((0.0..=300.0).contains(&d.update(9600, 480, 48000).unwrap()));
        }
        assert!(d.ppm > 290.0);
        d.reset();
        assert!(d.update(0, 480, 48000).unwrap() < 0.0);
        let before = d.ppm;
        assert_eq!(d.update(4800, 0, 48000).unwrap(), before);
    }
}
