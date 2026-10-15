//! Audible, low-amplitude own-session signal -> selected render loopback.
//! Bounded memory only; no microphone, PCM file, default-policy or endpoint-volume writes.
use crate::{bridge::Format, platform::Apartment};
use jack_engine::{
    FrameQueue,
    mixer::{DeviceMixer, PcmFormat},
    probe::{self, SignalProof},
};
use serde::Serialize;
use std::{
    error::Error,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Media::Audio::*,
        System::Com::{CLSCTX_ALL, CoCreateGuid, CoCreateInstance},
    },
    core::{HSTRING, Interface},
};
struct Running(IAudioClient);
impl Drop for Running {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}
#[derive(Debug, Serialize)]
pub struct PcmProof {
    pub endpoint_id: String,
    pub capture_endpoint_id: String,
    pub sample_rate: u32,
    pub rendered_frames: u64,
    pub captured_frames: usize,
    pub captured_total_frames: u64,
    pub observation_ms: u128,
    pub continuity: Option<probe::ContinuityProof>,
    pub startup_discontinuities: u64,
    pub discontinuities: u64,
    pub timestamp_errors: u64,
    pub signal: SignalProof,
    pub expected_gain: f32,
    pub compared_peak: f32,
    pub signal_after_group_mute: Option<SignalProof>,
    pub default_roles_unchanged: bool,
    pub endpoint_volumes_unchanged: bool,
    pub virtual_endpoint_proven: bool,
    pub passed: bool,
}
type ControlState = (String, Vec<&'static str>, Option<(u32, bool)>);
fn controls(snapshot: &crate::Snapshot) -> Vec<ControlState> {
    let mut state: Vec<_> = snapshot
        .endpoints
        .iter()
        .map(|e| {
            (
                e.id.clone(),
                e.defaults.clone(),
                e.volume.as_ref().map(|v| (v.level.to_bits(), v.muted)),
            )
        })
        .collect();
    state.sort_by(|a, b| a.0.cmp(&b.0));
    state
}
/// Output can be audible. Caller chooses an explicit active render endpoint.
/// A physical endpoint result verifies the harness only, never virtual-driver fidelity.
pub fn verify_pcm(endpoint: &str) -> Result<PcmProof, Box<dyn Error>> {
    verify_to(endpoint, endpoint, false)
}
/// Two synthetic group PCM feeds exercise mixing and independent gain on real WASAPI.
/// These feeds do not represent two routed application/virtual endpoint streams.
pub fn verify_mixer(endpoint: &str) -> Result<PcmProof, Box<dyn Error>> {
    verify_to(endpoint, endpoint, true)
}
fn mixed_reference(gaming_gain: f32) -> Vec<[f32; 2]> {
    (0..probe::PERIOD)
        .map(|f| {
            std::array::from_fn(|c| {
                probe::sample(f, c) * gaming_gain + probe::sample(f + 739, c) * 0.4
            })
        })
        .collect()
}
fn verify_to(
    endpoint: &str,
    capture_endpoint: &str,
    mixed: bool,
) -> Result<PcmProof, Box<dyn Error>> {
    verify_to_with_gain(endpoint, capture_endpoint, mixed, 1.0)
}
fn verify_to_with_gain(
    endpoint: &str,
    capture_endpoint: &str,
    mixed: bool,
    expected_gain: f32,
) -> Result<PcmProof, Box<dyn Error>> {
    verify_to_duration(endpoint, capture_endpoint, mixed, expected_gain, 1200)
}
fn verify_to_duration(
    endpoint: &str,
    capture_endpoint: &str,
    mixed: bool,
    expected_gain: f32,
    milliseconds: u64,
) -> Result<PcmProof, Box<dyn Error>> {
    if !(1200..=300000).contains(&milliseconds) || (mixed && milliseconds != 1200) {
        return Err("invalid PCM observation duration".into());
    }
    if endpoint.is_empty() || capture_endpoint.is_empty() {
        return Err("choose an explicit render endpoint".into());
    }
    let before = crate::snapshot()?;
    let _apartment = Apartment::new()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDevice(&HSTRING::from(endpoint))?;
        let capture_device = enumerator.GetDevice(&HSTRING::from(capture_endpoint))?;
        for device in [&device, &capture_device] {
            if device.GetState()? != DEVICE_STATE_ACTIVE
                || device.cast::<IMMEndpoint>()?.GetDataFlow()? != eRender
            {
                return Err("choose active render endpoints".into());
            }
        }
        let render_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let capture_client: IAudioClient = capture_device.Activate(CLSCTX_ALL, None)?;
        let format = Format(render_client.GetMixFormat()?);
        let (rate, channels, mask) = format.details()?;
        let capture_format = Format(capture_client.GetMixFormat()?);
        if format.details()? != capture_format.details()? {
            return Err("PCM proof requires matching rate/channel/mask".into());
        }
        if channels != 2 {
            return Err("PCM proof requires stereo float32".into());
        }
        let guid = CoCreateGuid()?;
        render_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_NOPERSIST,
            1_000_000,
            0,
            format.0,
            Some(&guid),
        )?;
        capture_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            1_000_000,
            0,
            format.0,
            None,
        )?;
        let control: IAudioSessionControl = render_client.GetService()?;
        control.SetDisplayName(&HSTRING::from("Jack PCM proof (low amplitude)"), &guid)?;
        let own_volume: ISimpleAudioVolume = render_client.GetService()?;
        own_volume.SetMasterVolume(1.0, &guid)?;
        let render: IAudioRenderClient = render_client.GetService()?;
        let capture: IAudioCaptureClient = capture_client.GetService()?;
        let buffer_frames = render_client.GetBufferSize()?;
        let capacity = rate as usize * 2 * channels;
        // Keep only the most recent two seconds, regardless of observation length.
        // Discarding old observations does not drop transport packets or save PCM.
        let mut capture_queue = FrameQueue::new(rate as usize * 2, channels)?;
        let mut discard_pcm = vec![0.0f32; capacity];
        let mut captured_total_frames = 0;
        let mut continuity = if milliseconds > 2000 && !mixed && expected_gain > 0.0 {
            Some(probe::ContinuityChecker::new(expected_gain)?)
        } else {
            None
        };
        let mut rendered = 0u64;
        let mut mixer = DeviceMixer::new(
            PcmFormat {
                sample_rate: rate,
                channels,
                channel_mask: mask,
            },
            buffer_frames as usize,
            buffer_frames as usize,
        )?;
        mixer.add_group(jack_core::GAMING, 0.65)?;
        mixer.add_group(jack_core::CHAT, 0.4)?;
        let mut gaming_pcm = vec![0.0f32; buffer_frames as usize * 2];
        let mut chat_pcm = vec![0.0f32; buffer_frames as usize * 2];
        let mute_at = rate as usize / 2;
        let signal_end = if milliseconds == 1200 {
            rate as usize * 9 / 10
        } else {
            rate as usize * milliseconds as usize / 1000 - rate as usize / 10
        };
        let mut muted = false;
        let mut fill = |frames: u32, rendered: &mut u64| -> windows::core::Result<()> {
            let target = render.GetBuffer(frames)?;
            let pcm = std::slice::from_raw_parts_mut(target.cast::<f32>(), frames as usize * 2);
            for (offset, frame) in pcm.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let index = *rendered as usize + offset;
                // Preserve the original short fixture; long observations generate
                // PCM until100ms before the requested end, rather than only800ms.
                if index >= rate as usize / 10 && index < signal_end {
                    frame[0] = probe::sample(index, 0);
                    frame[1] = probe::sample(index, 1);
                } else {
                    frame.fill(0.0);
                }
                if mixed {
                    gaming_pcm[offset * 2..offset * 2 + 2].copy_from_slice(frame);
                    for c in 0..2 {
                        chat_pcm[offset * 2 + c] =
                            if index >= rate as usize / 10 && index < signal_end {
                                probe::sample(index + 739, c)
                            } else {
                                0.0
                            };
                    }
                }
            }
            if mixed {
                let mut offset = 0;
                while offset < frames as usize {
                    let index = *rendered as usize + offset;
                    if index >= mute_at && !muted {
                        mixer
                            .set_gain(jack_core::GAMING, 0.0, rate as usize / 200)
                            .expect("valid group/ramp");
                        muted = true;
                    }
                    let count = if index < mute_at {
                        (frames as usize - offset).min(mute_at - index)
                    } else {
                        frames as usize - offset
                    };
                    let range = offset * 2..(offset + count) * 2;
                    mixer
                        .push(jack_core::GAMING, &gaming_pcm[range.clone()])
                        .expect("bounded stereo group PCM");
                    mixer
                        .push(jack_core::CHAT, &chat_pcm[range.clone()])
                        .expect("bounded stereo group PCM");
                    mixer.render(&mut pcm[range]).expect("bounded output block");
                    offset += count;
                }
            }
            *rendered += u64::from(frames);
            render.ReleaseBuffer(frames, 0)
        };
        // The independent test generator must not underrun merely because it runs
        // at ordinary thread priority while the transport has MMCSS scheduling.
        let fixture_mmcss = crate::transport::Mmcss::new()?;
        fill(buffer_frames, &mut rendered)?;
        capture_client.Start()?;
        let capture_guard = Running(capture_client.clone());
        render_client.Start()?;
        let render_guard = Running(render_client.clone());
        let start = Instant::now();
        let (mut startup_discontinuities, mut discontinuities, mut timestamp_errors) = (0, 0, 0);
        let mut packets = 0;
        while start.elapsed() < Duration::from_millis(milliseconds) {
            for _ in 0..64 {
                if capture.GetNextPacketSize()? == 0 {
                    break;
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let samples = frames as usize * 2;
                if samples > capacity {
                    capture.ReleaseBuffer(frames)?;
                    return Err("capture exceeded bounded proof storage".into());
                }
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                    if packets == 0 {
                        startup_discontinuities += 1;
                    } else {
                        discontinuities += 1;
                    }
                }
                if flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0 {
                    timestamp_errors += 1;
                }
                let discard = (capture_queue.available_frames() + frames as usize)
                    .saturating_sub(rate as usize * 2);
                if discard > 0 {
                    capture_queue.pop(&mut discard_pcm[..discard * 2])?;
                }
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    capture_queue.push_silence(frames as usize);
                    if start.elapsed() >= Duration::from_secs(1)
                        && start.elapsed() < Duration::from_millis(milliseconds - 1000)
                        && let Some(checker) = &mut continuity
                    {
                        checker.observe_silence(frames as usize);
                    }
                } else if data.is_null() && frames > 0 {
                    capture.ReleaseBuffer(frames)?;
                    return Err("null PCM without SILENT flag".into());
                } else if frames > 0 {
                    let pcm = std::slice::from_raw_parts(data.cast::<f32>(), samples);
                    if pcm.iter().any(|v| !v.is_finite()) {
                        capture.ReleaseBuffer(frames)?;
                        return Err("non-finite capture PCM".into());
                    }
                    if start.elapsed() >= Duration::from_secs(1)
                        && start.elapsed() < Duration::from_millis(milliseconds - 1000)
                        && let Some(checker) = &mut continuity
                    {
                        checker.observe(pcm)?;
                    }
                    capture_queue.push(pcm)?;
                }
                capture.ReleaseBuffer(frames)?;
                captured_total_frames += u64::from(frames);
                packets += 1;
            }
            let available = buffer_frames.saturating_sub(render_client.GetCurrentPadding()?);
            if available > 0 {
                fill(available, &mut rendered)?;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let observation_ms = start.elapsed().as_millis();
        drop(render_guard);
        drop(capture_guard);
        drop(fixture_mmcss);
        let captured_frames = capture_queue.available_frames();
        let mut measured = vec![0.0f32; captured_frames * 2];
        capture_queue.pop(&mut measured)?;
        let start_frame = if mixed {
            captured_frames / 4
        } else {
            captured_frames / 3
        };
        let compared = probe::PERIOD * 2;
        if start_frame + compared > captured_frames {
            return Err("not enough loopback frames".into());
        }
        let selection = &measured[start_frame * 2..(start_frame + compared) * 2];
        let compared_peak = selection
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        let signal = if mixed {
            probe::analyze_against(selection, &mixed_reference(0.65))?
        } else if expected_gain != 1.0 && expected_gain != 0.0 {
            let reference: Vec<_> = (0..probe::PERIOD)
                .map(|f| std::array::from_fn(|c| probe::sample(f, c) * expected_gain))
                .collect();
            probe::analyze_against(selection, &reference)?
        } else {
            probe::analyze(selection)?
        };
        let signal_after_group_mute = if mixed {
            let late = captured_frames * 2 / 3;
            if late + compared > captured_frames {
                return Err("not enough post-mute frames".into());
            }
            Some(probe::analyze_against(
                &measured[late * 2..(late + compared) * 2],
                &mixed_reference(0.0),
            )?)
        } else {
            None
        };
        drop(measured);
        let after = crate::snapshot()?;
        let a = controls(&before);
        let b = controls(&after);
        let default_roles_unchanged = a
            .iter()
            .map(|e| (&e.0, &e.1))
            .eq(b.iter().map(|e| (&e.0, &e.1)));
        let endpoint_volumes_unchanged = a
            .iter()
            .map(|e| (&e.0, &e.2))
            .eq(b.iter().map(|e| (&e.0, &e.2)));
        let passed = signal.passed
            && continuity.as_ref().is_none_or(|c| c.report().passed)
            && signal_after_group_mute.as_ref().is_none_or(|s| s.passed)
            && discontinuities == 0
            && timestamp_errors == 0
            && default_roles_unchanged
            && endpoint_volumes_unchanged
            && before.warnings.is_empty()
            && after.warnings.is_empty();
        Ok(PcmProof {
            endpoint_id: endpoint.into(),
            capture_endpoint_id: capture_endpoint.into(),
            sample_rate: rate,
            rendered_frames: rendered,
            captured_frames,
            captured_total_frames,
            observation_ms,
            continuity: continuity.map(|c| c.report()),
            startup_discontinuities,
            discontinuities,
            timestamp_errors,
            signal,
            expected_gain,
            compared_peak,
            signal_after_group_mute,
            default_roles_unchanged,
            endpoint_volumes_unchanged,
            virtual_endpoint_proven: false,
            passed,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct BridgeProof {
    pub pcm: PcmProof,
    pub bridge: crate::BridgeStats,
    pub passed: bool,
}
struct BridgeWorker(Option<std::thread::JoinHandle<Result<crate::BridgeStats, String>>>);
impl Drop for BridgeWorker {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            let _ = worker.join();
        }
    }
}
pub fn verify_bridge(source: &str, output: &str) -> Result<BridgeProof, Box<dyn Error>> {
    if source.is_empty() || output.is_empty() || source.eq_ignore_ascii_case(output) {
        return Err("choose two distinct explicit render endpoints".into());
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    let own_source = source.to_owned();
    let own_output = output.to_owned();
    let mut worker = BridgeWorker(Some(std::thread::spawn(move || {
        crate::bridge::bridge_lab_ready(&own_source, &own_output, 3, 1.0, Some(sender))
            .map_err(|e| e.to_string())
    })));
    receiver
        .recv_timeout(Duration::from_secs(2))
        .map_err(|e| format!("bridge initialization failed: {e}"))?;
    let pcm = verify_to(source, output, false);
    // Always join, including proof failure. RAII clients stop before returning.
    let bridge = worker
        .0
        .take()
        .expect("worker is present")
        .join()
        .map_err(|_| "bridge worker panicked")??;
    let pcm = pcm?;
    let passed = pcm.passed && bridge.dropped_frames == 0;
    Ok(BridgeProof {
        pcm,
        bridge,
        passed,
    })
}

#[derive(Debug, Serialize)]
pub struct TransportProof {
    pub prepared_outputs: Vec<crate::transport::OutputInfo>,
    pub unity: PcmProof,
    pub attenuated: PcmProof,
    pub muted: PcmProof,
    pub mute_proven: bool,
    pub restored: PcmProof,
    pub stop: crate::transport::StopReport,
    pub restart: PcmProof,
    pub restart_stop: crate::transport::StopReport,
    pub partial_prepare_failure_released: bool,
    pub handles_before_failed_prepare: u32,
    pub handles_after_failed_prepare: u32,
    pub failed_prepare_handle_counts: Vec<u32>,
    pub failed_prepare_errors: Vec<String>,
    pub active_own_sessions_after: usize,
    pub controls_unchanged: bool,
    pub passed: bool,
}
/// Actual worker preparation, PCM/gain, stop, restart and failed preparation.
/// Explicit physical-loopback lab only; no virtual-source provenance claim.
pub fn verify_transport(source: &str, output: &str) -> Result<TransportProof, Box<dyn Error>> {
    verify_transport_duration(source, output, 1200)
}
pub fn verify_transport_long(
    source: &str,
    output: &str,
    seconds: u64,
) -> Result<TransportProof, Box<dyn Error>> {
    if !(10..=300).contains(&seconds) {
        return Err("long transport proof requires10..=300 seconds".into());
    }
    verify_transport_duration(source, output, seconds * 1000)
}
fn own_handle_count() -> Result<u32, Box<dyn Error>> {
    let mut count = 0;
    unsafe {
        windows::Win32::System::Threading::GetProcessHandleCount(
            windows::Win32::System::Threading::GetCurrentProcess(),
            &mut count,
        )?;
    }
    Ok(count)
}
fn verify_transport_duration(
    source: &str,
    output: &str,
    milliseconds: u64,
) -> Result<TransportProof, Box<dyn Error>> {
    use jack_core::runtime::{EnginePlan, GroupPlan, OutputPlan};
    let before = crate::snapshot()?;
    let plan = EnginePlan {
        groups: vec![GroupPlan {
            id: jack_core::GAMING,
            source: source.into(),
            output: output.into(),
            gain: 1.0,
        }],
        outputs: vec![OutputPlan {
            endpoint: output.into(),
            groups: vec![jack_core::GAMING],
        }],
        fallbacks: vec![],
    };
    let prepared = crate::transport::prepare(&plan)?;
    let prepared_outputs = prepared.outputs();
    let mut running = prepared.start()?;
    let unity = verify_to_duration(source, output, false, 1.0, milliseconds)?;
    running.set_group_gain(jack_core::GAMING, 0.35)?;
    let attenuated = verify_to_with_gain(source, output, false, 0.35)?;
    running.set_group_gain(jack_core::GAMING, 0.0)?;
    let muted = verify_to_with_gain(source, output, false, 0.0)?;
    let mute_proven =
        muted.compared_peak < 0.000001 && muted.discontinuities == 0 && muted.timestamp_errors == 0;
    running.set_group_gain(jack_core::GAMING, 1.0)?;
    let restored = verify_to_with_gain(source, output, false, 1.0)?;
    let first_healthy = running.healthy();
    let stop = running.stop();
    let stopped = !running.healthy() && running.live().is_empty();
    let mut restarted = crate::transport::prepare(&plan)?.start()?;
    let restart = verify_to_with_gain(source, output, false, 1.0)?;
    let restart_healthy = restarted.healthy();
    let restart_stop = restarted.stop();
    let mut bad_plan = plan.clone();
    bad_plan.groups.push(GroupPlan {
        id: jack_core::CHAT,
        source: "missing-jack-lab-source".into(),
        output: "missing-jack-lab-output".into(),
        gain: 1.0,
    });
    bad_plan.outputs.push(OutputPlan {
        endpoint: "missing-jack-lab-output".into(),
        groups: vec![jack_core::CHAT],
    });
    let handles_before_failed_prepare = own_handle_count()?;
    let mut failed_prepare_handle_counts = vec![handles_before_failed_prepare];
    let mut failed_prepare_errors = vec![];
    for _ in 0..10 {
        match crate::transport::prepare(&bad_plan) {
            Ok(prepared) => {
                drop(prepared);
                return Err("invalid endpoint unexpectedly prepared".into());
            }
            Err(error) => failed_prepare_errors.push(error),
        }
        failed_prepare_handle_counts.push(own_handle_count()?);
    }
    let handles_after_failed_prepare = own_handle_count()?;
    let after = crate::snapshot()?;
    let active_own_sessions_after = after
        .sessions
        .iter()
        .filter(|s| s.pid == std::process::id() && s.state == 1)
        .count();
    let own_active_before = before
        .sessions
        .iter()
        .filter(|s| s.pid == std::process::id() && s.state == 1)
        .count();
    // Windows COM/RPC may retain process-wide handles after the first activation.
    // Keep every measurement; require no growth across the final five failures,
    // and no active own session. This proves bounded handle behavior in this run,
    // not a complete COM/OS allocation-leak audit.
    let partial_prepare_failure_released = failed_prepare_errors.len() == 10
        && active_own_sessions_after == own_active_before
        && failed_prepare_handle_counts[5..]
            .windows(2)
            .all(|w| w[1] <= w[0]);
    let controls_unchanged = controls(&before) == controls(&after);
    let passed = unity.passed
        && attenuated.passed
        && mute_proven
        && restored.passed
        && restart.passed
        && first_healthy
        && restart_healthy
        && stopped
        && stop.errors.is_empty()
        && restart_stop.errors.is_empty()
        && controls_unchanged
        && partial_prepare_failure_released
        && stop.outputs.len() == 1
        && restart_stop.outputs.len() == 1
        && stop.outputs.iter().chain(&restart_stop.outputs).all(|o| {
            o.dropped_frames == 0
                && o.clipped_samples == 0
                && o.sources
                    .iter()
                    .all(|s| s.discontinuities == 0 && s.timestamp_errors == 0)
        });
    Ok(TransportProof {
        prepared_outputs,
        unity,
        attenuated,
        muted,
        mute_proven,
        restored,
        stop,
        restart,
        restart_stop,
        partial_prepare_failure_released,
        handles_before_failed_prepare,
        handles_after_failed_prepare,
        failed_prepare_handle_counts,
        failed_prepare_errors,
        active_own_sessions_after,
        controls_unchanged,
        passed,
    })
}
