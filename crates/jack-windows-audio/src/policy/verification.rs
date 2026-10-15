//! Host integration commands. Before/after snapshots + durable route journal.
use super::{PolicyCapabilities, WindowsAudioPolicy};
use crate::{AudioMonitor, MonitorDiagnostics, Snapshot};
use jack_core::routing::{
    AudioPolicy, FileJournal, Flow, JournalStore, RecoveryReport, Role, RouteController, RouteKey,
    RoutingError,
};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    thread,
    time::Duration,
};

struct SilentFixture(Option<thread::JoinHandle<Result<(), String>>>);
impl Drop for SilentFixture {
    fn drop(&mut self) {
        if let Some(worker) = self.0.take() {
            let _ = worker.join();
        }
    }
}
fn start_self_fixture(
    policy: &mut WindowsAudioPolicy,
) -> Result<SilentFixture, Box<dyn std::error::Error>> {
    let source = policy
        .read(&RouteKey::Default {
            flow: Flow::Render,
            role: Role::Multimedia,
        })?
        .ok_or("no render default for silent fixture")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let fixture = SilentFixture(Some(thread::spawn(move || {
        crate::fixture::silent_session(&source, sender).map_err(|e| e.to_string())
    })));
    receiver
        .recv_timeout(Duration::from_secs(2))
        .map_err(|e| format!("silent fixture initialization failed: {e}"))?;
    Ok(fixture)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct RouteState {
    pub key: RouteKey,
    pub endpoint: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct PolicyVerification {
    pub kind: &'static str,
    pub flow: Flow,
    pub target: String,
    pub capabilities: PolicyCapabilities,
    pub before: Vec<RouteState>,
    pub applied: Vec<RouteState>,
    pub after: Vec<RouteState>,
    pub changed: bool,
    pub readback_matched: bool,
    pub restored_match: bool,
    pub all_defaults_restored: bool,
    pub endpoint_volumes_unchanged: bool,
    pub operation_error: Option<String>,
    pub recovery: RecoveryReport,
    pub monitor: MonitorDiagnostics,
    pub warnings: Vec<String>,
    pub passed: bool,
}
fn lease(path: &Path) -> Result<File, RoutingError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path.with_extension("lock"))?)
}
fn read_keys(
    policy: &mut WindowsAudioPolicy,
    keys: &[RouteKey],
) -> Result<Vec<RouteState>, RoutingError> {
    keys.iter()
        .map(|key| {
            Ok(RouteState {
                key: key.clone(),
                endpoint: policy.read(key)?,
            })
        })
        .collect()
}
fn all_defaults() -> Vec<RouteKey> {
    [Flow::Render, Flow::Capture]
        .into_iter()
        .flat_map(|flow| {
            [Role::Console, Role::Multimedia, Role::Communications]
                .into_iter()
                .map(move |role| RouteKey::Default { flow, role })
        })
        .collect()
}
fn volume_state(snapshot: &Snapshot) -> Vec<(String, u32, bool)> {
    let mut values: Vec<_> = snapshot
        .endpoints
        .iter()
        .filter_map(|e| {
            e.volume
                .as_ref()
                .map(|v| (e.id.clone(), v.level.to_bits(), v.muted))
        })
        .collect();
    values.sort();
    values
}
fn verify(
    flow: Flow,
    target: &str,
    journal_path: &Path,
    self_app: bool,
) -> Result<PolicyVerification, Box<dyn std::error::Error>> {
    let _lease = lease(journal_path)?;
    let mut policy = WindowsAudioPolicy::new()?;
    policy.validate_endpoint(target, flow)?;
    let identity = if self_app {
        Some(policy.bind_process(std::process::id())?)
    } else {
        None
    };
    let _fixture = if self_app {
        Some(start_self_fixture(&mut policy)?)
    } else {
        None
    };
    let keys: Vec<_> = [Role::Console, Role::Multimedia, Role::Communications]
        .into_iter()
        .map(|role| match &identity {
            Some(id) => RouteKey::App {
                stable_app_id: id.clone(),
                flow,
                role,
            },
            None => RouteKey::Default { flow, role },
        })
        .collect();
    let store = FileJournal {
        path: journal_path.into(),
    };
    if !store.load()?.changes.is_empty() {
        return Err(
            "journal has pending recovery; run recover-policy before starting a new test".into(),
        );
    }
    let before = read_keys(&mut policy, &keys)?;
    let defaults_before = read_keys(&mut policy, &all_defaults())?;
    let capabilities = policy.capabilities();
    let mut monitor = AudioMonitor::new()?;
    let snapshot_before = monitor.snapshot()?;
    // Keep a complete readable backup of all roles and endpoint volumes before mutations.
    fs::write(
        journal_path.with_extension("before.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"defaults":defaults_before,"routes":before,"audio_snapshot":snapshot_before}),
        )?,
    )?;
    let changed = before.iter().any(|r| r.endpoint.as_deref() != Some(target));
    let mut controller = RouteController { policy, store };
    let mut applied = vec![];
    let operation = (|| -> Result<(), RoutingError> {
        controller.apply_batch(keys.iter().cloned().map(|k| (k, target.into())).collect())?;
        applied = read_keys(&mut controller.policy, &keys)?;
        thread::sleep(Duration::from_millis(250));
        Ok(())
    })();
    // Always attempt restoration, even when the apply/readback step fails.
    let recovery = controller.restore_best_effort()?;
    let after = read_keys(&mut controller.policy, &keys)?;
    let defaults_after = read_keys(&mut controller.policy, &all_defaults())?;
    thread::sleep(Duration::from_millis(100));
    let snapshot_after = monitor.snapshot()?;
    let readback_matched = applied.len() == keys.len()
        && applied
            .iter()
            .all(|r| r.endpoint.as_deref() == Some(target));
    let restored_match = before == after;
    let all_defaults_restored = defaults_before == defaults_after;
    let endpoint_volumes_unchanged =
        volume_state(&snapshot_before) == volume_state(&snapshot_after);
    let mut warnings = snapshot_before.warnings;
    warnings.extend(snapshot_after.warnings);
    warnings.sort();
    warnings.dedup();
    let operation_error = operation.err().map(|e| e.to_string());
    let monitor = monitor.diagnostics();
    let passed = operation_error.is_none()
        && readback_matched
        && restored_match
        && all_defaults_restored
        && endpoint_volumes_unchanged
        && recovery.complete
        && warnings.is_empty();
    Ok(PolicyVerification {
        kind: if self_app {
            "self_app"
        } else {
            "system_default"
        },
        flow,
        target: target.into(),
        capabilities,
        before,
        applied,
        after,
        changed,
        readback_matched,
        restored_match,
        all_defaults_restored,
        endpoint_volumes_unchanged,
        operation_error,
        recovery,
        monitor,
        warnings,
        passed,
    })
}
pub fn verify_default_roundtrip(
    flow: Flow,
    target: &str,
    journal_path: &Path,
) -> Result<PolicyVerification, Box<dyn std::error::Error>> {
    verify(flow, target, journal_path, false)
}
pub fn verify_self_app_roundtrip(
    flow: Flow,
    target: &str,
    journal_path: &Path,
) -> Result<PolicyVerification, Box<dyn std::error::Error>> {
    verify(flow, target, journal_path, true)
}
pub fn recover_policy_journal(path: &Path) -> Result<RecoveryReport, Box<dyn std::error::Error>> {
    let _lease = lease(path)?;
    let mut policy = WindowsAudioPolicy::new()?;
    let identity = policy.bind_process(std::process::id())?;
    let store = FileJournal { path: path.into() };
    let own_app_pending =
        store.load()?.changes.iter().any(
            |c| matches!(&c.key,RouteKey::App {stable_app_id,..} if stable_app_id == &identity),
        );
    let _fixture = if own_app_pending {
        Some(start_self_fixture(&mut policy)?)
    } else {
        None
    };
    let mut controller = RouteController { policy, store };
    Ok(controller.restore_best_effort()?)
}
