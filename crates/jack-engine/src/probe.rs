//! Deterministic stereo PCM proof signal and alignment-aware comparison.
//! Phase is modulo PERIOD; it is not an end-to-end latency measurement.
use serde::Serialize;
pub const PERIOD: usize = 2048;
pub const AMPLITUDE: f32 = 0.01;
pub fn sample(frame: usize, channel: usize) -> f32 {
    let mut value = (frame % PERIOD) as u32 ^ if channel == 0 { 0x31415926 } else { 0x27182818 };
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846ca68b);
    value ^= value >> 16;
    ((value & 0xffff) as f32 / 32768.0 - 1.0) * AMPLITUDE
}
#[derive(Debug, Serialize)]
pub struct ChannelProof {
    pub correlation: f64,
    pub gain: f64,
    pub relative_rms_error: f64,
}
#[derive(Debug, Serialize)]
pub struct SignalProof {
    pub pattern_phase_frames: usize,
    pub compared_frames: usize,
    pub channels: Vec<ChannelProof>,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContinuityProof {
    pub alignment_found: bool,
    pub checked_frames: u64,
    pub mismatched_frames: u64,
    pub max_absolute_error: f64,
    pub passed: bool,
}
/// Independent golden-pattern comparison across packet boundaries. Construction
/// allocates the reference; observe allocates nothing and never realigns after a
/// gap, so a dropped/duplicated frame cannot be hidden by a later phase fit.
pub struct ContinuityChecker {
    reference: Vec<[f32; 2]>,
    phase: Option<usize>,
    checked: u64,
    mismatched: u64,
    max_error: f64,
}
impl ContinuityChecker {
    pub fn new(gain: f32) -> Result<Self, &'static str> {
        if !gain.is_finite() || !(0.0..=1.0).contains(&gain) || gain == 0.0 {
            return Err("continuity reference requires a nonzero valid gain");
        }
        Ok(Self {
            reference: (0..PERIOD)
                .map(|f| std::array::from_fn(|c| sample(f, c) * gain))
                .collect(),
            phase: None,
            checked: 0,
            mismatched: 0,
            max_error: 0.0,
        })
    }
    pub fn observe(&mut self, stereo: &[f32]) -> Result<(), &'static str> {
        if !stereo.len().is_multiple_of(2) || stereo.iter().any(|v| !v.is_finite()) {
            return Err("invalid continuity PCM");
        }
        if stereo.is_empty() {
            return Ok(());
        }
        if self.phase.is_none() && stereo.len() >= 4 {
            self.phase = (0..PERIOD).find(|&p| {
                (0..2).all(|f| {
                    (0..2).all(|c| {
                        stereo[f * 2 + c].to_bits() == self.reference[(p + f) % PERIOD][c].to_bits()
                    })
                })
            });
        }
        for measured in stereo.chunks_exact(2) {
            self.checked += 1;
            if let Some(phase) = self.phase {
                let expected = self.reference[phase];
                if (0..2).any(|c| measured[c].to_bits() != expected[c].to_bits()) {
                    self.mismatched += 1;
                }
                for c in 0..2 {
                    self.max_error = self
                        .max_error
                        .max((f64::from(measured[c]) - f64::from(expected[c])).abs());
                }
                self.phase = Some((phase + 1) % PERIOD);
            } else {
                self.mismatched += 1;
            }
        }
        Ok(())
    }
    pub fn observe_silence(&mut self, frames: usize) {
        self.checked += frames as u64;
        self.mismatched += frames as u64;
        if let Some(phase) = self.phase {
            self.phase = Some((phase + frames) % PERIOD);
        }
    }
    pub fn report(&self) -> ContinuityProof {
        ContinuityProof {
            alignment_found: self.phase.is_some(),
            checked_frames: self.checked,
            mismatched_frames: self.mismatched,
            max_absolute_error: self.max_error,
            passed: self.phase.is_some()
                && self.checked >= (PERIOD * 2) as u64
                && self.mismatched == 0,
        }
    }
}
pub fn analyze(stereo: &[f32]) -> Result<SignalProof, &'static str> {
    let reference: Vec<_> = (0..PERIOD).map(|f| [sample(f, 0), sample(f, 1)]).collect();
    analyze_against(stereo, &reference)
}
/// Reference is independent of the mixer under test; exactly one stereo period.
pub fn analyze_against(
    stereo: &[f32],
    reference: &[[f32; 2]],
) -> Result<SignalProof, &'static str> {
    if !stereo.len().is_multiple_of(2) || stereo.len() < PERIOD * 4 {
        return Err("proof needs at least two periods of whole stereo frames");
    }
    if stereo.iter().any(|v| !v.is_finite()) {
        return Err("non-finite capture sample");
    }
    if reference.len() != PERIOD
        || reference.iter().flatten().any(|v| !v.is_finite())
        || (0..2).any(|c| {
            reference
                .iter()
                .map(|f| f64::from(f[c]).powi(2))
                .sum::<f64>()
                == 0.0
        })
    {
        return Err("invalid/silent stereo reference");
    }
    // Both channels participate in alignment, avoiding false success on a swap.
    let mut best = f64::NEG_INFINITY;
    let mut phase = 0;
    for offset in 0..PERIOD {
        let mut dot = 0.0;
        for (frame, measured) in stereo.chunks_exact(2).take(512).enumerate() {
            let expected = reference[(frame + offset) % PERIOD];
            dot += f64::from(expected[0]) * f64::from(measured[0])
                + f64::from(expected[1]) * f64::from(measured[1]);
        }
        if dot > best {
            best = dot;
            phase = offset;
        }
    }
    let mut channels = Vec::with_capacity(2);
    for channel in 0..2 {
        let (mut x2, mut y2, mut xy) = (0.0f64, 0.0f64, 0.0f64);
        for (frame, measured) in stereo.chunks_exact(2).enumerate() {
            let x = f64::from(reference[(frame + phase) % PERIOD][channel]);
            let y = f64::from(measured[channel]);
            x2 += x * x;
            y2 += y * y;
            xy += x * y;
        }
        let gain = xy / x2;
        let correlation = if y2 > 0.0 { xy / (x2 * y2).sqrt() } else { 0.0 };
        // Error against unit gain, so attenuation cannot be hidden by fitting it away.
        let relative_rms_error = ((y2 + x2 - 2.0 * xy).max(0.0) / x2).sqrt();
        channels.push(ChannelProof {
            correlation,
            gain,
            relative_rms_error,
        });
    }
    let passed = channels.iter().all(|c| {
        c.correlation >= 0.995 && (0.95..=1.05).contains(&c.gain) && c.relative_rms_error <= 0.1
    });
    Ok(SignalProof {
        pattern_phase_frames: phase,
        compared_frames: stereo.len() / 2,
        channels,
        passed,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(offset: usize) -> Vec<f32> {
        (0..PERIOD * 2)
            .flat_map(|f| [sample(f + offset, 0), sample(f + offset, 1)])
            .collect()
    }
    #[test]
    fn delayed_stereo_identity_is_detected() {
        let report = analyze(&fixture(733)).unwrap();
        assert!(report.passed);
        assert_eq!(report.pattern_phase_frames, 733);
        assert!(report.channels.iter().all(|c| c.relative_rms_error < 1e-6));
    }
    #[test]
    fn continuity_tracks_packets_and_rejects_a_gap_even_after_phase_changes() {
        let mut checker = ContinuityChecker::new(1.0).unwrap();
        let pcm = fixture(713);
        checker.observe(&pcm[..2000]).unwrap();
        checker.observe(&pcm[2000..]).unwrap();
        assert!(checker.report().passed);
        checker.observe_silence(1);
        checker.observe(&pcm).unwrap();
        assert!(!checker.report().passed);
        let mut dropped = ContinuityChecker::new(1.0).unwrap();
        dropped.observe(&pcm[..1024]).unwrap();
        dropped.observe(&pcm[1026..]).unwrap();
        assert!(dropped.report().mismatched_frames > 0);
        let mut duplicated = ContinuityChecker::new(1.0).unwrap();
        duplicated.observe(&pcm[..1024]).unwrap();
        duplicated.observe(&pcm[1022..]).unwrap();
        assert!(duplicated.report().mismatched_frames > 0);
    }
    #[test]
    fn continuity_rejects_corruption_wrong_gain_silence_and_nonfinite() {
        let mut checker = ContinuityChecker::new(0.35).unwrap();
        let mut pcm: Vec<_> = fixture(19).into_iter().map(|v| v * 0.35).collect();
        checker.observe(&pcm).unwrap();
        assert!(checker.report().passed);
        pcm[200] += 0.001;
        checker.observe(&pcm).unwrap();
        assert!(!checker.report().passed);
        let mut silent = ContinuityChecker::new(1.0).unwrap();
        silent.observe(&vec![0.0; PERIOD * 4]).unwrap();
        assert!(!silent.report().passed);
        assert!(silent.observe(&[f32::NAN, 0.0]).is_err());
        assert!(ContinuityChecker::new(0.0).is_err());
        let mut wrong_gain = ContinuityChecker::new(1.0).unwrap();
        wrong_gain.observe(&pcm).unwrap();
        assert!(!wrong_gain.report().passed);
    }
    #[test]
    fn independent_mixed_reference_detects_wrong_group_gain() {
        let reference: Vec<_> = (0..PERIOD)
            .map(|f| std::array::from_fn(|c| sample(f, c) * 0.65 + sample(f + 739, c) * 0.4))
            .collect();
        let pcm: Vec<_> = (0..PERIOD * 2)
            .flat_map(|f| reference[(f + 713) % PERIOD])
            .collect();
        assert!(analyze_against(&pcm, &reference).unwrap().passed);
        let wrong: Vec<_> = (0..PERIOD)
            .map(|f| std::array::from_fn(|c| sample(f, c) * 0.65 + sample(f + 739, c) * 0.2))
            .collect();
        assert!(!analyze_against(&pcm, &wrong).unwrap().passed);
        assert!(analyze_against(&pcm, &vec![[0.0; 2]; PERIOD]).is_err());
    }
    #[test]
    fn silence_tone_channel_swap_and_wrong_gain_fail() {
        let mut pcm = fixture(0);
        for frame in pcm.chunks_exact_mut(2) {
            frame.swap(0, 1);
        }
        assert!(!analyze(&pcm).unwrap().passed);
        assert!(!analyze(&vec![0.0; PERIOD * 4]).unwrap().passed);
        let tone: Vec<_> = (0..PERIOD * 2)
            .flat_map(|f| {
                let v = (f as f32 * std::f32::consts::TAU / 16.0).sin() * AMPLITUDE;
                [v, v]
            })
            .collect();
        assert!(!analyze(&tone).unwrap().passed);
        let quiet: Vec<_> = fixture(0).into_iter().map(|v| v * 0.2).collect();
        assert!(!analyze(&quiet).unwrap().passed);
    }
    #[test]
    fn dropped_frames_nonfinite_and_partial_frames_are_rejected() {
        let mut pcm = fixture(0);
        pcm.drain(1000..1100);
        pcm.extend([0.0; 100]);
        assert!(!analyze(&pcm).unwrap().passed);
        pcm[0] = f32::NAN;
        assert!(analyze(&pcm).is_err());
        assert!(analyze(&[0.0; 3]).is_err());
    }
}
