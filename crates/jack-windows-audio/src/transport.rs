//! Persistent, event-driven loopback transport: one MTA/MMCSS worker per output.
//! This layer never assigns apps or changes defaults/endpoint volume. The product
//! controller must verify Jack source provenance before passing an EnginePlan.
//! Physical loopback tests verify transport, not Jack virtual endpoint identity.
use crate::{bridge::Format, platform::Apartment};
use jack_core::runtime::{EnginePlan, GroupPlan};
use jack_engine::{
    mixer::{DeviceMixer, GroupStats, PcmFormat},
    resampler::StreamingResampler,
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use uuid::Uuid;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Media::Audio::*,
        System::{
            Com::{CLSCTX_ALL, CoCreateGuid, CoCreateInstance},
            Threading::*,
        },
    },
    core::{HSTRING, Interface, PCWSTR, w},
};

const PREPARED: u8 = 0;
const START: u8 = 1;
const RUNNING: u8 = 2;
const STOP: u8 = 3;
const STOPPED: u8 = 4;
const FAULTED: u8 = 5;
type TransportResult<T> = Result<T, String>;

struct Event(HANDLE);
// SAFETY: a kernel event may be signaled/waited on from different threads. Arc
// retains the sole owned handle until all clients and waiters have been released.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}
impl Event {
    fn new() -> TransportResult<Arc<Self>> {
        unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map(|h| Arc::new(Self(h)))
            .map_err(|e| e.to_string())
    }
    fn signal(&self) -> TransportResult<()> {
        unsafe { SetEvent(self.0) }.map_err(|e| e.to_string())
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub(super) struct Mmcss(HANDLE);
impl Mmcss {
    pub(super) fn new() -> TransportResult<Self> {
        let mut task = 0;
        let guard = Self(
            unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) }
                .map_err(|e| e.to_string())?,
        );
        unsafe { AvSetMmThreadPriority(guard.0, AVRT_PRIORITY_HIGH) }.map_err(|e| e.to_string())?;
        Ok(guard)
    }
}
impl Drop for Mmcss {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}
struct Stream {
    client: IAudioClient,
    _event: Arc<Event>,
    running: bool,
}
impl Stream {
    fn start(&mut self) -> TransportResult<()> {
        unsafe { self.client.Start() }.map_err(|e| e.to_string())?;
        self.running = true;
        Ok(())
    }
    fn stop(&mut self) -> TransportResult<()> {
        if self.running {
            unsafe { self.client.Stop() }.map_err(|e| e.to_string())?;
            self.running = false;
        }
        Ok(())
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
struct CapturePacket<'a> {
    capture: &'a IAudioCaptureClient,
    frames: Option<u32>,
}
impl CapturePacket<'_> {
    fn release(mut self) -> TransportResult<()> {
        let frames = self.frames.take().expect("capture packet is held");
        unsafe { self.capture.ReleaseBuffer(frames) }.map_err(|e| e.to_string())
    }
}
impl Drop for CapturePacket<'_> {
    fn drop(&mut self) {
        if let Some(frames) = self.frames {
            unsafe {
                let _ = self.capture.ReleaseBuffer(frames);
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputInfo {
    pub endpoint: String,
    pub format: PcmFormat,
    pub buffer_frames: u32,
    pub groups: Vec<Uuid>,
    pub mmcss: bool,
    pub event_driven: bool,
}
#[derive(Debug, Serialize)]
pub struct SourceReport {
    pub group: Uuid,
    pub endpoint: String,
    pub source_rate: u32,
    pub linear_resampling: bool,
    pub applied_gain: f32,
    pub captured_frames: u64,
    pub startup_discontinuities: u64,
    pub discontinuities: u64,
    pub timestamp_errors: u64,
    pub mixer: GroupStats,
}
#[derive(Debug, Serialize)]
pub struct OutputReport {
    pub info: OutputInfo,
    pub sources: Vec<SourceReport>,
    pub rendered_frames: u64,
    pub event_wakeups: u64,
    pub health_checks: u64,
    pub dropped_frames: u64,
    pub clipped_samples: u64,
    pub elapsed_ms: u128,
}
#[derive(Debug, Default, Serialize)]
pub struct StopReport {
    pub outputs: Vec<OutputReport>,
    pub errors: Vec<String>,
    pub elapsed_ms: u128,
}
#[derive(Debug, Serialize)]
pub struct LiveOutput {
    pub endpoint: String,
    pub healthy: bool,
    pub rendered_frames: u64,
    pub event_wakeups: u64,
    pub dropped_frames: u64,
}
#[derive(Default)]
struct Counters {
    rendered: AtomicU64,
    wakeups: AtomicU64,
    dropped: AtomicU64,
}
struct GainControl {
    id: Uuid,
    gain: Arc<AtomicU32>,
}
struct Worker {
    info: OutputInfo,
    phase: Arc<AtomicU8>,
    wake: Arc<Event>,
    controls: Vec<GainControl>,
    counters: Arc<Counters>,
    started: mpsc::Receiver<TransportResult<()>>,
    thread: Option<JoinHandle<TransportResult<OutputReport>>>,
}
impl Worker {
    fn request_stop(&self) {
        self.phase.store(STOP, Ordering::Release);
        let _ = self.wake.signal();
    }
    fn join(&mut self) -> TransportResult<OutputReport> {
        self.thread
            .take()
            .ok_or("worker already joined")?
            .join()
            .map_err(|_| "audio worker panicked")?
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.request_stop();
            let _ = self.join();
        }
    }
}

/// All clients and PCM buffers are acquired on the eventual worker threads.
/// Dropping this value without starting closes every prepared client.
pub struct PreparedTransport {
    workers: Vec<Worker>,
}
pub struct RunningTransport {
    workers: Vec<Worker>,
}
impl PreparedTransport {
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.workers.iter().map(|w| w.info.clone()).collect()
    }
    pub fn start(self) -> TransportResult<RunningTransport> {
        let running = RunningTransport {
            workers: self.workers,
        };
        for worker in &running.workers {
            worker.phase.store(START, Ordering::Release);
            worker.wake.signal()?;
        }
        for worker in &running.workers {
            worker
                .started
                .recv_timeout(Duration::from_secs(3))
                .map_err(|e| format!("{} start: {e}", worker.info.endpoint))??;
        }
        if !running.healthy() {
            return Err("worker stopped during transport start".into());
        }
        Ok(running)
    }
}
impl RunningTransport {
    pub fn healthy(&self) -> bool {
        !self.workers.is_empty()
            && self.workers.iter().all(|w| {
                w.phase.load(Ordering::Acquire) == RUNNING
                    && w.thread.as_ref().is_some_and(|t| !t.is_finished())
            })
    }
    pub fn set_group_gain(&self, group: Uuid, gain: f32) -> TransportResult<()> {
        if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
            return Err("gain must be finite within 0..=1".into());
        }
        if !self.healthy() {
            return Err("transport is not healthy".into());
        }
        for worker in &self.workers {
            if let Some(control) = worker.controls.iter().find(|c| c.id == group) {
                control.gain.store(gain.to_bits(), Ordering::Release);
                worker.wake.signal()?;
                return Ok(());
            }
        }
        Err("unknown audio group".into())
    }
    pub fn live(&self) -> Vec<LiveOutput> {
        self.workers
            .iter()
            .map(|w| LiveOutput {
                endpoint: w.info.endpoint.clone(),
                healthy: w.phase.load(Ordering::Acquire) == RUNNING
                    && w.thread.as_ref().is_some_and(|t| !t.is_finished()),
                rendered_frames: w.counters.rendered.load(Ordering::Relaxed),
                event_wakeups: w.counters.wakeups.load(Ordering::Relaxed),
                dropped_frames: w.counters.dropped.load(Ordering::Relaxed),
            })
            .collect()
    }
    /// Signal all outputs before joining any. No output is abandoned after an error.
    pub fn stop(&mut self) -> StopReport {
        let start = Instant::now();
        let mut report = StopReport::default();
        for worker in &self.workers {
            worker.request_stop();
        }
        for worker in &mut self.workers {
            match worker.join() {
                Ok(output) => report.outputs.push(output),
                Err(error) => report
                    .errors
                    .push(format!("{}: {error}", worker.info.endpoint)),
            }
        }
        self.workers.clear();
        report.elapsed_ms = start.elapsed().as_millis();
        report
    }
}
impl Drop for RunningTransport {
    fn drop(&mut self) {
        if !self.workers.is_empty() {
            let _ = self.stop();
        }
    }
}

/// Explicit transport plan only. No default discovery, policy changes, microphone
/// capture, or inference of virtual-source identity from names happens here.
pub fn prepare(plan: &EnginePlan) -> TransportResult<PreparedTransport> {
    validate_plan(plan)?;
    let mut prepared = PreparedTransport { workers: vec![] };
    for output in &plan.outputs {
        let groups: Vec<_> = plan
            .groups
            .iter()
            .filter(|g| g.output.eq_ignore_ascii_case(&output.endpoint))
            .cloned()
            .collect();
        prepared
            .workers
            .push(prepare_worker(output.endpoint.clone(), groups)?);
    }
    Ok(prepared)
}
fn validate_plan(plan: &EnginePlan) -> TransportResult<()> {
    if plan.groups.is_empty() || plan.outputs.is_empty() {
        return Err("empty audio plan".into());
    }
    let mut ids = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    for output in &plan.outputs {
        if output.endpoint.trim().is_empty()
            || !outputs.insert(output.endpoint.to_lowercase())
            || output.groups.is_empty()
        {
            return Err("invalid/duplicate/empty output".into());
        }
    }
    for group in &plan.groups {
        if group.id.is_nil()
            || !ids.insert(group.id)
            || group.source.trim().is_empty()
            || !sources.insert(group.source.to_lowercase())
            || !outputs.contains(&group.output.to_lowercase())
            || !group.gain.is_finite()
            || !(0.0..=1.0).contains(&group.gain)
        {
            return Err("invalid/duplicate group, source, gain or output".into());
        }
    }
    if !sources.is_disjoint(&outputs) {
        return Err("capture sources and outputs overlap: feedback risk".into());
    }
    for output in &plan.outputs {
        let actual: BTreeSet<_> = plan
            .groups
            .iter()
            .filter(|g| g.output.eq_ignore_ascii_case(&output.endpoint))
            .map(|g| g.id)
            .collect();
        let declared: BTreeSet<_> = output.groups.iter().copied().collect();
        if actual != declared || declared.len() != output.groups.len() {
            return Err("output/group ownership mismatch".into());
        }
    }
    Ok(())
}
fn prepare_worker(endpoint: String, groups: Vec<GroupPlan>) -> TransportResult<Worker> {
    let wake = Event::new()?;
    let phase = Arc::new(AtomicU8::new(PREPARED));
    let counters = Arc::new(Counters::default());
    let controls: Vec<_> = groups
        .iter()
        .map(|g| GainControl {
            id: g.id,
            gain: Arc::new(AtomicU32::new(g.gain.to_bits())),
        })
        .collect();
    let gains: Vec<_> = controls.iter().map(|c| c.gain.clone()).collect();
    let (ready, initialized) = mpsc::sync_channel(1);
    let (started, start_result) = mpsc::sync_channel(1);
    let own_wake = wake.clone();
    let own_phase = phase.clone();
    let own_counters = counters.clone();
    let thread = thread::Builder::new()
        .name("jack-output".into())
        .spawn(move || {
            let result = run_worker(
                endpoint,
                groups,
                gains,
                &own_wake,
                &own_phase,
                &own_counters,
                &ready,
                &started,
            );
            own_phase.store(
                if result.is_ok() { STOPPED } else { FAULTED },
                Ordering::Release,
            );
            if let Err(error) = &result {
                let _ = ready.try_send(Err(error.clone()));
                let _ = started.try_send(Err(error.clone()));
            }
            result
        })
        .map_err(|e| e.to_string())?;
    // Keep RAII ownership even if the initialization result times out or fails.
    let mut worker = Worker {
        info: OutputInfo {
            endpoint: String::new(),
            format: PcmFormat {
                sample_rate: 0,
                channels: 0,
                channel_mask: 0,
            },
            buffer_frames: 0,
            groups: vec![],
            mmcss: false,
            event_driven: false,
        },
        phase,
        wake,
        controls,
        counters,
        started: start_result,
        thread: Some(thread),
    };
    worker.info = initialized
        .recv_timeout(Duration::from_secs(5))
        .map_err(|e| format!("transport preparation: {e}"))??;
    Ok(worker)
}
struct Source {
    capture: IAudioCaptureClient,
    stream: Stream,
    device: IMMDevice,
    id: Uuid,
    endpoint: String,
    rate: u32,
    buffer_frames: u32,
    gain: Arc<AtomicU32>,
    applied_gain: f32,
    resampler: StreamingResampler,
    captured: u64,
    packets: u64,
    startup_discontinuities: u64,
    discontinuities: u64,
    timestamp_errors: u64,
}
impl Source {
    fn drain(&mut self, channels: usize) -> TransportResult<u64> {
        let mut dropped = 0;
        // Bound per-source work so a busy source cannot starve other groups/output.
        for _ in 0..8 {
            if unsafe { self.capture.GetNextPacketSize() }.map_err(|e| e.to_string())? == 0 {
                break;
            }
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
            }
            .map_err(|e| e.to_string())?;
            let packet = CapturePacket {
                capture: &self.capture,
                frames: Some(frames),
            };
            if frames > self.buffer_frames {
                return Err("capture packet exceeds client buffer capacity".into());
            }
            if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                if self.packets == 0 {
                    self.startup_discontinuities += 1;
                } else {
                    self.discontinuities += 1;
                    self.resampler.reset();
                }
            }
            if flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0 {
                self.timestamp_errors += 1;
            }
            let accepted = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                self.resampler.push_silence(frames as usize)
            } else if frames == 0 {
                0
            } else {
                if data.is_null() {
                    return Err("null capture PCM without SILENT".into());
                }
                let pcm = unsafe {
                    std::slice::from_raw_parts(data.cast::<f32>(), frames as usize * channels)
                };
                self.resampler.push(pcm).map_err(str::to_owned)?
            };
            packet.release()?;
            self.captured += u64::from(frames);
            self.packets += 1;
            dropped += u64::from(frames) - accepted as u64;
        }
        Ok(dropped)
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    endpoint: String,
    groups: Vec<GroupPlan>,
    gains: Vec<Arc<AtomicU32>>,
    wake: &Arc<Event>,
    phase: &AtomicU8,
    counters: &Counters,
    ready: &mpsc::SyncSender<TransportResult<OutputInfo>>,
    started: &mpsc::SyncSender<TransportResult<()>>,
) -> TransportResult<OutputReport> {
    let _apartment = Apartment::new().map_err(|e| e.to_string())?;
    // Clients and services never leave this MTA thread. Allocation/validation below
    // precedes Start; the servicing loop uses only fixed PCM buffers and atomics.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let device = enumerator
            .GetDevice(&HSTRING::from(&endpoint))
            .map_err(|e| e.to_string())?;
        check_device(&device)?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| e.to_string())?;
        let format = Format(client.GetMixFormat().map_err(|e| e.to_string())?);
        let (rate, channels, mask) = format.details().map_err(|e| e.to_string())?;
        let pcm_format = PcmFormat {
            sample_rate: rate,
            channels,
            channel_mask: mask,
        };
        pcm_format.validate().map_err(str::to_owned)?;
        let render_event = Event::new()?;
        let guid = CoCreateGuid().map_err(|e| e.to_string())?;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_NOPERSIST,
                0,
                0,
                format.0,
                Some(&guid),
            )
            .map_err(|e| e.to_string())?;
        let mut output = Stream {
            client,
            _event: render_event.clone(),
            running: false,
        };
        output
            .client
            .SetEventHandle(render_event.0)
            .map_err(|e| e.to_string())?;
        let render: IAudioRenderClient = output.client.GetService().map_err(|e| e.to_string())?;
        let own_volume: ISimpleAudioVolume =
            output.client.GetService().map_err(|e| e.to_string())?;
        own_volume
            .SetMasterVolume(1.0, &guid)
            .map_err(|e| e.to_string())?;
        let frames = output.client.GetBufferSize().map_err(|e| e.to_string())?;
        if frames == 0 || frames as usize > rate as usize * 2 {
            return Err("invalid WASAPI buffer capacity".into());
        }
        let mut mixer = DeviceMixer::new(
            pcm_format,
            frames as usize,
            (rate as usize / 4).max(frames as usize),
        )
        .map_err(str::to_owned)?;
        let mut sources = Vec::with_capacity(groups.len());
        for (group, gain) in groups.iter().zip(gains) {
            let source = enumerator
                .GetDevice(&HSTRING::from(&group.source))
                .map_err(|e| e.to_string())?;
            check_device(&source)?;
            let capture_client: IAudioClient = source
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| e.to_string())?;
            let capture_format = Format(capture_client.GetMixFormat().map_err(|e| e.to_string())?);
            let (source_rate, source_channels, source_mask) =
                capture_format.details().map_err(|e| e.to_string())?;
            if (source_channels, source_mask) != (channels, mask) {
                return Err("source/output channel layout mismatch".into());
            }
            capture_client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK
                        | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_NOPERSIST,
                    0,
                    0,
                    capture_format.0,
                    Some(&guid),
                )
                .map_err(|e| e.to_string())?;
            let stream = Stream {
                client: capture_client,
                _event: wake.clone(),
                running: false,
            };
            // All sources share the worker's wake event; no MAXIMUM_WAIT_OBJECTS
            // limit or additional capture threads is introduced as a group limit.
            stream
                .client
                .SetEventHandle(wake.0)
                .map_err(|e| e.to_string())?;
            let capture_frames = stream.client.GetBufferSize().map_err(|e| e.to_string())?;
            if capture_frames == 0 || capture_frames as usize > source_rate as usize * 2 {
                return Err("invalid capture buffer capacity".into());
            }
            let capture: IAudioCaptureClient =
                stream.client.GetService().map_err(|e| e.to_string())?;
            mixer
                .add_group(group.id, group.gain)
                .map_err(str::to_owned)?;
            sources.push(Source {
                capture,
                stream,
                device: source,
                id: group.id,
                endpoint: group.source.clone(),
                rate: source_rate,
                buffer_frames: capture_frames,
                gain,
                applied_gain: group.gain,
                resampler: StreamingResampler::new(
                    source_rate,
                    rate,
                    channels,
                    (source_rate as usize / 4).max(2),
                )
                .map_err(str::to_owned)?,
                captured: 0,
                packets: 0,
                startup_discontinuities: 0,
                discontinuities: 0,
                timestamp_errors: 0,
            });
        }
        let mut output_pcm = vec![0.0f32; frames as usize * channels];
        let mut converted = vec![0.0f32; output_pcm.len()];
        let info = OutputInfo {
            endpoint,
            format: pcm_format,
            buffer_frames: frames,
            groups: groups.iter().map(|g| g.id).collect(),
            mmcss: true,
            event_driven: true,
        };
        let _mmcss = Mmcss::new()?;
        ready.send(Ok(info.clone())).map_err(|e| e.to_string())?;
        while phase.load(Ordering::Acquire) == PREPARED {
            let result = WaitForSingleObject(wake.0, 500);
            if result == WAIT_FAILED {
                return Err(windows::core::Error::from_thread().to_string());
            }
        }
        if phase.load(Ordering::Acquire) == STOP {
            return Ok(OutputReport {
                info,
                sources: vec![],
                rendered_frames: 0,
                event_wakeups: 0,
                health_checks: 0,
                dropped_frames: 0,
                clipped_samples: 0,
                elapsed_ms: 0,
            });
        }
        for source in &mut sources {
            source.stream.start()?;
        }
        render.GetBuffer(frames).map_err(|e| e.to_string())?;
        render
            .ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)
            .map_err(|e| e.to_string())?;
        output.start()?;
        // A concurrent stop during Start must never be overwritten by RUNNING.
        phase
            .compare_exchange(START, RUNNING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "transport start was cancelled")?;
        started.send(Ok(())).map_err(|e| e.to_string())?;
        let start = Instant::now();
        let mut check_at = start;
        let mut last_render_progress = start;
        let mut health_checks = 0;
        let mut clipped_samples = 0;
        while phase.load(Ordering::Acquire) != STOP {
            let result = WaitForMultipleObjects(&[wake.0, render_event.0], false, 500);
            if result == WAIT_FAILED {
                return Err(windows::core::Error::from_thread().to_string());
            }
            if result != WAIT_OBJECT_0 && result.0 != WAIT_OBJECT_0.0 + 1 && result != WAIT_TIMEOUT
            {
                return Err("unexpected audio wait result".into());
            }
            if phase.load(Ordering::Acquire) == STOP {
                break;
            }
            if start.elapsed().as_millis() > 0 && check_at.elapsed() >= Duration::from_millis(500) {
                check_device(&device)?;
                for source in &sources {
                    check_device(&source.device)?;
                }
                check_at = Instant::now();
                health_checks += 1;
            }
            if result == WAIT_TIMEOUT || last_render_progress.elapsed() > Duration::from_millis(500)
            {
                return Err("WASAPI render event stalled".into());
            }
            counters.wakeups.fetch_add(1, Ordering::Relaxed);
            for source in &mut sources {
                counters
                    .dropped
                    .fetch_add(source.drain(channels)?, Ordering::Relaxed);
                let gain = f32::from_bits(source.gain.load(Ordering::Acquire));
                if gain != source.applied_gain {
                    mixer
                        .set_gain(source.id, gain, rate as usize / 200)
                        .map_err(str::to_owned)?;
                    source.applied_gain = gain;
                }
            }
            let padding = output
                .client
                .GetCurrentPadding()
                .map_err(|e| e.to_string())?;
            if padding > frames {
                return Err("render padding exceeds capacity".into());
            }
            let available = frames - padding;
            if available == 0 {
                continue;
            }
            let samples = available as usize * channels;
            for source in &mut sources {
                let real = source
                    .resampler
                    .process(&mut converted[..samples])
                    .map_err(str::to_owned)?;
                let pushed = mixer
                    .push(source.id, &converted[..real * channels])
                    .map_err(str::to_owned)?;
                counters
                    .dropped
                    .fetch_add(pushed.dropped_frames as u64, Ordering::Relaxed);
            }
            let pcm = &mut output_pcm[..samples];
            let mixed = mixer.render(pcm).map_err(str::to_owned)?;
            clipped_samples += mixed.clipped_samples as u64;
            let target = render.GetBuffer(available).map_err(|e| e.to_string())?;
            if target.is_null() {
                let _ = render.ReleaseBuffer(available, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
                return Err("null render buffer".into());
            }
            std::ptr::copy_nonoverlapping(
                pcm.as_ptr().cast::<u8>(),
                target,
                std::mem::size_of_val(pcm),
            );
            render
                .ReleaseBuffer(available, 0)
                .map_err(|e| e.to_string())?;
            counters
                .rendered
                .fetch_add(u64::from(available), Ordering::Relaxed);
            last_render_progress = Instant::now();
        }
        // Attempt every Stop, including failures. Stream Drop retries a failed Stop.
        let mut stop_error = None;
        for source in &mut sources {
            if let Err(error) = source.stream.stop() {
                stop_error.get_or_insert(error);
            }
        }
        if let Err(error) = output.stop() {
            stop_error.get_or_insert(error);
        }
        if let Some(error) = stop_error {
            return Err(error);
        }
        let source_reports = sources
            .iter()
            .map(|source| SourceReport {
                group: source.id,
                endpoint: source.endpoint.clone(),
                source_rate: source.rate,
                linear_resampling: source.rate != rate,
                applied_gain: source.applied_gain,
                captured_frames: source.captured,
                startup_discontinuities: source.startup_discontinuities,
                discontinuities: source.discontinuities,
                timestamp_errors: source.timestamp_errors,
                mixer: mixer.stats(source.id).expect("prepared group persists"),
            })
            .collect();
        Ok(OutputReport {
            info,
            sources: source_reports,
            rendered_frames: counters.rendered.load(Ordering::Relaxed),
            event_wakeups: counters.wakeups.load(Ordering::Relaxed),
            health_checks,
            dropped_frames: counters.dropped.load(Ordering::Relaxed),
            clipped_samples,
            elapsed_ms: start.elapsed().as_millis(),
        })
    }
}
fn check_device(device: &IMMDevice) -> TransportResult<()> {
    unsafe {
        if device.GetState().map_err(|e| e.to_string())? != DEVICE_STATE_ACTIVE
            || device
                .cast::<IMMEndpoint>()
                .map_err(|e| e.to_string())?
                .GetDataFlow()
                .map_err(|e| e.to_string())?
                != eRender
        {
            return Err("device is unavailable or is not a render endpoint".into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use jack_core::runtime::OutputPlan;
    fn plan() -> EnginePlan {
        EnginePlan {
            groups: vec![
                GroupPlan {
                    id: jack_core::GAMING,
                    source: "a".into(),
                    output: "out".into(),
                    gain: 1.0,
                },
                GroupPlan {
                    id: jack_core::CHAT,
                    source: "b".into(),
                    output: "out".into(),
                    gain: 0.5,
                },
            ],
            outputs: vec![OutputPlan {
                endpoint: "out".into(),
                groups: vec![jack_core::GAMING, jack_core::CHAT],
            }],
            fallbacks: vec![],
        }
    }
    #[test]
    fn feedback_cycles_and_aliases_are_rejected_before_opening_devices() {
        let mut p = plan();
        p.groups[0].source = "OUT".into();
        assert!(prepare(&p).is_err());
        p.groups[0].source = "B".into();
        assert!(prepare(&p).is_err());
    }
    #[test]
    fn output_ownership_must_match_all_groups_exactly() {
        let mut p = plan();
        assert!(validate_plan(&p).is_ok());
        p.outputs[0].groups.pop();
        assert!(prepare(&p).is_err());
        p.outputs[0].groups.push(jack_core::GAMING);
        assert!(prepare(&p).is_err());
    }
    #[test]
    fn invalid_gain_and_empty_plans_do_not_open_clients() {
        let mut p = plan();
        p.groups[0].gain = f32::NAN;
        assert!(prepare(&p).is_err());
        p.groups.clear();
        assert!(prepare(&p).is_err());
    }
}
