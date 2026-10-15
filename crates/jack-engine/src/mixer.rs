//! One physical output mixer. Construction/reconfiguration belongs to control code.
//! push/render/set_gain allocate no memory; all slots share the output PCM format.
use crate::{FrameQueue, GainRamp};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: usize,
    pub channel_mask: u32,
}
impl PcmFormat {
    pub fn validate(self) -> Result<(), &'static str> {
        if !(8000..=384000).contains(&self.sample_rate) || !(1..=32).contains(&self.channels) {
            return Err("unsupported PCM rate/channel count");
        }
        if self.channel_mask != 0 && self.channel_mask.count_ones() as usize != self.channels {
            return Err("channel mask does not match channel count");
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct GroupStats {
    pub received_frames: u64,
    pub dropped_frames: u64,
    pub rendered_frames: u64,
    pub underrun_frames: u64,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PushReport {
    pub accepted_frames: usize,
    pub dropped_frames: usize,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RenderReport {
    pub frames: usize,
    pub groups: usize,
    pub clipped_samples: usize,
}
struct Group {
    id: Uuid,
    queue: FrameQueue,
    gain: GainRamp,
    stats: GroupStats,
}
pub struct DeviceMixer {
    format: PcmFormat,
    max_block_frames: usize,
    queue_frames: usize,
    groups: Vec<Group>,
    scratch: Vec<f32>,
    sum: Vec<f64>,
}
impl DeviceMixer {
    pub fn new(
        format: PcmFormat,
        max_block_frames: usize,
        queue_frames: usize,
    ) -> Result<Self, &'static str> {
        format.validate()?;
        let samples = max_block_frames
            .checked_mul(format.channels)
            .filter(|n| *n > 0)
            .ok_or("invalid render capacity")?;
        queue_frames
            .checked_mul(format.channels)
            .filter(|n| *n > 0)
            .ok_or("invalid queue capacity")?;
        Ok(Self {
            format,
            max_block_frames,
            queue_frames,
            groups: vec![],
            scratch: vec![0.0; samples],
            sum: vec![0.0; samples],
        })
    }
    pub fn format(&self) -> PcmFormat {
        self.format
    }
    /// Control-side allocation. Do not call while servicing a WASAPI buffer.
    pub fn add_group(&mut self, id: Uuid, gain: f32) -> Result<(), &'static str> {
        if id.is_nil() || self.groups.iter().any(|g| g.id == id) {
            return Err("invalid/duplicate group id");
        }
        let gain = GainRamp::new(gain)?;
        let queue = FrameQueue::new(self.queue_frames, self.format.channels)?;
        self.groups.push(Group {
            id,
            gain,
            queue,
            stats: GroupStats::default(),
        });
        Ok(())
    }
    /// Control-side removal drops the group's queued audio; other slots survive.
    pub fn remove_group(&mut self, id: Uuid) -> Option<GroupStats> {
        self.groups
            .iter()
            .position(|g| g.id == id)
            .map(|i| self.groups.remove(i).stats)
    }
    fn group(&mut self, id: Uuid) -> Result<&mut Group, &'static str> {
        self.groups
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or("unknown audio group")
    }
    pub fn stats(&self, id: Uuid) -> Option<GroupStats> {
        self.groups.iter().find(|g| g.id == id).map(|g| g.stats)
    }
    /// Apply gain at consumption time, including samples already queued.
    pub fn set_gain(
        &mut self,
        id: Uuid,
        gain: f32,
        ramp_frames: usize,
    ) -> Result<(), &'static str> {
        self.group(id)?.gain.set_target(gain, ramp_frames)
    }
    pub fn push(&mut self, id: Uuid, pcm: &[f32]) -> Result<PushReport, &'static str> {
        let channels = self.format.channels;
        if !pcm.len().is_multiple_of(channels) {
            return Err("partial PCM frame");
        }
        let group = self.group(id)?;
        let requested = pcm.len() / channels;
        let accepted = group.queue.push(pcm)?;
        Ok(account_push(group, requested, accepted))
    }
    pub fn push_silence(&mut self, id: Uuid, frames: usize) -> Result<PushReport, &'static str> {
        let group = self.group(id)?;
        let accepted = group.queue.push_silence(frames);
        Ok(account_push(group, frames, accepted))
    }
    /// Reject invalid blocks BEFORE any queue/ramp is consumed. No allocation/I/O.
    pub fn render(&mut self, output: &mut [f32]) -> Result<RenderReport, &'static str> {
        if !output.len().is_multiple_of(self.format.channels)
            || output.len() / self.format.channels > self.max_block_frames
        {
            return Err("invalid/oversized render block");
        }
        let frames = output.len() / self.format.channels;
        let scratch = &mut self.scratch[..output.len()];
        let sum = &mut self.sum[..output.len()];
        sum.fill(0.0);
        for group in &mut self.groups {
            let real = group.queue.pop(scratch)?;
            group.gain.apply(scratch, self.format.channels)?;
            group.stats.rendered_frames += frames as u64;
            group.stats.underrun_frames += (frames - real) as u64;
            // Sum before clipping. f64 also preserves cancellation of large finite f32s.
            for (total, sample) in sum.iter_mut().zip(scratch.iter()) {
                *total += f64::from(*sample);
            }
        }
        let mut clipped_samples = 0;
        for (out, total) in output.iter_mut().zip(sum) {
            if *total > 1.0 || *total < -1.0 {
                clipped_samples += 1;
            }
            *out = total.clamp(-1.0, 1.0) as f32;
        }
        Ok(RenderReport {
            frames,
            groups: self.groups.len(),
            clipped_samples,
        })
    }
}
fn account_push(group: &mut Group, requested: usize, accepted: usize) -> PushReport {
    let dropped = requested - accepted;
    group.stats.received_frames += requested as u64;
    group.stats.dropped_frames += dropped as u64;
    PushReport {
        accepted_frames: accepted,
        dropped_frames: dropped,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const A: Uuid = Uuid::from_u128(1);
    const B: Uuid = Uuid::from_u128(2);
    const C: Uuid = Uuid::from_u128(3);
    fn mixer() -> DeviceMixer {
        DeviceMixer::new(
            PcmFormat {
                sample_rate: 48000,
                channels: 2,
                channel_mask: 3,
            },
            8,
            8,
        )
        .unwrap()
    }
    #[test]
    fn independent_gains_mix_and_missing_group_audio_is_silence() {
        let mut m = mixer();
        m.add_group(A, 0.5).unwrap();
        m.add_group(B, 0.25).unwrap();
        m.push(A, &[0.4, -0.4, 0.2, -0.2]).unwrap();
        m.push(B, &[0.8, 0.8]).unwrap();
        let mut out = [0.0; 4];
        m.render(&mut out).unwrap();
        assert_eq!(out, [0.4, 0.0, 0.1, -0.1]);
        assert_eq!(m.stats(B).unwrap().underrun_frames, 1);
    }
    #[test]
    fn volume_changes_affect_queued_pcm_and_ramps_cross_blocks() {
        let mut m = mixer();
        m.add_group(A, 1.0).unwrap();
        m.add_group(B, 1.0).unwrap();
        m.push(A, &[0.4; 8]).unwrap();
        m.push(B, &[0.1; 8]).unwrap();
        m.set_gain(A, 0.0, 4).unwrap();
        let mut out = [0.0; 4];
        m.render(&mut out).unwrap();
        assert!((out[0] - 0.4).abs() < 1e-6 && (out[2] - 0.3).abs() < 1e-6);
        m.render(&mut out).unwrap();
        assert!((out[0] - 0.2).abs() < 1e-6);
        assert_eq!(out[2], 0.1);
    }
    #[test]
    fn sum_clamps_once_and_handles_large_finite_cancellation() {
        let mut m = mixer();
        for id in [A, B, C] {
            m.add_group(id, 1.0).unwrap();
        }
        for (id, value) in [(A, 0.8), (B, 0.8), (C, -0.8)] {
            m.push(id, &[value; 2]).unwrap();
        }
        let mut out = [0.0; 2];
        assert_eq!(m.render(&mut out).unwrap().clipped_samples, 0);
        assert_eq!(out, [0.8; 2]);
        m.push(A, &[f32::MAX; 2]).unwrap();
        m.push(B, &[-f32::MAX; 2]).unwrap();
        m.push(C, &[0.25; 2]).unwrap();
        m.render(&mut out).unwrap();
        assert_eq!(out, [0.25; 2]);
        m.push(A, &[0.8; 2]).unwrap();
        m.push(B, &[0.8; 2]).unwrap();
        assert_eq!(m.render(&mut out).unwrap().clipped_samples, 2);
        assert_eq!(out, [1.0; 2]);
    }
    #[test]
    fn failed_render_does_not_consume_audio_and_overflow_counts_frames() {
        let mut m = mixer();
        m.add_group(A, 1.0).unwrap();
        let push = m.push(A, &[0.25; 20]).unwrap();
        assert_eq!(push.accepted_frames, 8);
        assert_eq!(push.dropped_frames, 2);
        assert!(m.render(&mut [0.0; 3]).is_err());
        assert!(m.render(&mut [0.0; 18]).is_err());
        let mut out = [0.0; 16];
        m.render(&mut out).unwrap();
        assert_eq!(out, [0.25; 16]);
        assert_eq!(m.stats(A).unwrap().rendered_frames, 8);
    }
    #[test]
    fn device_and_group_isolation_survive_removal_and_recreation() {
        let mut left = mixer();
        let mut right = mixer();
        left.add_group(A, 1.0).unwrap();
        left.add_group(B, 1.0).unwrap();
        right.add_group(C, 1.0).unwrap();
        left.push(A, &[0.5; 2]).unwrap();
        left.push(B, &[0.25; 2]).unwrap();
        right.push(C, &[-0.5; 2]).unwrap();
        left.remove_group(A).unwrap();
        left.add_group(A, 1.0).unwrap();
        let mut out = [0.0; 2];
        left.render(&mut out).unwrap();
        assert_eq!(out, [0.25; 2]);
        right.render(&mut out).unwrap();
        assert_eq!(out, [-0.5; 2]);
        assert!(left.push(C, &[0.0; 2]).is_err());
    }
    #[test]
    fn invalid_config_and_gain_preserve_valid_slot() {
        let mut m = mixer();
        assert!(m.add_group(Uuid::nil(), 1.0).is_err());
        m.add_group(A, 0.5).unwrap();
        assert!(m.add_group(A, 1.0).is_err());
        assert!(m.set_gain(A, f32::NAN, 3).is_err());
        assert!(m.push(A, &[0.0]).is_err());
        m.push(A, &[0.4; 2]).unwrap();
        let mut out = [0.0; 2];
        m.render(&mut out).unwrap();
        assert_eq!(out, [0.2; 2]);
        assert!(
            DeviceMixer::new(
                PcmFormat {
                    channels: 2,
                    channel_mask: 1,
                    sample_rate: 48000
                },
                8,
                8
            )
            .is_err()
        );
    }
}
