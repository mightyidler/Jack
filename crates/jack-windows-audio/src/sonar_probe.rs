//! Diagnostic only: quiet own-session tone through Sonar, measured at a physical
//! render loopback. Supports Sonar's 8-channel virtual outputs and stereo devices.
//! No microphone recording, default-policy change, or persisted PCM.
use crate::{bridge::Format, platform::Apartment};
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

#[derive(Debug, Serialize)]
pub struct Observation {
    pub source: String,
    pub output: String,
    pub frames: u64,
    pub tone_amplitude: f64,
    pub peak: f32,
}
struct Running(IAudioClient);
impl Drop for Running {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

/// Bounded own-session signal for simultaneous multi-process channel diagnostics.
pub fn play_tone(source: &str, frequency: f64, seconds: u64) -> Result<(), Box<dyn Error>> {
    if !frequency.is_finite()
        || !(100.0..=2000.0).contains(&frequency)
        || !(1..=60).contains(&seconds)
    {
        return Err("invalid tone diagnostic request".into());
    }
    let _apartment = Apartment::new()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDevice(&HSTRING::from(source))?;
        if device.GetState()? != DEVICE_STATE_ACTIVE
            || device.cast::<IMMEndpoint>()?.GetDataFlow()? != eRender
        {
            return Err("active render source required".into());
        }
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = Format(client.GetMixFormat()?);
        let (rate, channels, _) = format.details()?;
        let guid = CoCreateGuid()?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_NOPERSIST,
            1_000_000,
            0,
            format.0,
            Some(&guid),
        )?;
        let control: IAudioSessionControl = client.GetService()?;
        control.SetDisplayName(
            &HSTRING::from("Jack simultaneous quiet signal proof"),
            &guid,
        )?;
        let volume: ISimpleAudioVolume = client.GetService()?;
        volume.SetMasterVolume(1.0, &guid)?;
        let render: IAudioRenderClient = client.GetService()?;
        let buffer = client.GetBufferSize()?;
        let mut rendered = 0u64;
        let mut fill = |frames: u32| -> windows::core::Result<()> {
            let data = render.GetBuffer(frames)?;
            let samples =
                std::slice::from_raw_parts_mut(data.cast::<f32>(), frames as usize * channels);
            for (offset, frame) in samples.chunks_exact_mut(channels).enumerate() {
                frame.fill(0.0);
                let value = (std::f64::consts::TAU * frequency * (rendered + offset as u64) as f64
                    / rate as f64)
                    .sin() as f32
                    * 0.002;
                for sample in frame.iter_mut().take(2) {
                    *sample = value;
                }
            }
            rendered += u64::from(frames);
            render.ReleaseBuffer(frames, 0)
        };
        fill(buffer)?;
        client.Start()?;
        let _running = Running(client.clone());
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(seconds) {
            let available = buffer.saturating_sub(client.GetCurrentPadding()?);
            if available > 0 {
                fill(available)?;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    Ok(())
}

/// Measure a known continuous tone from an external test app, without rendering
/// another signal or opening a capture/microphone device.
pub fn measure(output: &str, frequency: f64) -> Result<Observation, Box<dyn Error>> {
    if !frequency.is_finite() || !(100.0..=2000.0).contains(&frequency) {
        return Err("invalid probe frequency".into());
    }
    let _apartment = Apartment::new()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDevice(&HSTRING::from(output))?;
        if device.GetState()? != DEVICE_STATE_ACTIVE
            || device.cast::<IMMEndpoint>()?.GetDataFlow()? != eRender
        {
            return Err("active render output required".into());
        }
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = Format(client.GetMixFormat()?);
        let (rate, channels, _) = format.details()?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            1_000_000,
            0,
            format.0,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        let _running = Running(client);
        let start = Instant::now();
        let (mut total, mut frames_measured, mut sine, mut cosine, mut peak) =
            (0u64, 0u64, 0.0f64, 0.0f64, 0.0f32);
        while start.elapsed() < Duration::from_millis(900) {
            for _ in 0..64 {
                if capture.GetNextPacketSize()? == 0 {
                    break;
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let samples = if data.is_null() || flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0
                {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(data.cast::<f32>(), frames as usize * channels)
                };
                if start.elapsed() > Duration::from_millis(150) {
                    for frame in 0..frames as usize {
                        let sample = if samples.is_empty() {
                            0.0
                        } else {
                            samples[frame * channels]
                        };
                        let angle =
                            std::f64::consts::TAU * frequency * (total + frame as u64) as f64
                                / rate as f64;
                        sine += f64::from(sample) * angle.sin();
                        cosine += f64::from(sample) * angle.cos();
                        peak = peak.max(sample.abs());
                    }
                    frames_measured += u64::from(frames);
                }
                total += u64::from(frames);
                capture.ReleaseBuffer(frames)?;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // WASAPI can deliver no packets at all while the output is completely idle.
        Ok(Observation {
            source: "external test application".into(),
            output: output.into(),
            frames: frames_measured,
            tone_amplitude: if frames_measured == 0 {
                0.0
            } else {
                2.0 * sine.hypot(cosine) / frames_measured as f64
            },
            peak,
        })
    }
}

pub fn observe(source: &str, output: &str) -> Result<Observation, Box<dyn Error>> {
    let _apartment = Apartment::new()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let source_device = enumerator.GetDevice(&HSTRING::from(source))?;
        let output_device = enumerator.GetDevice(&HSTRING::from(output))?;
        for device in [&source_device, &output_device] {
            if device.GetState()? != DEVICE_STATE_ACTIVE
                || device.cast::<IMMEndpoint>()?.GetDataFlow()? != eRender
            {
                return Err("active render devices required".into());
            }
        }
        let render_client: IAudioClient = source_device.Activate(CLSCTX_ALL, None)?;
        let capture_client: IAudioClient = output_device.Activate(CLSCTX_ALL, None)?;
        let render_format = Format(render_client.GetMixFormat()?);
        let capture_format = Format(capture_client.GetMixFormat()?);
        let (render_rate, render_channels, _) = render_format.details()?;
        let (capture_rate, capture_channels, _) = capture_format.details()?;
        let guid = CoCreateGuid()?;
        render_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_NOPERSIST,
            1_000_000,
            0,
            render_format.0,
            Some(&guid),
        )?;
        capture_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            1_000_000,
            0,
            capture_format.0,
            None,
        )?;
        let control: IAudioSessionControl = render_client.GetService()?;
        control.SetDisplayName(&HSTRING::from("Jack Sonar quiet signal proof"), &guid)?;
        let volume: ISimpleAudioVolume = render_client.GetService()?;
        volume.SetMasterVolume(1.0, &guid)?;
        let render: IAudioRenderClient = render_client.GetService()?;
        let capture: IAudioCaptureClient = capture_client.GetService()?;
        let buffer = render_client.GetBufferSize()?;
        let mut rendered = 0u64;
        let fill = |frames: u32, rendered: &mut u64| -> windows::core::Result<()> {
            let target = render.GetBuffer(frames)?;
            let samples = std::slice::from_raw_parts_mut(
                target.cast::<f32>(),
                frames as usize * render_channels,
            );
            for (offset, frame) in samples.chunks_exact_mut(render_channels).enumerate() {
                frame.fill(0.0);
                let sample = (std::f64::consts::TAU * 431.0 * (*rendered + offset as u64) as f64
                    / render_rate as f64)
                    .sin() as f32
                    * 0.002;
                for channel in frame.iter_mut().take(2) {
                    *channel = sample;
                }
            }
            *rendered += u64::from(frames);
            render.ReleaseBuffer(frames, 0)
        };
        fill(buffer, &mut rendered)?;
        capture_client.Start()?;
        let _capture_guard = Running(capture_client.clone());
        render_client.Start()?;
        let _render_guard = Running(render_client.clone());
        let start = Instant::now();
        let (mut captured, mut measured, mut sine, mut cosine, mut peak) =
            (0u64, 0u64, 0.0f64, 0.0f64, 0.0f32);
        while start.elapsed() < Duration::from_millis(1100) {
            for _ in 0..64 {
                if capture.GetNextPacketSize()? == 0 {
                    break;
                }
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let samples = if silent || data.is_null() {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(
                        data.cast::<f32>(),
                        frames as usize * capture_channels,
                    )
                };
                if start.elapsed() > Duration::from_millis(250) {
                    for frame in 0..frames as usize {
                        let sample = if samples.is_empty() {
                            0.0
                        } else {
                            samples[frame * capture_channels]
                        };
                        let angle =
                            std::f64::consts::TAU * 431.0 * (captured + frame as u64) as f64
                                / capture_rate as f64;
                        sine += f64::from(sample) * angle.sin();
                        cosine += f64::from(sample) * angle.cos();
                        peak = peak.max(sample.abs());
                    }
                    measured += u64::from(frames);
                }
                captured += u64::from(frames);
                capture.ReleaseBuffer(frames)?;
            }
            let available = buffer.saturating_sub(render_client.GetCurrentPadding()?);
            if available > 0 {
                fill(available, &mut rendered)?;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if measured == 0 {
            return Err("no physical loopback frames received".into());
        }
        Ok(Observation {
            source: source.into(),
            output: output.into(),
            frames: measured,
            tone_amplitude: 2.0 * sine.hypot(cosine) / measured as f64,
            peak,
        })
    }
}
