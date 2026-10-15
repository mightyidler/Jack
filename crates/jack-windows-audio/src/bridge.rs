//! Lab-only transport: shared WASAPI loopback -> a distinct render endpoint.
//! Float32, matching channel layouts. Linear rate conversion is diagnostic quality.
use crate::platform::Apartment;
use jack_engine::{
    mixer::{DeviceMixer, PcmFormat},
    resampler::StreamingResampler,
};
use serde::Serialize;
use std::{
    error::Error,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Media::{Audio::*, Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT},
        System::Com::{CLSCTX_ALL, CoCreateGuid, CoCreateInstance, CoTaskMemFree},
    },
    core::{HSTRING, Interface},
};

type LabResult<T> = Result<T, Box<dyn Error>>;
pub(super) struct Format(pub(super) *mut WAVEFORMATEX);
impl Drop for Format {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
        }
    }
}
impl Format {
    pub(super) fn details(&self) -> LabResult<(u32, usize, u32)> {
        unsafe {
            let base = self.0.read_unaligned();
            let channel_count = base.nChannels;
            let sample_rate = base.nSamplesPerSec;
            if !(1..=32).contains(&channel_count)
                || !(8000..=384000).contains(&sample_rate)
                || base.wBitsPerSample != 32
                || base.nBlockAlign != base.nChannels * 4
            {
                return Err("lab bridge requires valid float32 PCM".into());
            }
            let mask = match base.wFormatTag {
                3 => 0,
                0xfffe if base.cbSize >= 22 => {
                    let extended = self.0.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned();
                    let sub_format = extended.SubFormat;
                    if sub_format != KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                        return Err("non-float extensible PCM is unsupported".into());
                    }
                    extended.dwChannelMask
                }
                _ => return Err("lab bridge supports only IEEE float32".into()),
            };
            Ok((base.nSamplesPerSec, usize::from(base.nChannels), mask))
        }
    }
}
struct Running(IAudioClient);
impl Drop for Running {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

#[derive(Debug, Serialize, Default)]
pub struct BridgeStats {
    pub source_sample_rate: u32,
    pub output_sample_rate: u32,
    pub linear_resampling: bool,
    pub group_gain: f32,
    pub captured_frames: u64,
    pub rendered_frames: u64,
    pub dropped_frames: u64,
    pub underrun_frames: u64,
    pub discontinuities: u64,
    pub source_peak: f32,
    pub clipped_samples: u64,
    pub elapsed_ms: u128,
}

/// Caller must explicitly choose lab endpoints. Never changes system defaults/policy.
/// Actual PCM fidelity must be measured in a lab; nonzero counters prove no such thing.
pub fn bridge_lab(
    source_id: &str,
    output_id: &str,
    seconds: u64,
    gain: f32,
) -> LabResult<BridgeStats> {
    bridge_lab_ready(source_id, output_id, seconds, gain, None)
}
pub(super) fn bridge_lab_ready(
    source_id: &str,
    output_id: &str,
    seconds: u64,
    gain: f32,
    ready: Option<std::sync::mpsc::Sender<()>>,
) -> LabResult<BridgeStats> {
    if source_id.is_empty() || output_id.is_empty() || source_id.eq_ignore_ascii_case(output_id) {
        return Err("choose two distinct explicit render endpoints".into());
    }
    if !(1..=30).contains(&seconds) {
        return Err("lab bridge is limited to 1..=30 seconds".into());
    }
    if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
        return Err("gain must be finite and within 0..=1".into());
    }
    let _apartment = Apartment::new()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let source = enumerator.GetDevice(&HSTRING::from(source_id))?;
        let output = enumerator.GetDevice(&HSTRING::from(output_id))?;
        for device in [&source, &output] {
            if device.cast::<IMMEndpoint>()?.GetDataFlow()? != eRender {
                return Err("both endpoints must be render devices".into());
            }
        }
        let capture_client: IAudioClient = source.Activate(CLSCTX_ALL, None)?;
        let render_client: IAudioClient = output.Activate(CLSCTX_ALL, None)?;
        let capture_format = Format(capture_client.GetMixFormat()?);
        let output_format = Format(render_client.GetMixFormat()?);
        let details = capture_format.details()?;
        let output_details = output_format.details()?;
        if (details.1, details.2) != (output_details.1, output_details.2) {
            return Err("channel/mask mismatch: channel mapping is not implemented".into());
        }
        let (rate, channels, _) = details;
        let (output_rate, _, output_mask) = output_details;
        // 100 ms client buffers; polling is deliberately a diagnostic, not a low-latency claim.
        capture_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            1_000_000,
            0,
            capture_format.0,
            None,
        )?;
        let session = CoCreateGuid()?;
        render_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_NOPERSIST,
            1_000_000,
            0,
            output_format.0,
            Some(&session),
        )?;
        let own_volume: ISimpleAudioVolume = render_client.GetService()?;
        own_volume.SetMasterVolume(1.0, &session)?;
        let capture: IAudioCaptureClient = capture_client.GetService()?;
        let render: IAudioRenderClient = render_client.GetService()?;
        let buffer_frames = render_client.GetBufferSize()?;
        let mut output_pcm = vec![0.0f32; buffer_frames as usize * channels];
        let mut converted_pcm = vec![0.0f32; buffer_frames as usize * channels];
        let mut resampler =
            StreamingResampler::new(rate, output_rate, channels, rate as usize / 2)?;
        let mut mixer = DeviceMixer::new(
            PcmFormat {
                sample_rate: output_rate,
                channels,
                channel_mask: output_mask,
            },
            buffer_frames as usize,
            (output_rate as usize / 2).max(buffer_frames as usize),
        )?;
        let lab_group = jack_core::GAMING;
        mixer.add_group(lab_group, gain)?;
        // Prime render silence before Start. All allocations above precede the loop.
        render.GetBuffer(buffer_frames)?;
        render.ReleaseBuffer(buffer_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
        render_client.Start()?;
        let _render_running = Running(render_client.clone());
        capture_client.Start()?;
        let _capture_running = Running(capture_client.clone());
        if let Some(sender) = ready {
            let _ = sender.send(());
        }
        let start = Instant::now();
        let mut stats = BridgeStats {
            source_sample_rate: rate,
            output_sample_rate: output_rate,
            linear_resampling: rate != output_rate,
            group_gain: gain,
            ..BridgeStats::default()
        };
        while start.elapsed() < Duration::from_secs(seconds) {
            // Bound draining so a busy source cannot starve output.
            for _ in 0..64 {
                if capture.GetNextPacketSize()? == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                if frames > 0 && data.is_null() && flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0
                {
                    capture.ReleaseBuffer(frames)?;
                    return Err("capture returned null PCM without the SILENT flag".into());
                }
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                    stats.discontinuities += 1;
                }
                let accepted = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null()
                {
                    resampler.push_silence(frames as usize)
                } else {
                    // WASAPI supplies a frame-aligned float32 packet with this format.
                    let pcm =
                        std::slice::from_raw_parts(data.cast::<f32>(), frames as usize * channels);
                    for sample in pcm {
                        if sample.is_finite() {
                            stats.source_peak = stats.source_peak.max(sample.abs());
                        }
                    }
                    resampler
                        .push(pcm)
                        .expect("WASAPI packet contains whole frames")
                };
                capture.ReleaseBuffer(frames)?;
                stats.captured_frames += u64::from(frames);
                stats.dropped_frames += frames as u64 - accepted as u64;
            }
            let available = buffer_frames.saturating_sub(render_client.GetCurrentPadding()?);
            if available > 0 {
                let pcm = &mut output_pcm[..available as usize * channels];
                let converted = &mut converted_pcm[..pcm.len()];
                let real = resampler.process(converted).expect("whole render frames");
                let pushed = mixer
                    .push(lab_group, &converted[..real * channels])
                    .expect("whole converted frames");
                stats.dropped_frames += pushed.dropped_frames as u64;
                let report = mixer.render(pcm).expect("bounded render frames");
                stats.clipped_samples += report.clipped_samples as u64;
                let target = render.GetBuffer(available)?;
                std::ptr::copy_nonoverlapping(
                    pcm.as_ptr().cast::<u8>(),
                    target,
                    std::mem::size_of_val(pcm),
                );
                render.ReleaseBuffer(available, 0)?;
                stats.rendered_frames += u64::from(available);
                stats.underrun_frames += available as u64 - real as u64;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        stats.elapsed_ms = start.elapsed().as_millis();
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_lab_requests_fail_before_opening_audio_devices() {
        assert!(bridge_lab("", "output", 1, 0.2).is_err());
        assert!(bridge_lab("endpoint", "ENDPOINT", 1, 0.2).is_err());
        assert!(bridge_lab("source", "output", 31, 0.2).is_err());
        assert!(bridge_lab("source", "output", 1, f32::NAN).is_err());
        assert!(bridge_lab("source", "output", 1, 1.1).is_err());
    }
}
