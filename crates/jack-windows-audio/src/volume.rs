//! Physical endpoint volume for unified mode only. Group gain belongs in jack-engine.
use crate::{AudioMonitor, platform::Apartment};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Write,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::E_INVALIDARG,
        Media::Audio::{
            DEVICE_STATE_ACTIVE, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
            MMDeviceEnumerator,
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance},
    },
    core::{GUID, HSTRING, Result},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct VolumeState {
    pub level: f32,
    pub muted: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeRecoveryRecord {
    pub endpoint_id: String,
    pub before: VolumeState,
    pub expected: VolumeState,
}
#[derive(Debug, Serialize)]
pub struct VolumeRecoveryReport {
    pub endpoint_id: String,
    pub current: VolumeState,
    pub after: VolumeState,
    pub restored: bool,
    pub external_change_preserved: bool,
}
fn same_volume(a: &VolumeState, b: &VolumeState) -> bool {
    (a.level - b.level).abs() <= 0.0001 && a.muted == b.muted
}
impl VolumeRecoveryRecord {
    fn validate(&self) -> std::result::Result<(), &'static str> {
        if self.endpoint_id.trim().is_empty()
            || self.endpoint_id.contains('\0')
            || !self.before.level.is_finite()
            || !self.expected.level.is_finite()
            || !(0.0..=1.0).contains(&self.before.level)
            || !(0.0..=1.0).contains(&self.expected.level)
            || self.before.muted != self.expected.muted
        {
            return Err(
                "invalid volume recovery record; mute changes cannot be restored by a level-only operation",
            );
        }
        Ok(())
    }
}
/// Recovery only after the GUI releases the global owner lease. Never restore a
/// value that no longer matches our recorded applied level/mute. Windows has no CAS.
pub fn recover_volume(
    path: &Path,
) -> std::result::Result<VolumeRecoveryReport, Box<dyn std::error::Error>> {
    let record: VolumeRecoveryRecord = serde_json::from_slice(&std::fs::read(path)?)?;
    record.validate()?;
    let root = crate::control::current_user_root()?;
    let _lease = crate::control::ControlLease::acquire(&root.join("control.lock"))?;
    let control = EndpointVolumeControl::new(&record.endpoint_id)?;
    let current = control.state()?;
    let already_restored = same_volume(&current, &record.before);
    let external_change_preserved = !already_restored && !same_volume(&current, &record.expected);
    if !already_restored && !external_change_preserved {
        control.set_level(record.before.level)?;
    }
    let after = control.state()?;
    let restored = same_volume(&after, &record.before);
    Ok(VolumeRecoveryReport {
        endpoint_id: record.endpoint_id,
        current,
        after,
        restored,
        external_change_preserved,
    })
}
pub struct EndpointVolumeControl {
    volume: IAudioEndpointVolume,
    _apartment: Apartment,
}
impl EndpointVolumeControl {
    pub fn new(endpoint: &str) -> Result<Self> {
        let apartment = Apartment::new()?;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        let device = unsafe { enumerator.GetDevice(&HSTRING::from(endpoint))? };
        if unsafe { device.GetState()? } != DEVICE_STATE_ACTIVE {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        }
        let volume = unsafe { device.Activate(CLSCTX_ALL, None)? };
        Ok(Self {
            volume,
            _apartment: apartment,
        })
    }
    pub fn state(&self) -> Result<VolumeState> {
        unsafe {
            Ok(VolumeState {
                level: self.volume.GetMasterVolumeLevelScalar()?,
                muted: self.volume.GetMute()?.as_bool(),
            })
        }
    }
    pub fn set_level(&self, level: f32) -> Result<()> {
        if !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        }
        const CONTEXT: GUID = GUID::from_u128(0xcf046bc8_3b9a_4d48_8a4f_31f8ba6c956e);
        unsafe { self.volume.SetMasterVolumeLevelScalar(level, &CONTEXT) }
    }
}
#[derive(Debug, Serialize)]
pub struct VolumeVerification {
    pub endpoint_id: String,
    pub before: VolumeState,
    pub requested: f32,
    pub applied: VolumeState,
    pub after: VolumeState,
    pub changed: bool,
    pub callbacks_received: u64,
    pub restored: bool,
    pub external_change_preserved: bool,
    pub operation_error: Option<String>,
    pub passed: bool,
}
pub fn verify_volume_roundtrip(
    endpoint: &str,
    backup: &Path,
) -> std::result::Result<VolumeVerification, Box<dyn std::error::Error>> {
    let control = EndpointVolumeControl::new(endpoint)?;
    let before = control.state()?;
    let requested = if before.level >= 0.04 {
        before.level - 0.02
    } else {
        before.level + 0.02
    };
    if let Some(parent) = backup.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = File::create(backup)?;
    file.write_all(&serde_json::to_vec_pretty(
        &serde_json::json!({"endpoint_id":endpoint,"before":before,"requested":requested}),
    )?)?;
    file.sync_all()?;
    drop(file);
    let mut monitor = AudioMonitor::new()?;
    let _ = monitor.snapshot()?;
    let counter = monitor.diagnostics().endpoint_volume_events;
    let operation_error = control.set_level(requested).err().map(|e| e.to_string());
    let applied = control.state()?;
    let deadline = Instant::now() + Duration::from_millis(300);
    while Instant::now() < deadline && monitor.diagnostics().endpoint_volume_events == counter {
        thread::sleep(Duration::from_millis(10));
    }
    let current = control.state()?;
    // Preserve an external value if it changed since our applied readback.
    let external_change_preserved =
        (current.level - applied.level).abs() > 0.0001 || current.muted != applied.muted;
    // A failed setter or mismatched readback does not establish ownership.
    let owns_applied = operation_error.is_none()
        && (applied.level - requested).abs() <= 0.0001
        && applied.muted == before.muted;
    if owns_applied && !external_change_preserved {
        control.set_level(before.level)?;
    }
    thread::sleep(Duration::from_millis(50));
    let after = control.state()?;
    let callbacks_received = monitor.diagnostics().endpoint_volume_events - counter;
    let restored = (before.level - after.level).abs() <= 0.0001 && before.muted == after.muted;
    let changed = (before.level - applied.level).abs() > 0.0001;
    let passed = operation_error.is_none() && changed && callbacks_received > 0 && restored;
    Ok(VolumeVerification {
        endpoint_id: endpoint.into(),
        before,
        requested,
        applied,
        after,
        changed,
        callbacks_received,
        restored,
        external_change_preserved,
        operation_error,
        passed,
    })
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    fn record() -> VolumeRecoveryRecord {
        VolumeRecoveryRecord {
            endpoint_id: "device".into(),
            before: VolumeState {
                level: 0.21,
                muted: false,
            },
            expected: VolumeState {
                level: 0.23,
                muted: false,
            },
        }
    }
    #[test]
    fn recovery_rejects_invalid_level_and_mute_ownership() {
        let mut r = record();
        assert!(r.validate().is_ok());
        r.before.level = f32::NAN;
        assert!(r.validate().is_err());
        r.before.level = 0.21;
        r.expected.level = 1.1;
        assert!(r.validate().is_err());
        r.expected.level = 0.23;
        r.expected.muted = true;
        assert!(r.validate().is_err());
    }
    #[test]
    fn externally_changed_level_or_mute_does_not_match_ownership() {
        let r = record();
        assert!(!same_volume(
            &r.expected,
            &VolumeState {
                level: 0.4,
                muted: false
            }
        ));
        assert!(!same_volume(
            &r.expected,
            &VolumeState {
                level: 0.23,
                muted: true
            }
        ));
    }
}
