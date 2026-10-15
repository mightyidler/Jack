//! Durable routing protocol. Windows policy bindings live in jack-windows-audio.
//! Every external write is preceded by a synced journal record.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs, io, path::PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    Render,
    Capture,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Console,
    Multimedia,
    Communications,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteKey {
    Default {
        flow: Flow,
        role: Role,
    },
    App {
        stable_app_id: String,
        flow: Flow,
        role: Role,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    Applied,
    Restoring,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    pub key: RouteKey,
    pub previous: Option<String>,
    /// Value immediately before this attempt, including an earlier Jack-owned route.
    pub expected: Option<String>,
    pub applied: String,
    pub phase: Phase,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub schema_version: u32,
    pub changes: Vec<Change>,
}
impl Default for Journal {
    fn default() -> Self {
        Self {
            schema_version: 1,
            changes: vec![],
        }
    }
}
impl Journal {
    fn validate(&self) -> Result<(), RoutingError> {
        if self.schema_version != 1 {
            return Err(RoutingError::Schema(self.schema_version));
        }
        let mut keys = BTreeSet::new();
        for change in &self.changes {
            if change.applied.trim().is_empty()
                || change
                    .previous
                    .as_ref()
                    .is_some_and(|p| p.trim().is_empty())
                || change
                    .expected
                    .as_ref()
                    .is_some_and(|p| p.trim().is_empty())
                || !keys.insert(&change.key)
                || matches!(&change.key, RouteKey::App{stable_app_id, ..} if stable_app_id.trim().is_empty())
            {
                return Err(RoutingError::InvalidJournal);
            }
        }
        Ok(())
    }
}
#[derive(Debug, thiserror::Error)]
pub enum RoutingError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("unsupported routing journal schema {0}")]
    Schema(u32),
    #[error("invalid routing journal or request")]
    InvalidJournal,
    #[error("policy adapter: {0}")]
    Policy(String),
    #[error("policy changed externally; reconcile before applying again")]
    Conflict,
    #[error("policy readback does not match the requested route")]
    Readback,
    #[error("pending restoration must finish before a new route is applied")]
    RecoveryNeeded,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Applied,
    Conflict,
}
pub trait AudioPolicy {
    fn read(&mut self, key: &RouteKey) -> Result<Option<String>, RoutingError>;
    /// Adapter must recheck expected immediately before write; return Conflict on drift.
    /// Windows may lack an atomic CAS API: that residual race must be documented/tested.
    fn compare_and_set(
        &mut self,
        key: &RouteKey,
        expected: Option<&str>,
        desired: Option<&str>,
    ) -> Result<WriteOutcome, RoutingError>;
}
pub trait JournalStore {
    fn load(&self) -> Result<Journal, RoutingError>;
    fn save(&mut self, journal: &Journal) -> Result<(), RoutingError>;
}
pub struct FileJournal {
    pub path: PathBuf,
}
impl JournalStore for FileJournal {
    fn load(&self) -> Result<Journal, RoutingError> {
        let journal = match fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Journal::default(),
            Err(e) => return Err(e.into()),
        };
        journal.validate()?;
        Ok(journal)
    }
    fn save(&mut self, journal: &Journal) -> Result<(), RoutingError> {
        journal.validate()?;
        // Preserve corrupt/future state rather than silently replacing it.
        self.load()?;
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        crate::settings::write_atomic(&self.path, &serde_json::to_vec_pretty(journal)?)?;
        Ok(())
    }
}
#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreDisposition {
    Restored,
    NotApplied,
    AlreadyRestored,
    ExternalChangePreserved,
}
#[derive(Debug, Serialize)]
pub struct RestoredRoute {
    pub key: RouteKey,
    pub disposition: RestoreDisposition,
}
#[derive(Debug, Serialize)]
pub struct PendingRoute {
    pub key: RouteKey,
    pub error: String,
}
#[derive(Debug, Serialize)]
pub struct RecoveryReport {
    pub restored: Vec<RestoredRoute>,
    pub pending: Vec<PendingRoute>,
    pub complete: bool,
}

/// Single writer required. Recovery can be retried after any I/O or policy error.
pub struct RouteController<P, S> {
    pub policy: P,
    pub store: S,
}
impl<P: AudioPolicy, S: JournalStore> RouteController<P, S> {
    /// Journal a write performed by an external backend (e.g. Sonar) before
    /// calling it. Readback still uses the independently readable Windows policy.
    pub fn prepare_external_batch(
        &mut self,
        requests: Vec<(RouteKey, String)>,
    ) -> Result<(), RoutingError> {
        let mut journal = self.store.load()?;
        journal.validate()?;
        let mut seen = BTreeSet::new();
        for (key, endpoint) in requests {
            if endpoint.is_empty() || !seen.insert(key.clone()) {
                return Err(RoutingError::InvalidJournal);
            }
            let current = self.policy.read(&key)?;
            let existing = journal.changes.iter().position(|c| c.key == key);
            let previous = if let Some(index) = existing {
                let old = &journal.changes[index];
                if old.phase == Phase::Restoring {
                    return Err(RoutingError::RecoveryNeeded);
                }
                if current.as_deref() != Some(&old.applied) {
                    return Err(RoutingError::Conflict);
                }
                old.previous.clone()
            } else {
                current.clone()
            };
            if current.as_deref() == Some(&endpoint) {
                continue;
            }
            let change = Change {
                key,
                previous,
                expected: current,
                applied: endpoint,
                phase: Phase::Prepared,
            };
            if let Some(index) = existing {
                journal.changes[index] = change;
            } else {
                journal.changes.push(change);
            }
        }
        self.store.save(&journal)
    }
    pub fn confirm_external_batch(&mut self, keys: &[RouteKey]) -> Result<(), RoutingError> {
        let mut journal = self.store.load()?;
        journal.validate()?;
        let mut selected = false;
        let mut matched = false;
        for key in keys {
            if let Some(index) = journal.changes.iter().position(|c| c.key == *key) {
                selected = true;
                let current = self.policy.read(key)?;
                let change = &mut journal.changes[index];
                if current.as_deref() == Some(&change.applied) {
                    change.phase = Phase::Applied;
                    matched = true;
                } else if current == change.expected {
                    // Sonar changes Console/Multimedia but can leave Communications
                    // alone. Do not claim ownership of a role it did not write.
                    if current == change.previous {
                        journal.changes.remove(index);
                    } else {
                        change.applied = current.ok_or(RoutingError::Readback)?;
                        change.phase = Phase::Applied;
                    }
                } else {
                    return Err(RoutingError::Conflict);
                }
            }
        }
        if selected && !matched {
            return Err(RoutingError::Readback);
        }
        self.store.save(&journal)
    }
    /// Persist the entire plan BEFORE the first write. Later role writes may be
    /// redundant if Windows coupled another role to an earlier change.
    pub fn apply_batch(&mut self, requests: Vec<(RouteKey, String)>) -> Result<(), RoutingError> {
        let mut journal = self.store.load()?;
        journal.validate()?;
        let mut keys = BTreeSet::new();
        let mut planned = vec![];
        for (key, endpoint) in requests {
            if endpoint.trim().is_empty()
                || !keys.insert(key.clone())
                || matches!(&key, RouteKey::App{stable_app_id,..} if stable_app_id.trim().is_empty())
            {
                return Err(RoutingError::InvalidJournal);
            }
            let current = self.policy.read(&key)?;
            let existing = journal.changes.iter().position(|c| c.key == key);
            let previous = if let Some(index) = existing {
                let old = &journal.changes[index];
                if old.phase == Phase::Restoring {
                    return Err(RoutingError::RecoveryNeeded);
                }
                if current.as_deref() != Some(old.applied.as_str()) {
                    return Err(RoutingError::Conflict);
                }
                old.previous.clone()
            } else {
                current.clone()
            };
            if current.as_deref() == Some(endpoint.as_str()) {
                continue;
            }
            let change = Change {
                key: key.clone(),
                previous,
                expected: current.clone(),
                applied: endpoint.clone(),
                phase: Phase::Prepared,
            };
            match existing {
                Some(index) => journal.changes[index] = change,
                None => journal.changes.push(change),
            }
            planned.push((key, endpoint, current));
        }
        if planned.is_empty() {
            return Ok(());
        }
        self.store.save(&journal)?;
        for (key, endpoint, expected) in planned {
            // A preceding write in this prepared batch may already have moved this role.
            if self.policy.read(&key)?.as_deref() != Some(endpoint.as_str())
                && self
                    .policy
                    .compare_and_set(&key, expected.as_deref(), Some(&endpoint))?
                    == WriteOutcome::Conflict
            {
                return Err(RoutingError::Conflict);
            }
            if self.policy.read(&key)?.as_deref() != Some(endpoint.as_str()) {
                return Err(RoutingError::Readback);
            }
            journal
                .changes
                .iter_mut()
                .find(|c| c.key == key)
                .ok_or(RoutingError::InvalidJournal)?
                .phase = Phase::Applied;
            self.store.save(&journal)?;
        }
        Ok(())
    }
    pub fn apply(&mut self, key: RouteKey, endpoint: String) -> Result<(), RoutingError> {
        if endpoint.trim().is_empty()
            || matches!(&key, RouteKey::App{stable_app_id, ..} if stable_app_id.trim().is_empty())
        {
            return Err(RoutingError::InvalidJournal);
        }
        let mut journal = self.store.load()?;
        journal.validate()?;
        let current = self.policy.read(&key)?;
        let existing = journal.changes.iter().position(|c| c.key == key);
        let previous = if let Some(index) = existing {
            let owned = &journal.changes[index];
            if owned.phase == Phase::Restoring {
                return Err(RoutingError::RecoveryNeeded);
            }
            if current.as_deref() != Some(owned.applied.as_str()) {
                return Err(RoutingError::Conflict);
            }
            owned.previous.clone()
        } else {
            current.clone()
        };
        if current.as_deref() == Some(endpoint.as_str()) {
            return Ok(());
        }
        let change = Change {
            key: key.clone(),
            previous,
            expected: current.clone(),
            applied: endpoint.clone(),
            phase: Phase::Prepared,
        };
        let index = match existing {
            Some(index) => {
                journal.changes[index] = change;
                index
            }
            None => {
                journal.changes.push(change);
                journal.changes.len() - 1
            }
        };
        self.store.save(&journal)?;
        if self
            .policy
            .compare_and_set(&key, current.as_deref(), Some(&endpoint))?
            == WriteOutcome::Conflict
        {
            return Err(RoutingError::Conflict);
        }
        if self.policy.read(&key)?.as_deref() != Some(endpoint.as_str()) {
            return Err(RoutingError::Readback);
        }
        journal.changes[index].phase = Phase::Applied;
        self.store.save(&journal)?;
        Ok(())
    }
    pub fn restore_all(&mut self) -> Result<Vec<RestoredRoute>, RoutingError> {
        let report = self.restore_best_effort()?;
        if !report.complete {
            return Err(RoutingError::Policy(format!(
                "{} routes remain pending recovery",
                report.pending.len()
            )));
        }
        Ok(report.restored)
    }
    pub fn restore_best_effort(&mut self) -> Result<RecoveryReport, RoutingError> {
        self.restore_matching(|_| true)
    }
    /// Scoped completion says only that selected keys recovered; other keys stay owned.
    pub fn restore_keys_best_effort(
        &mut self,
        keys: &BTreeSet<RouteKey>,
    ) -> Result<RecoveryReport, RoutingError> {
        self.restore_matching(|key| keys.contains(key))
    }
    fn restore_matching(
        &mut self,
        selected: impl Fn(&RouteKey) -> bool,
    ) -> Result<RecoveryReport, RoutingError> {
        let mut journal = self.store.load()?;
        journal.validate()?;
        let mut results = vec![];
        let mut pending = vec![];
        for change in journal.changes.clone() {
            if !selected(&change.key) {
                continue;
            }
            let index = journal
                .changes
                .iter()
                .position(|c| c.key == change.key)
                .ok_or(RoutingError::InvalidJournal)?;
            let attempt = (|| -> Result<RestoreDisposition, RoutingError> {
                let current = self.policy.read(&change.key)?;
                let disposition = if change.phase != Phase::Prepared && current == change.previous {
                    RestoreDisposition::AlreadyRestored
                } else if change.phase == Phase::Prepared && current == change.previous {
                    RestoreDisposition::NotApplied
                } else if current.as_deref() != Some(change.applied.as_str())
                    && !(change.phase != Phase::Applied
                        && current == change.expected
                        && change.expected != change.previous)
                {
                    RestoreDisposition::ExternalChangePreserved
                } else {
                    journal.changes[index].phase = Phase::Restoring;
                    self.store.save(&journal)?;
                    if self.policy.compare_and_set(
                        &change.key,
                        current.as_deref(),
                        change.previous.as_deref(),
                    )? == WriteOutcome::Conflict
                    {
                        return Err(RoutingError::Conflict);
                    }
                    if self.policy.read(&change.key)? != change.previous {
                        return Err(RoutingError::Readback);
                    }
                    RestoreDisposition::Restored
                };
                Ok(disposition)
            })();
            let disposition = match attempt {
                Ok(value) => value,
                Err(
                    error @ (RoutingError::Policy(_)
                    | RoutingError::Conflict
                    | RoutingError::Readback),
                ) => {
                    pending.push(PendingRoute {
                        key: change.key,
                        error: error.to_string(),
                    });
                    continue;
                }
                Err(error) => return Err(error),
            };
            journal.changes.remove(index);
            self.store.save(&journal)?;
            results.push(RestoredRoute {
                key: change.key,
                disposition,
            });
        }
        Ok(RecoveryReport {
            complete: pending.is_empty(),
            restored: results,
            pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[derive(Default)]
    struct Policy {
        routes: BTreeMap<RouteKey, String>,
        writes: usize,
        fail_before: bool,
        fail_after: bool,
        lie: bool,
        concurrent_change: bool,
        fail_key: Option<RouteKey>,
    }
    impl AudioPolicy for Policy {
        fn read(&mut self, key: &RouteKey) -> Result<Option<String>, RoutingError> {
            Ok(self.routes.get(key).cloned())
        }
        fn compare_and_set(
            &mut self,
            key: &RouteKey,
            expected: Option<&str>,
            desired: Option<&str>,
        ) -> Result<WriteOutcome, RoutingError> {
            if self.concurrent_change {
                self.routes.insert(key.clone(), "external".into());
                self.concurrent_change = false;
            }
            if self.routes.get(key).map(String::as_str) != expected {
                return Ok(WriteOutcome::Conflict);
            }
            self.writes += 1;
            if self.fail_key.as_ref() == Some(key) {
                return Err(RoutingError::Policy("selected route failed".into()));
            }
            if self.fail_before {
                return Err(RoutingError::Policy("before write".into()));
            }
            if !self.lie {
                match desired {
                    Some(id) => {
                        self.routes.insert(key.clone(), id.into());
                    }
                    None => {
                        self.routes.remove(key);
                    }
                }
            }
            if self.fail_after {
                return Err(RoutingError::Policy("after write".into()));
            }
            Ok(WriteOutcome::Applied)
        }
    }
    fn key() -> RouteKey {
        RouteKey::App {
            stable_app_id: "path:c:/discord/discord.exe".into(),
            flow: Flow::Render,
            role: Role::Multimedia,
        }
    }
    fn controller(dir: &tempfile::TempDir) -> RouteController<Policy, FileJournal> {
        RouteController {
            policy: Policy::default(),
            store: FileJournal {
                path: dir.path().join("routing.json"),
            },
        }
    }
    #[test]
    fn retarget_preserves_original_override_and_restore_clears_owned_journal() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.policy.routes.insert(key(), "headphones".into());
        c.apply(key(), "jack-chat".into()).unwrap();
        c.apply(key(), "jack-media".into()).unwrap();
        assert_eq!(
            c.store.load().unwrap().changes[0].previous.as_deref(),
            Some("headphones")
        );
        assert_eq!(
            c.restore_all().unwrap()[0].disposition,
            RestoreDisposition::Restored
        );
        assert_eq!(c.policy.routes[&key()], "headphones");
        assert!(c.store.load().unwrap().changes.is_empty());
    }
    #[test]
    fn external_backend_write_is_prepared_before_mutation_and_recovered_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.policy.routes.insert(key(), "original".into());
        c.prepare_external_batch(vec![(key(), "sonar-chat".into())])
            .unwrap();
        assert_eq!(c.policy.routes[&key()], "original");
        assert_eq!(c.policy.writes, 0);
        assert!(matches!(
            c.confirm_external_batch(&[key()]),
            Err(RoutingError::Readback)
        ));
        c.policy.routes.insert(key(), "sonar-chat".into());
        let mut restarted = RouteController {
            policy: c.policy,
            store: c.store,
        };
        restarted.restore_all().unwrap();
        assert_eq!(restarted.policy.routes[&key()], "original");
    }
    #[test]
    fn external_backend_owns_only_roles_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        let mut console = key();
        if let RouteKey::App { role, .. } = &mut console {
            *role = Role::Console;
        }
        let mut communications = key();
        if let RouteKey::App { role, .. } = &mut communications {
            *role = Role::Communications;
        }
        c.policy
            .routes
            .insert(communications.clone(), "user-chat-device".into());
        c.prepare_external_batch(vec![
            (console.clone(), "sonar".into()),
            (key(), "sonar".into()),
            (communications.clone(), "sonar".into()),
        ])
        .unwrap();
        c.policy.routes.insert(console.clone(), "sonar".into());
        c.policy.routes.insert(key(), "sonar".into());
        c.confirm_external_batch(&[console, key(), communications.clone()])
            .unwrap();
        assert_eq!(c.store.load().unwrap().changes.len(), 2);
        c.restore_all().unwrap();
        assert_eq!(c.policy.routes[&communications], "user-chat-device");
    }
    #[test]
    fn external_backend_reassignment_retains_original_and_preserves_external_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.prepare_external_batch(vec![(key(), "sonar-chat".into())])
            .unwrap();
        c.policy.routes.insert(key(), "sonar-chat".into());
        c.confirm_external_batch(&[key()]).unwrap();
        c.prepare_external_batch(vec![(key(), "sonar-media".into())])
            .unwrap();
        c.policy.routes.insert(key(), "sonar-media".into());
        c.confirm_external_batch(&[key()]).unwrap();
        assert_eq!(c.store.load().unwrap().changes[0].previous, None);
        c.policy.routes.insert(key(), "user-external".into());
        assert_eq!(
            c.restore_all().unwrap()[0].disposition,
            RestoreDisposition::ExternalChangePreserved
        );
        assert_eq!(c.policy.routes[&key()], "user-external");
    }
    #[test]
    fn uncertain_write_is_recovered_from_disk_after_controller_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.policy.fail_after = true;
        assert!(c.apply(key(), "jack-chat".into()).is_err());
        assert_eq!(c.store.load().unwrap().changes[0].phase, Phase::Prepared);
        let mut restarted = RouteController {
            policy: c.policy,
            store: FileJournal { path: c.store.path },
        };
        restarted.policy.fail_after = false;
        restarted.restore_all().unwrap();
        assert!(!restarted.policy.routes.contains_key(&key()));
    }
    #[test]
    fn failed_write_before_mutation_needs_no_policy_restore() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.policy.fail_before = true;
        assert!(c.apply(key(), "jack".into()).is_err());
        let writes = c.policy.writes;
        assert_eq!(
            c.restore_all().unwrap()[0].disposition,
            RestoreDisposition::NotApplied
        );
        assert_eq!(c.policy.writes, writes);
    }
    #[test]
    fn failed_retarget_still_restores_the_pre_jack_route() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.policy.routes.insert(key(), "headphones".into());
        c.apply(key(), "jack-chat".into()).unwrap();
        c.policy.fail_before = true;
        assert!(c.apply(key(), "jack-media".into()).is_err());
        assert_eq!(c.policy.routes[&key()], "jack-chat");
        c.policy.fail_before = false;
        assert_eq!(
            c.restore_all().unwrap()[0].disposition,
            RestoreDisposition::Restored
        );
        assert_eq!(c.policy.routes[&key()], "headphones");
    }
    #[test]
    fn user_change_and_read_write_conflicts_never_get_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.apply(key(), "jack".into()).unwrap();
        c.policy.routes.insert(key(), "external".into());
        let writes = c.policy.writes;
        assert_eq!(
            c.restore_all().unwrap()[0].disposition,
            RestoreDisposition::ExternalChangePreserved
        );
        assert_eq!(c.policy.writes, writes);
        c.policy.concurrent_change = true;
        c.policy.routes.insert(key(), "headphones".into());
        assert!(matches!(
            c.apply(key(), "jack".into()),
            Err(RoutingError::Conflict)
        ));
        assert_eq!(c.policy.routes[&key()], "external");
    }
    #[test]
    fn readback_prevents_fake_applied_status_and_recovery_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.policy.lie = true;
        assert!(matches!(
            c.apply(key(), "jack".into()),
            Err(RoutingError::Readback)
        ));
        assert_eq!(c.store.load().unwrap().changes[0].phase, Phase::Prepared);
        c.restore_all().unwrap();
        assert!(c.restore_all().unwrap().is_empty());
    }
    #[test]
    fn restore_crash_after_policy_write_can_be_reconciled_without_repeating_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.apply(key(), "jack".into()).unwrap();
        let mut journal = c.store.load().unwrap();
        journal.changes[0].phase = Phase::Restoring;
        c.store.save(&journal).unwrap();
        c.policy.routes.remove(&key());
        let writes = c.policy.writes;
        assert_eq!(
            c.restore_all().unwrap()[0].disposition,
            RestoreDisposition::AlreadyRestored
        );
        assert_eq!(c.policy.writes, writes);
    }
    #[test]
    fn unreadable_corrupt_or_future_journal_blocks_all_policy_writes() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        for contents in ["broken", r#"{"schema_version":99,"changes":[]}"#] {
            fs::write(&c.store.path, contents).unwrap();
            assert!(c.apply(key(), "jack".into()).is_err());
            assert_eq!(c.policy.writes, 0);
            assert_eq!(fs::read_to_string(&c.store.path).unwrap(), contents);
        }
        fs::remove_file(&c.store.path).unwrap();
        fs::create_dir(&c.store.path).unwrap();
        assert!(c.apply(key(), "jack".into()).is_err());
        assert_eq!(c.policy.writes, 0);
    }
    #[derive(Default)]
    struct FaultStore {
        journal: Journal,
        saves: usize,
        fail_on: usize,
    }
    impl JournalStore for FaultStore {
        fn load(&self) -> Result<Journal, RoutingError> {
            Ok(self.journal.clone())
        }
        fn save(&mut self, journal: &Journal) -> Result<(), RoutingError> {
            self.saves += 1;
            if self.saves == self.fail_on {
                return Err(io::Error::other("simulated disk failure").into());
            }
            self.journal = journal.clone();
            Ok(())
        }
    }
    #[test]
    fn journal_sync_failure_prevents_the_policy_write() {
        let mut c = RouteController {
            policy: Policy::default(),
            store: FaultStore {
                fail_on: 1,
                ..Default::default()
            },
        };
        assert!(matches!(
            c.apply(key(), "jack".into()),
            Err(RoutingError::Io(_))
        ));
        assert_eq!(c.policy.writes, 0);
        assert!(c.store.journal.changes.is_empty());
    }
    #[test]
    fn final_commit_failure_keeps_recoverable_prepared_record() {
        let mut c = RouteController {
            policy: Policy::default(),
            store: FaultStore {
                fail_on: 2,
                ..Default::default()
            },
        };
        assert!(matches!(
            c.apply(key(), "jack".into()),
            Err(RoutingError::Io(_))
        ));
        assert_eq!(c.policy.routes[&key()], "jack");
        assert_eq!(c.store.journal.changes[0].phase, Phase::Prepared);
        c.restore_all().unwrap();
        assert!(!c.policy.routes.contains_key(&key()));
        assert!(c.store.journal.changes.is_empty());
    }
    #[test]
    fn batch_plan_is_durable_before_a_partial_apply_and_all_roles_recover() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        let other = RouteKey::Default {
            flow: Flow::Render,
            role: Role::Console,
        };
        c.policy.fail_key = Some(other.clone());
        assert!(
            c.apply_batch(vec![(key(), "jack".into()), (other.clone(), "jack".into())])
                .is_err()
        );
        let journal = c.store.load().unwrap();
        assert_eq!(journal.changes.len(), 2);
        assert_eq!(journal.changes[0].phase, Phase::Applied);
        assert_eq!(journal.changes[1].phase, Phase::Prepared);
        c.policy.fail_key = None;
        c.restore_all().unwrap();
        assert!(c.policy.routes.is_empty());
        assert!(c.store.load().unwrap().changes.is_empty());
    }
    #[test]
    fn one_policy_failure_does_not_prevent_other_routes_from_restoring() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        let other = RouteKey::Default {
            flow: Flow::Render,
            role: Role::Console,
        };
        c.apply_batch(vec![(key(), "jack".into()), (other.clone(), "jack".into())])
            .unwrap();
        c.policy.fail_key = Some(key());
        let report = c.restore_best_effort().unwrap();
        assert!(!report.complete);
        assert_eq!(report.pending.len(), 1);
        assert_eq!(report.restored.len(), 1);
        assert!(c.policy.routes.contains_key(&key()));
        assert!(!c.policy.routes.contains_key(&other));
        assert_eq!(c.store.load().unwrap().changes.len(), 1);
        c.policy.fail_key = None;
        assert!(c.restore_best_effort().unwrap().complete);
    }
    #[test]
    fn role_already_back_at_original_is_not_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = controller(&dir);
        c.policy.routes.insert(key(), "original".into());
        c.apply(key(), "jack".into()).unwrap();
        // Models Windows restoring a coupled role when an earlier role was restored.
        c.policy.routes.insert(key(), "original".into());
        let writes = c.policy.writes;
        assert_eq!(
            c.restore_all().unwrap()[0].disposition,
            RestoreDisposition::AlreadyRestored
        );
        assert_eq!(c.policy.writes, writes);
    }
}
