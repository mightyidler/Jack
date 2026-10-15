//! Integration fixture creates its own non-persistent silent render session only.
//! No microphone capture, default-policy writes or endpoint-volume writes.
use crate::{
    AudioMonitor, MonitorDiagnostics, Snapshot,
    platform::{Apartment, owned_string},
};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    error::Error,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Media::Audio::*,
        System::Com::{CLSCTX_ALL, CoCreateGuid, CoCreateInstance, CoTaskMemFree},
    },
    core::{HSTRING, Interface, Result},
};

#[derive(Debug, Serialize)]
pub struct EventVerification {
    pub fixture_instance_id: String,
    pub observed_fixture: bool,
    pub observed_active: bool,
    pub observed_inactive_after_active: bool,
    pub session_created_callback_received: bool,
    pub session_change_callback_received: bool,
    pub default_roles_unchanged: bool,
    pub endpoint_volumes_unchanged: bool,
    pub diagnostics: MonitorDiagnostics,
    pub warnings: Vec<String>,
    pub passed: bool,
}

fn defaults(snapshot: &Snapshot) -> BTreeSet<(String, String, String)> {
    snapshot
        .endpoints
        .iter()
        .flat_map(|e| {
            e.defaults
                .iter()
                .map(|r| (e.flow.to_string(), r.to_string(), e.id.clone()))
        })
        .collect()
}
fn volumes(snapshot: &Snapshot) -> BTreeSet<(String, u32, bool)> {
    snapshot
        .endpoints
        .iter()
        .filter_map(|e| {
            e.volume
                .as_ref()
                .map(|v| (e.id.clone(), v.level.to_bits(), v.muted))
        })
        .collect()
}
struct Format(*mut WAVEFORMATEX);
impl Drop for Format {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.cast()));
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

pub(super) fn silent_session(endpoint: &str, sender: mpsc::Sender<String>) -> Result<()> {
    let _apartment = Apartment::new()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDevice(&HSTRING::from(endpoint))?;
        if device.cast::<IMMEndpoint>()?.GetDataFlow()? != eRender {
            return Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let format = Format(client.GetMixFormat()?);
        let session_guid = CoCreateGuid()?;
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_NOPERSIST,
            1_000_000,
            0,
            format.0,
            Some(&session_guid),
        )?;
        let control: IAudioSessionControl = client.GetService()?;
        control.SetDisplayName(&HSTRING::from("Jack silent event fixture"), &session_guid)?;
        let extended: IAudioSessionControl2 = control.cast()?;
        let _ = sender.send(owned_string(extended.GetSessionInstanceIdentifier()?)?);
        let volume: ISimpleAudioVolume = client.GetService()?;
        let render: IAudioRenderClient = client.GetService()?;
        let frames = client.GetBufferSize()?;
        render.GetBuffer(frames)?;
        render.ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
        client.Start()?;
        let running = Running(client.clone());
        let start = Instant::now();
        let mut step = 0;
        while start.elapsed() < Duration::from_millis(1000) {
            let available = frames.saturating_sub(client.GetCurrentPadding()?);
            if available > 0 {
                render.GetBuffer(available)?;
                render.ReleaseBuffer(available, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)?;
            }
            if step == 0 && start.elapsed() > Duration::from_millis(300) {
                volume.SetMasterVolume(0.25, &session_guid)?;
                step = 1;
            }
            if step == 1 && start.elapsed() > Duration::from_millis(600) {
                volume.SetMasterVolume(0.5, &session_guid)?;
                step = 2;
            }
            thread::sleep(Duration::from_millis(10));
        }
        drop(running);
        // Leave the inactive session alive long enough for an asynchronous snapshot.
        thread::sleep(Duration::from_millis(600));
        Ok(())
    }
}

pub fn verify_session_events(
    endpoint_id: &str,
) -> std::result::Result<EventVerification, Box<dyn Error>> {
    let mut monitor = AudioMonitor::new()?;
    let before = monitor.snapshot()?;
    if !before
        .endpoints
        .iter()
        .any(|e| e.id == endpoint_id && e.flow == "render")
    {
        return Err("choose an active explicit render Endpoint ID".into());
    }
    let before_counts = monitor.diagnostics();
    let (sender, receiver) = mpsc::channel();
    let endpoint = endpoint_id.to_string();
    let worker =
        thread::spawn(move || silent_session(&endpoint, sender).map_err(|e| e.to_string()));
    let observed = (|| -> std::result::Result<_, Box<dyn Error>> {
        let mut fixture_id = String::new();
        let mut seen = false;
        let mut active = false;
        let mut inactive = false;
        let mut warnings: BTreeSet<_> = before.warnings.iter().cloned().collect();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Ok(id) = receiver.try_recv() {
                fixture_id = id;
            }
            if monitor.take_changed() {
                let value = monitor.snapshot()?;
                warnings.extend(value.warnings);
                for session in value
                    .sessions
                    .iter()
                    .filter(|s| s.instance_id == fixture_id && s.endpoint_id == endpoint_id)
                {
                    seen = true;
                    if session.state == AudioSessionStateActive.0 {
                        active = true;
                    }
                    if active && session.state == AudioSessionStateInactive.0 {
                        inactive = true;
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok((fixture_id, seen, active, inactive, warnings))
    })();
    // Always join the fixture, including monitor errors. No background stream is abandoned.
    worker
        .join()
        .map_err(|_| "silent fixture thread panicked")??;
    let (
        fixture_instance_id,
        observed_fixture,
        observed_active,
        observed_inactive_after_active,
        mut warnings,
    ) = observed?;
    let after = monitor.snapshot()?;
    warnings.extend(after.warnings.iter().cloned());
    let diagnostics = monitor.diagnostics();
    let session_created_callback_received =
        diagnostics.session_created_events > before_counts.session_created_events;
    let session_change_callback_received =
        diagnostics.session_change_events > before_counts.session_change_events;
    let default_roles_unchanged = defaults(&before) == defaults(&after);
    let endpoint_volumes_unchanged = volumes(&before) == volumes(&after);
    let passed = observed_fixture
        && observed_active
        && observed_inactive_after_active
        && session_created_callback_received
        && session_change_callback_received
        && default_roles_unchanged
        && endpoint_volumes_unchanged
        && diagnostics.missed_session_events == 0
        && warnings.is_empty();
    Ok(EventVerification {
        fixture_instance_id,
        observed_fixture,
        observed_active,
        observed_inactive_after_active,
        session_created_callback_received,
        session_change_callback_received,
        default_roles_unchanged,
        endpoint_volumes_unchanged,
        diagnostics,
        warnings: warnings.into_iter().collect(),
        passed,
    })
}
