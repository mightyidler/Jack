//! Persistent app assignments plus explicit undo of Sonar's Windows route writes.
//! The engine/channel state stays in Sonar; only app rules are Jack preferences.
use jack_core::{AppIdentity, Classification, OutputChannel, Settings, classify, routing::*};
use jack_sonar::{Command, Service, State, sonar::models::Channel};
use jack_windows_audio::{
    Snapshot,
    identity::{process_executable_path, process_identity},
    policy::WindowsAudioPolicy,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub type Routes = RouteController<WindowsAudioPolicy, FileJournal>;
#[derive(Debug, Clone, Serialize)]
pub struct AppView {
    pub process_id: u32,
    pub stable_id: Option<String>,
    pub requested: Option<Channel>,
    pub origin: String,
    pub pending: bool,
    pub message: Option<String>,
    pub icon_data_url: Option<String>,
}

#[cfg(test)]
mod group_recovery_tests {
    use super::*;
    #[test]
    #[ignore = "Live user-designated AMPLibraryAgent route/readback/restore; requires Jack to be closed"]
    fn live_inactive_amplibrary_route_and_pending_display() {
        let proof = std::path::PathBuf::from(
            std::env::var("JACK_APP_ROUTE_PROOF").expect("explicit proof path required"),
        );
        let root = jack_windows_audio::control::current_user_root().unwrap();
        let _lease =
            jack_windows_audio::control::ControlLease::acquire(&root.join("control.lock")).unwrap();
        let mut routes = Routes {
            policy: WindowsAudioPolicy::new().unwrap(),
            store: FileJournal {
                path: proof.with_extension("journal.json"),
            },
        };
        assert!(routes.restore_best_effort().unwrap().complete);
        let mut monitor = jack_windows_audio::AudioMonitor::new().unwrap();
        let snapshot = monitor.snapshot().unwrap();
        let mut service = Service::new().unwrap();
        let initial = service.refresh(false);
        let app = initial
            .applications
            .iter()
            .find(|a| {
                a.process_name == "AMPLibraryAgent"
                    && a.activity == jack_sonar::sonar::models::ApplicationActivity::Inactive
            })
            .expect("user-designated inactive app must still be present");
        let pid = app.process_id;
        let binding = routes.policy.bind_process(pid).unwrap();
        let keys = app_keys(&binding);
        let previous = keys
            .iter()
            .map(|k| routes.policy.read(k))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let mut routing = AppRouting::default();
        let identity = routing.identity(pid).unwrap();
        let request = routing.apply(
            &mut routes,
            &mut service,
            &initial,
            pid,
            &app.process_name,
            Channel::Chat,
        );
        let applied = keys
            .iter()
            .map(|k| routes.policy.read(k))
            .collect::<Result<Vec<_>, _>>();
        let mut settings = Settings {
            channel_separation_enabled: true,
            ..Settings::default()
        };
        settings
            .sonar_app_overrides
            .insert(identity.stable_id, OutputChannel::Chat);
        if request.is_ok() {
            routing.reconcile(&mut routes, &mut service, &settings, &snapshot, &initial);
        }
        let display = routing.apps.iter().find(|a| a.process_id == pid);
        let chat = initial
            .sources
            .iter()
            .find(|s| s.channel == Channel::Chat)
            .unwrap();
        let confirmed = applied.as_ref().is_ok_and(|v| {
            v[0].as_deref() == Some(&chat.endpoint_id)
                && v[1].as_deref() == Some(&chat.endpoint_id)
                && v[2] == previous[2]
        });
        let pending = display.is_some_and(|a| a.pending && a.requested == Some(Channel::Chat));
        let restore = routes.restore_keys_best_effort(&keys.into_iter().collect());
        let restored = app_keys(&binding)
            .iter()
            .map(|k| routes.policy.read(k))
            .collect::<Result<Vec<_>, _>>();
        let passed = request.is_ok()
            && confirmed
            && pending
            && restore.as_ref().is_ok_and(|r| r.complete)
            && restored.as_ref().is_ok_and(|v| *v == previous);
        let report = serde_json::json!({"passed":passed,"scope":"Actual inactive AMPLibraryAgent Windows policy + production pending AppView + exact restoration; does not prove native GUI drag or future playback/GG display","process_id":pid,"original_session":app,"before_policy":previous,"after_policy":applied.ok(),"confirmed_chat_policy":confirmed,"pending_chat_display":pending,"request_error":request.err(),"restore_error":restore.err().map(|e|e.to_string()),"restored_policy":restored.ok()});
        std::fs::write(proof, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        assert!(passed, "{report}");
    }
    #[derive(Default)]
    struct Policy {
        values: BTreeMap<RouteKey, Option<String>>,
        writes: Vec<RouteKey>,
        reject: bool,
    }
    impl AudioPolicy for Policy {
        fn read(&mut self, key: &RouteKey) -> Result<Option<String>, RoutingError> {
            self.values
                .get(key)
                .cloned()
                .ok_or_else(|| RoutingError::Policy("app exited".into()))
        }
        fn compare_and_set(
            &mut self,
            key: &RouteKey,
            expected: Option<&str>,
            desired: Option<&str>,
        ) -> Result<WriteOutcome, RoutingError> {
            if self.read(key)?.as_deref() != expected {
                return Err(RoutingError::Conflict);
            }
            if self.reject {
                return Err(RoutingError::Policy("write rejected".into()));
            }
            self.writes.push(key.clone());
            self.values.insert(key.clone(), desired.map(str::to_owned));
            Ok(WriteOutcome::Applied)
        }
    }
    #[derive(Default)]
    struct Store(Journal);
    impl JournalStore for Store {
        fn load(&self) -> Result<Journal, RoutingError> {
            Ok(self.0.clone())
        }
        fn save(&mut self, journal: &Journal) -> Result<(), RoutingError> {
            self.0 = journal.clone();
            Ok(())
        }
    }
    fn change(app: &str, endpoint: &str) -> Change {
        Change {
            key: app_keys(app)[0].clone(),
            previous: None,
            expected: None,
            applied: endpoint.into(),
            phase: Phase::Applied,
        }
    }
    #[test]
    fn deleting_one_group_preserves_other_groups_external_choices_and_exited_app_journal() {
        let owned = change("game", "game-old");
        let foreign = change("external", "game-current");
        let other = change("music", "media");
        let exited = change("exited", "game-old");
        let mut routes = RouteController {
            policy: Policy {
                values: BTreeMap::from([
                    (owned.key.clone(), Some(owned.applied.clone())),
                    (foreign.key.clone(), Some("physical-user-choice".into())),
                    (other.key.clone(), Some(other.applied.clone())),
                ]),
                ..Default::default()
            },
            store: Store(Journal {
                changes: vec![
                    owned.clone(),
                    foreign.clone(),
                    other.clone(),
                    exited.clone(),
                ],
                ..Default::default()
            }),
        };
        restore_source_routes(
            &mut routes,
            &BTreeSet::from(["game-old".into(), "game-current".into()]),
        )
        .unwrap();
        assert_eq!(routes.policy.values[&owned.key], None);
        assert_eq!(
            routes.policy.values[&foreign.key].as_deref(),
            Some("physical-user-choice")
        );
        assert_eq!(routes.policy.values[&other.key].as_deref(), Some("media"));
        assert_eq!(routes.policy.writes, vec![owned.key]);
        assert_eq!(
            routes
                .store
                .0
                .changes
                .iter()
                .map(|c| &c.key)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([&other.key, &exited.key])
        );
    }
    #[test]
    fn failed_group_restoration_remains_recoverable_and_prevents_commit() {
        let owned = change("game", "game");
        let mut routes = RouteController {
            policy: Policy {
                values: BTreeMap::from([(owned.key.clone(), Some("game".into()))]),
                reject: true,
                ..Default::default()
            },
            store: Store(Journal {
                changes: vec![owned.clone()],
                ..Default::default()
            }),
        };
        assert!(restore_source_routes(&mut routes, &BTreeSet::from(["game".into()])).is_err());
        assert_eq!(routes.policy.values[&owned.key].as_deref(), Some("game"));
        assert_eq!(routes.store.0.changes.len(), 1);
        routes.policy.reject = false;
        restore_source_routes(&mut routes, &BTreeSet::from(["game".into()])).unwrap();
        assert_eq!(routes.policy.values[&owned.key], None);
        assert!(routes.store.0.changes.is_empty());
    }
    #[test]
    fn disabled_source_can_restore_its_app_intent_without_cached_endpoint() {
        let owned = change("aux-app", "disabled-source-id");
        let other = change("game-app", "game-source-id");
        let mut routes = RouteController {
            policy: Policy {
                values: BTreeMap::from([
                    (owned.key.clone(), Some(owned.applied.clone())),
                    (other.key.clone(), Some(other.applied.clone())),
                ]),
                ..Default::default()
            },
            store: Store(Journal {
                changes: vec![owned.clone(), other.clone()],
                ..Default::default()
            }),
        };
        restore_group_routes(
            &mut routes,
            &BTreeSet::new(),
            &BTreeSet::from([owned.key.clone()]),
        )
        .unwrap();
        assert_eq!(routes.policy.values[&owned.key], None);
        assert_eq!(
            routes.policy.values[&other.key].as_deref(),
            Some("game-source-id")
        );
        assert_eq!(routes.store.0.changes.len(), 1);
        assert_eq!(routes.store.0.changes[0].key, other.key);
    }

    #[test]
    fn returning_app_recovers_without_restoring_other_active_owned_routes() {
        let exited = change("exited", "chat");
        let active = change("active", "media");
        let mut routes = RouteController {
            policy: Policy {
                values: BTreeMap::from([(active.key.clone(), Some(active.applied.clone()))]),
                ..Default::default()
            },
            store: Store(Journal {
                changes: vec![exited.clone(), active.clone()],
                ..Default::default()
            }),
        };
        request_restore(&mut routes, &BTreeSet::from([exited.key.clone()])).unwrap();
        let mut pending = BTreeSet::new();
        recover_pending_routes(&mut routes, &mut pending).unwrap();
        assert!(pending.contains(&exited.key));
        assert_eq!(routes.store.0.changes.len(), 2);
        // The process has rebound even though it has no current app rule.
        routes
            .policy
            .values
            .insert(exited.key.clone(), Some(exited.applied.clone()));
        recover_pending_routes(&mut routes, &mut pending).unwrap();
        assert!(pending.is_empty());
        assert_eq!(routes.policy.values[&exited.key], None);
        assert_eq!(routes.policy.values[&active.key], Some(active.applied));
        assert_eq!(routes.store.0.changes.len(), 1);
    }

    #[test]
    fn returning_app_preserves_external_policy_and_failed_recovery_retries() {
        let owned = change("returning", "chat");
        let mut routes = RouteController {
            policy: Policy {
                values: BTreeMap::from([(owned.key.clone(), Some(owned.applied.clone()))]),
                reject: true,
                ..Default::default()
            },
            store: Store(Journal {
                changes: vec![owned.clone()],
                ..Default::default()
            }),
        };
        request_restore(&mut routes, &BTreeSet::from([owned.key.clone()])).unwrap();
        let mut pending = BTreeSet::new();
        recover_pending_routes(&mut routes, &mut pending).unwrap();
        assert!(pending.contains(&owned.key));
        routes.policy.reject = false;
        routes
            .policy
            .values
            .insert(owned.key.clone(), Some("external".into()));
        recover_pending_routes(&mut routes, &mut pending).unwrap();
        assert!(pending.is_empty());
        assert!(routes.store.0.changes.is_empty());
        assert_eq!(
            routes.policy.values[&owned.key].as_deref(),
            Some("external")
        );
    }

    #[test]
    fn applied_route_survives_app_exit_rebind_and_controller_restart_without_writes() {
        let assigned = change("spotify", "media");
        let mut routes = RouteController {
            policy: Policy::default(), // App is closed: its policy cannot be read.
            store: Store(Journal {
                changes: vec![assigned.clone()],
                ..Default::default()
            }),
        };
        let mut stale_pending = BTreeSet::from([assigned.key.clone()]);
        recover_pending_routes(&mut routes, &mut stale_pending).unwrap();
        assert!(stale_pending.is_empty());
        assert!(recover_unfinished(&mut routes).unwrap().complete);
        assert_eq!(routes.store.0.changes[0].phase, Phase::Applied);

        // Reopen the persisted journal in a new controller, with the new PID bound.
        let saved = serde_json::to_vec(&routes.store.0).unwrap();
        let mut restarted = RouteController {
            policy: Policy {
                values: BTreeMap::from([(assigned.key.clone(), Some("media".into()))]),
                ..Default::default()
            },
            store: Store(serde_json::from_slice(&saved).unwrap()),
        };
        assert!(recover_unfinished(&mut restarted).unwrap().complete);
        recover_pending_routes(&mut restarted, &mut BTreeSet::new()).unwrap();
        assert_eq!(
            restarted.policy.values[&assigned.key].as_deref(),
            Some("media")
        );
        assert!(restarted.policy.writes.is_empty());
        assert_eq!(restarted.store.0.changes.len(), 1);
    }

    #[test]
    fn explicit_group_removal_while_app_closed_survives_restart_and_undoes_only_that_group() {
        let removed = change("closed-aux-app", "aux");
        let retained = change("spotify", "media");
        let mut routes = RouteController {
            policy: Policy::default(),
            store: Store(Journal {
                changes: vec![removed.clone(), retained.clone()],
                ..Default::default()
            }),
        };
        restore_source_routes(&mut routes, &BTreeSet::from(["aux".into()])).unwrap();
        assert_eq!(routes.store.0.changes[0].phase, Phase::Restoring);
        assert_eq!(routes.store.0.changes[1].phase, Phase::Applied);
        let saved = serde_json::to_vec(&routes.store.0).unwrap();
        routes.store = Store(serde_json::from_slice(&saved).unwrap());
        routes
            .policy
            .values
            .insert(removed.key.clone(), Some("aux".into()));
        routes
            .policy
            .values
            .insert(retained.key.clone(), Some("media".into()));
        assert!(recover_unfinished(&mut routes).unwrap().complete);
        assert_eq!(routes.policy.values[&removed.key], None);
        assert_eq!(
            routes.policy.values[&retained.key].as_deref(),
            Some("media")
        );
        assert_eq!(routes.policy.writes, vec![removed.key]);
    }

    #[test]
    fn external_choice_releases_all_roles_without_restoring_any_of_them() {
        let keys = app_keys("spotify");
        let mut routes = RouteController {
            policy: Policy {
                values: BTreeMap::from([
                    (keys[0].clone(), Some("chat".into())),
                    (keys[1].clone(), Some("media".into())),
                ]),
                ..Default::default()
            },
            store: Store(Journal {
                changes: keys[..2]
                    .iter()
                    .cloned()
                    .map(|key| Change {
                        key,
                        previous: None,
                        expected: None,
                        applied: "media".into(),
                        phase: Phase::Applied,
                    })
                    .collect(),
                ..Default::default()
            }),
        };
        release_assignments(&mut routes, &keys.into_iter().collect()).unwrap();
        assert!(routes.store.0.changes.is_empty());
        assert!(routes.policy.writes.is_empty());
        assert!(recover_unfinished(&mut routes).unwrap().complete);
        assert_eq!(
            routes.policy.values[&app_keys("spotify")[0]].as_deref(),
            Some("chat")
        );
        assert_eq!(
            routes.policy.values[&app_keys("spotify")[1]].as_deref(),
            Some("media")
        );
    }

    #[test]
    fn startup_recovers_uncertain_write_but_keeps_successful_assignments() {
        let mut failed = change("failed", "aux");
        failed.phase = Phase::Prepared;
        let assigned = change("spotify", "media");
        let mut routes = RouteController {
            policy: Policy {
                values: BTreeMap::from([
                    (failed.key.clone(), Some("aux".into())),
                    (assigned.key.clone(), Some("media".into())),
                ]),
                ..Default::default()
            },
            store: Store(Journal {
                changes: vec![failed.clone(), assigned.clone()],
                ..Default::default()
            }),
        };
        assert!(recover_unfinished(&mut routes).unwrap().complete);
        assert_eq!(routes.policy.values[&failed.key], None);
        assert_eq!(
            routes.policy.values[&assigned.key].as_deref(),
            Some("media")
        );
        assert_eq!(routes.policy.writes, vec![failed.key]);
    }

    #[test]
    fn live_identity_does_not_require_an_audio_session_and_revalidates_cache() {
        let pid = std::process::id();
        let mut routing = AppRouting::default();
        let identity = routing.identity(pid).unwrap();
        assert!(identity.executable_name.is_some());
        routing.identities.get_mut(&pid).unwrap().0 = "stale-session-path".into();
        assert_eq!(routing.identity(pid).unwrap().stable_id, identity.stable_id);
        assert_ne!(routing.identities[&pid].0, "stale-session-path");
    }

    #[test]
    fn inventory_resolves_unknown_label_and_removes_confirmed_exited_process() {
        use jack_sonar::sonar::models::{
            ApplicationActivity, ApplicationRoute, ApplicationSession,
        };
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/C", "exit", "0"])
            .creation_flags(0x08000000)
            .spawn()
            .unwrap();
        let exited = child.id();
        child.wait().unwrap();
        let session = |pid| ApplicationSession {
            process_id: pid,
            process_name: "Unknown".into(),
            display_name: String::new(),
            route: ApplicationRoute::Unassigned,
            activity: ApplicationActivity::Inactive,
            routing_error: false,
        };
        let mut state = State {
            applications: vec![session(std::process::id()), session(exited)],
            ..State::default()
        };
        AppRouting::default().refresh_inventory(&mut state);
        assert_eq!(state.applications.len(), 1);
        assert!(!state.applications[0].display_name.is_empty());
        assert_eq!(state.applications[0].process_name, "Unknown");
    }
}
#[derive(Default)]
pub struct AppRouting {
    identities: BTreeMap<u32, (String, AppIdentity)>,
    suppressed: BTreeSet<String>,
    retry_after: BTreeMap<String, Instant>,
    pub apps: Vec<AppView>,
    pub warnings: Vec<String>,
    // Persist these in settings so a manual Sonar/Windows choice survives Jack restart.
    pub external_choices: BTreeSet<String>,
    // Retained only for ownership recovery while GG is unavailable, never for writes.
    source_ids: BTreeMap<Channel, BTreeSet<String>>,
    recovery_keys: BTreeSet<RouteKey>,
}
pub fn output(channel: Channel) -> Result<OutputChannel, String> {
    Ok(match channel {
        Channel::Game => OutputChannel::Game,
        Channel::Chat => OutputChannel::Chat,
        Channel::Media => OutputChannel::Media,
        Channel::Aux => OutputChannel::Aux,
        Channel::Microphone => return Err("An app needs an output channel".into()),
    })
}
pub(crate) fn channel(output: OutputChannel) -> Channel {
    match output {
        OutputChannel::Game => Channel::Game,
        OutputChannel::Chat => Channel::Chat,
        OutputChannel::Media => Channel::Media,
        OutputChannel::Aux => Channel::Aux,
    }
}
fn group_channel(id: uuid::Uuid, settings: &Settings) -> Option<Channel> {
    settings
        .groups
        .iter()
        .find(|group| group.id == id)
        .and_then(jack_core::AudioGroup::output_channel)
        .map(channel)
}
pub fn app_keys(identity: &str) -> [RouteKey; 3] {
    [Role::Console, Role::Multimedia, Role::Communications].map(|role| RouteKey::App {
        stable_app_id: identity.into(),
        flow: Flow::Render,
        role,
    })
}

/// Applied entries are durable assignments, not unfinished transactions. A missing
/// process only prevents readback; it must never turn an assignment into an undo.
pub fn recover_unfinished<P: AudioPolicy, S: JournalStore>(
    routes: &mut RouteController<P, S>,
) -> Result<RecoveryReport, RoutingError> {
    let keys = routes
        .store
        .load()?
        .changes
        .into_iter()
        .filter(|change| change.phase != Phase::Applied)
        .map(|change| change.key)
        .collect();
    routes.restore_keys_best_effort(&keys)
}

/// Record explicit undo before attempting it, including apps that are closed.
/// Prepared entries retain their phase so uncertain partial writes recover safely.
fn request_restore<P: AudioPolicy, S: JournalStore>(
    routes: &mut RouteController<P, S>,
    keys: &BTreeSet<RouteKey>,
) -> Result<(), RoutingError> {
    let mut journal = routes.store.load()?;
    let mut changed = false;
    for change in &mut journal.changes {
        if keys.contains(&change.key) && change.phase == Phase::Applied {
            change.phase = Phase::Restoring;
            changed = true;
        }
    }
    if changed {
        routes.store.save(&journal)?;
    }
    Ok(())
}
#[cfg(test)]
fn restore_source_routes<P: AudioPolicy, S: JournalStore>(
    routes: &mut RouteController<P, S>,
    sources: &BTreeSet<String>,
) -> Result<(), String> {
    restore_group_routes(routes, sources, &BTreeSet::new())
}
fn restore_group_routes<P: AudioPolicy, S: JournalStore>(
    routes: &mut RouteController<P, S>,
    sources: &BTreeSet<String>,
    app_keys: &BTreeSet<RouteKey>,
) -> Result<(), String> {
    let journal = routes.store.load().map_err(|e| e.to_string())?;
    let selected = journal
        .changes
        .iter()
        .filter(|change| sources.contains(&change.applied) || app_keys.contains(&change.key))
        .map(|change| change.key.clone())
        .collect::<BTreeSet<_>>();
    request_restore(routes, &selected).map_err(|e| e.to_string())?;
    let keys = selected
        .into_iter()
        .filter(|key| routes.policy.read(key).is_ok())
        .collect();
    let report = routes
        .restore_keys_best_effort(&keys)
        .map_err(|e| e.to_string())?;
    if !report.complete {
        return Err("Group app policy recovery pending; group retained".into());
    }
    Ok(())
}
impl AppRouting {
    /// Sonar can retain dead sessions and omit process labels. Keep inaccessible
    /// live apps visible, but discard only processes Windows confirms have exited.
    pub fn refresh_inventory(&mut self, sonar: &mut State) {
        sonar
            .applications
            .retain_mut(|app| match process_executable_path(app.process_id) {
                Ok(path) => {
                    if let Some(apple) = crate::apple_apps::from_executable(&path) {
                        app.display_name = apple.display_name.into();
                    } else if (app.process_name.trim().is_empty()
                        || app.process_name.eq_ignore_ascii_case("unknown"))
                        && (app.display_name.trim().is_empty()
                            || app.display_name.eq_ignore_ascii_case("unknown"))
                        && let Some(name) = std::path::Path::new(&path).file_stem()
                    {
                        app.display_name = name.to_string_lossy().into_owned();
                    }
                    true
                }
                Err(error) => !matches!(error.code(), code if
                    code == windows::Win32::Foundation::ERROR_INVALID_PARAMETER.to_hresult()
                    || code == windows::Win32::Foundation::ERROR_PROCESS_ABORTED.to_hresult()),
            });
    }

    pub fn remember_recovery(&mut self, report: &RecoveryReport) {
        self.recovery_keys
            .extend(report.pending.iter().map(|pending| pending.key.clone()));
    }

    pub fn identity(&mut self, pid: u32) -> Result<AppIdentity, String> {
        // Session caches can lag process start/exit and PID reuse. Always check
        // the live executable before reusing the expensive publisher identity.
        let path = process_executable_path(pid).map_err(|error| {
            format!("Cannot verify the executable of this audio session: {error}")
        })?;
        if let Some((cached, identity)) = self.identities.get(&pid)
            && cached == &path
        {
            return Ok(identity.clone());
        }
        let identity = process_identity(pid).map_err(|e| e.to_string())?;
        self.identities.insert(pid, (path, identity.clone()));
        Ok(identity)
    }
    pub fn allow(&mut self, stable: &str) {
        self.suppressed.remove(stable);
        self.retry_after.remove(stable);
    }
    /// Call only after the external choices have been saved in preferences. Keep
    /// the journal until then so a settings-write failure cannot lose the edit.
    pub fn acknowledge_external(&mut self, routes: &mut Routes) -> Result<(), String> {
        for app in &self.apps {
            if app
                .stable_id
                .as_ref()
                .is_some_and(|id| self.external_choices.contains(id))
            {
                // The app can exit after the snapshot. The saved exclusion is
                // already durable; release its journal when it returns instead.
                let Ok(binding) = routes.policy.bind_process(app.process_id) else {
                    continue;
                };
                release_assignments(routes, &app_keys(&binding).into_iter().collect())
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    pub fn restore_live(&mut self, routes: &mut Routes, sonar: &State) -> Result<(), String> {
        let mut keys = BTreeSet::new();
        for app in &sonar.applications {
            if let Ok(identity) = routes.policy.bind_process(app.process_id) {
                keys.extend(app_keys(&identity));
            }
        }
        // GG can stop reporting sessions while the app process is still alive.
        // Existing verified process bindings remain independently readable.
        let changes = routes.store.load().map_err(|e| e.to_string())?.changes;
        request_restore(
            routes,
            &changes.iter().map(|change| change.key.clone()).collect(),
        )
        .map_err(|e| e.to_string())?;
        for change in changes {
            if routes.policy.read(&change.key).is_ok() {
                keys.insert(change.key);
            }
        }
        let report = routes
            .restore_keys_best_effort(&keys)
            .map_err(|e| e.to_string())?;
        self.remember_recovery(&report);
        if !report.complete {
            return Err(report
                .pending
                .iter()
                .map(|r| r.error.clone())
                .collect::<Vec<_>>()
                .join("; "));
        }
        // Exited apps keep durable records. Reconcile publishes pending recovery
        // in both ON and OFF views, without treating active owned routes as errors.
        Ok(())
    }
    pub fn restore_channel(
        &mut self,
        routes: &mut Routes,
        sonar: &State,
        target: Channel,
        settings: &Settings,
        snapshot: &Snapshot,
    ) -> Result<(), String> {
        for source in &sonar.sources {
            self.source_ids
                .entry(source.channel)
                .or_default()
                .insert(source.endpoint_id.clone());
        }
        let mut keys = BTreeSet::new();
        for pid in snapshot
            .sessions
            .iter()
            .map(|session| session.pid)
            .filter(|pid| *pid != 0)
            .collect::<BTreeSet<_>>()
        {
            let Ok(identity) = self.identity(pid) else {
                continue;
            };
            let requested = settings
                .sonar_app_overrides
                .get(&identity.stable_id)
                .copied()
                .map(channel)
                .or_else(|| match classify(&identity, settings) {
                    Classification::User(id) | Classification::BuiltIn(id) => {
                        group_channel(id, settings)
                    }
                    Classification::Unclassified => None,
                });
            if let Ok(binding) = routes.policy.bind_process(pid)
                && requested == Some(target)
            {
                keys.extend(app_keys(&binding));
            }
        }
        let sources = self.source_ids.get(&target).cloned().unwrap_or_default();
        self.recovery_keys.extend(
            routes
                .store
                .load()
                .map_err(|e| e.to_string())?
                .changes
                .iter()
                .filter(|change| sources.contains(&change.applied) || keys.contains(&change.key))
                .map(|change| change.key.clone()),
        );
        restore_group_routes(routes, &sources, &keys)?;
        // Exited apps remain journaled and recover on their next session or Jack startup.
        Ok(())
    }
    pub fn apply(
        &mut self,
        routes: &mut Routes,
        service: &mut Service,
        state: &State,
        pid: u32,
        name: &str,
        target: Channel,
    ) -> Result<(), String> {
        let source = state
            .sources
            .iter()
            .find(|s| s.channel == target)
            .ok_or("Sonar does not expose this virtual output")?;
        self.source_ids
            .entry(target)
            .or_default()
            .insert(source.endpoint_id.clone());
        let identity = routes.policy.bind_process(pid).map_err(|e| e.to_string())?;
        let keys = app_keys(&identity);
        // GG's application endpoint operates on active streams. Its own frontend
        // filters inactive sessions out; an inactive PUT can succeed without writing
        // any policy. A Windows-side edit can also leave GG's stream flagged as
        // unroutable. Persist the next-playback preference in either case, without
        // asking GG to force a broken/current stream to move.
        if state.applications.iter().any(|app| {
            app.process_id == pid
                && app.process_name == name
                && (app.activity == jack_sonar::sonar::models::ApplicationActivity::Inactive
                    || app.routing_error)
        }) {
            let selected = keys[..2].to_vec(); // Preserve Communications, as GG does.
            if let Err(error) = routes.apply_batch(
                selected
                    .iter()
                    .cloned()
                    .map(|key| (key, source.endpoint_id.clone()))
                    .collect(),
            ) {
                let recovery = routes
                    .restore_keys_best_effort(&selected.into_iter().collect())
                    .map_err(|e| e.to_string())?;
                return Err(format!(
                    "{error}{}",
                    if recovery.complete {
                        ""
                    } else {
                        "; app policy recovery pending"
                    }
                ));
            }
            return Ok(());
        }
        // Persist the undo record before invoking Sonar. Sonar may modify all roles.
        routes
            .prepare_external_batch(
                keys.iter()
                    .cloned()
                    .map(|k| (k, source.endpoint_id.clone()))
                    .collect(),
            )
            .map_err(|e| e.to_string())?;
        let requested = service.execute(Command::Application {
            process_id: pid,
            process_name: name.into(),
            channel: target,
        });
        let policy_confirmation = routes.confirm_external_batch(&keys);
        match (requested, policy_confirmation) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(jack_sonar::error::Error::ApplicationRouteVerificationFailed { .. }), Ok(())) => {
                Ok(())
            } // requested route confirmed; effective session is still pending
            (request, confirmation) => {
                let recovery = routes
                    .restore_keys_best_effort(&keys.into_iter().collect())
                    .map_err(|e| e.to_string())?;
                let mut message = request
                    .err()
                    .map(|e| e.to_string())
                    .or_else(|| confirmation.err().map(|e| e.to_string()))
                    .unwrap_or_else(|| "App routing failed".into());
                if !recovery.complete {
                    message.push_str("; app policy recovery pending");
                }
                Err(message)
            }
        }
    }
    pub fn reconcile(
        &mut self,
        routes: &mut Routes,
        service: &mut Service,
        settings: &Settings,
        snapshot: &Snapshot,
        sonar: &State,
    ) {
        self.apps.clear();
        self.warnings.clear();
        self.external_choices.clear();
        // Rebind independently of classification. A restarted app may no longer
        // have a rule or appear in GG, but its old undo record still needs recovery.
        let journal = match routes.store.load() {
            Ok(journal) => journal,
            Err(error) => {
                self.warnings
                    .push(format!("Cannot read app recovery journal: {error}"));
                return;
            }
        };
        self.recovery_keys.extend(
            journal
                .changes
                .iter()
                .filter(|change| change.phase != Phase::Applied)
                .map(|change| change.key.clone()),
        );
        if !journal.changes.is_empty() {
            for pid in snapshot
                .sessions
                .iter()
                .map(|session| session.pid)
                .chain(sonar.applications.iter().map(|app| app.process_id))
                .filter(|pid| *pid != 0)
                .collect::<BTreeSet<_>>()
            {
                let _ = routes.policy.bind_process(pid);
            }
        }
        if let Err(error) = recover_pending_routes(routes, &mut self.recovery_keys) {
            self.warnings.push(error);
        }
        for source in &sonar.sources {
            self.source_ids
                .entry(source.channel)
                .or_default()
                .insert(source.endpoint_id.clone());
        }
        let live: BTreeSet<_> = sonar.applications.iter().map(|a| a.process_id).collect();
        self.identities.retain(|pid, _| live.contains(pid));
        if !settings.channel_separation_enabled
            && let Err(e) = self.restore_live(routes, sonar)
        {
            self.warnings.push(e);
        }
        for app in &sonar.applications {
            let mut view = AppView {
                process_id: app.process_id,
                stable_id: None,
                requested: None,
                origin: "default".into(),
                pending: false,
                message: None,
                icon_data_url: None,
            };
            let identity = match self.identity(app.process_id) {
                Ok(identity) => identity,
                Err(e) => {
                    view.message = Some(e);
                    self.apps.push(view);
                    continue;
                }
            };
            view.stable_id = Some(identity.stable_id.clone());
            let target = settings
                .sonar_app_overrides
                .get(&identity.stable_id)
                .copied()
                .map(channel)
                .filter(|target| {
                    settings
                        .groups
                        .iter()
                        .any(|group| group.output_channel().map(channel) == Some(*target))
                })
                .map(|c| (c, "user"))
                .or_else(|| match classify(&identity, settings) {
                    Classification::User(id) => group_channel(id, settings).map(|c| (c, "user")),
                    Classification::BuiltIn(id) => {
                        group_channel(id, settings).map(|c| (c, "automatic"))
                    }
                    Classification::Unclassified => None,
                });
            let Some((target, origin)) =
                target.filter(|_| !settings.sonar_auto_exclusions.contains(&identity.stable_id))
            else {
                // Missing classification is not an explicit request to undo a
                // successful assignment (publisher/session discovery can lag).
                if settings.sonar_auto_exclusions.contains(&identity.stable_id) {
                    self.external_choices.insert(identity.stable_id.clone());
                }
                view.origin = if settings.sonar_auto_exclusions.contains(&identity.stable_id) {
                    "original"
                } else {
                    "automatic"
                }
                .into();
                self.apps.push(view);
                continue;
            };
            view.requested = Some(target);
            view.origin = origin.into();
            if !settings.channel_separation_enabled || !sonar.available() {
                self.apps.push(view);
                continue;
            }
            if self.suppressed.contains(&identity.stable_id) {
                self.external_choices.insert(identity.stable_id.clone());
                view.origin = "external".into();
                view.message = Some(
                    "External app route preserved; assign again to resume Jack control".into(),
                );
                self.apps.push(view);
                continue;
            }
            let binding = match routes.policy.bind_process(app.process_id) {
                Ok(binding) => binding,
                Err(e) => {
                    view.message = Some(e.to_string());
                    self.apps.push(view);
                    continue;
                }
            };
            let keys = app_keys(&binding);
            if keys.iter().any(|key| self.recovery_keys.contains(key)) {
                view.message =
                    Some("Previous app policy recovery is pending; retrying automatically".into());
                self.apps.push(view);
                continue;
            }
            let journal = match routes.store.load() {
                Ok(j) => j,
                Err(e) => {
                    view.message = Some(e.to_string());
                    self.apps.push(view);
                    continue;
                }
            };
            let owned: Vec<_> = journal
                .changes
                .iter()
                .filter(|c| keys.contains(&c.key))
                .collect();
            let foreign = owned.iter().any(|c| {
                routes
                    .policy
                    .read(&c.key)
                    .is_ok_and(|value| value.as_deref() != Some(&c.applied))
            });
            if foreign {
                self.suppressed.insert(identity.stable_id.clone());
                self.external_choices.insert(identity.stable_id.clone());
                view.origin = "external".into();
                view.message = Some("External app route preserved".into());
                self.apps.push(view);
                continue;
            }
            let Some(source) = sonar.sources.iter().find(|s| s.channel == target) else {
                view.message = Some("Requested Sonar virtual output is unavailable".into());
                self.apps.push(view);
                continue;
            };
            // Sonar Classic writes Console/Multimedia; Communications is retained.
            // An accepted route must not be replayed while the app recreates its session.
            let policy_is_target = keys[..2].iter().all(|k| {
                routes
                    .policy
                    .read(k)
                    .is_ok_and(|v| v.as_deref() == Some(&source.endpoint_id))
            });
            // An external per-app preference wins over a previously saved Jack rule.
            let external_override = owned.is_empty()
                && keys
                    .iter()
                    .any(|k| routes.policy.read(k).is_ok_and(|v| v.is_some()));
            if external_override && !policy_is_target {
                self.external_choices.insert(identity.stable_id.clone());
                view.origin = "external".into();
                view.message = Some("Existing app preference preserved".into());
                self.apps.push(view);
                continue;
            }
            if !policy_is_target {
                if self
                    .retry_after
                    .get(&identity.stable_id)
                    .is_some_and(|time| Instant::now() < *time)
                {
                    view.message = Some("App routing retry is waiting".into());
                    self.apps.push(view);
                    continue;
                }
                if let Err(e) = self.apply(
                    routes,
                    service,
                    sonar,
                    app.process_id,
                    &app.process_name,
                    target,
                ) {
                    self.retry_after.insert(
                        identity.stable_id.clone(),
                        Instant::now() + Duration::from_secs(10),
                    );
                    view.message = Some(e);
                }
            }
            let confirmed_policy = keys[..2].iter().all(|key| {
                routes
                    .policy
                    .read(key)
                    .is_ok_and(|value| value.as_deref() == Some(&source.endpoint_id))
            });
            view.pending =
                confirmed_policy && app.route.channel() != Some(target) && view.message.is_none();
            self.apps.push(view);
        }
        match routes.store.load() {
            Ok(journal) => {
                let pending: BTreeSet<_> = journal
                    .changes
                    .iter()
                    .filter(|change| change.phase != Phase::Applied)
                    .map(|change| match &change.key {
                        RouteKey::App { stable_app_id, .. } => stable_app_id.clone(),
                        RouteKey::Default { .. } => "Windows defaults".into(),
                    })
                    .collect();
                if !pending.is_empty() {
                    self.warnings.push(format!("{} apps await policy recovery; recovery retries automatically when their audio sessions return", pending.len()));
                }
            }
            Err(error) => self
                .warnings
                .push(format!("Cannot read app recovery journal: {error}")),
        }
    }
    pub fn clear(&mut self, routes: &mut Routes, pid: u32) -> Result<(), String> {
        let binding = routes.policy.bind_process(pid).map_err(|e| e.to_string())?;
        let keys = app_keys(&binding).into_iter().collect();
        request_restore(routes, &keys).map_err(|e| e.to_string())?;
        let report = routes
            .restore_keys_best_effort(&keys)
            .map_err(|e| e.to_string())?;
        self.remember_recovery(&report);
        if !report.complete {
            return Err("App policy recovery pending".into());
        }
        Ok(())
    }
}

/// A user's external edit ends our ownership; do not undo other roles while
/// acknowledging it. Unfinished transactions still retain their recovery record.
fn release_assignments<P: AudioPolicy, S: JournalStore>(
    routes: &mut RouteController<P, S>,
    keys: &BTreeSet<RouteKey>,
) -> Result<(), RoutingError> {
    let mut journal = routes.store.load()?;
    let before = journal.changes.len();
    journal
        .changes
        .retain(|change| change.phase != Phase::Applied || !keys.contains(&change.key));
    if before != journal.changes.len() {
        routes.store.save(&journal)?;
    }
    Ok(())
}

fn recover_pending_routes<P: AudioPolicy, S: JournalStore>(
    routes: &mut RouteController<P, S>,
    pending: &mut BTreeSet<RouteKey>,
) -> Result<(), String> {
    // Derive intent from the durable phase, including undo requested while an app
    // was closed. Never infer undo from a failed read or a stale PID binding.
    *pending = routes
        .store
        .load()
        .map_err(|e| e.to_string())?
        .changes
        .into_iter()
        .filter(|change| change.phase != Phase::Applied)
        .map(|change| change.key)
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let report = routes
        .restore_keys_best_effort(pending)
        .map_err(|error| error.to_string())?;
    *pending = report.pending.into_iter().map(|entry| entry.key).collect();
    Ok(())
}
