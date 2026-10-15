use super::*;
use crate::{CHAT, GAMING, MEDIA};
use std::{cell::RefCell, rc::Rc};
type Events = Rc<RefCell<Vec<&'static str>>>;
#[derive(Default)]
struct Store {
    journal: Journal,
    fail_save: bool,
}
impl JournalStore for Store {
    fn load(&self) -> Result<Journal, RoutingError> {
        Ok(self.journal.clone())
    }
    fn save(&mut self, j: &Journal) -> Result<(), RoutingError> {
        if self.fail_save {
            return Err(std::io::Error::other("disk full").into());
        }
        self.journal = j.clone();
        Ok(())
    }
}
struct Policy {
    values: BTreeMap<RouteKey, String>,
    events: Events,
    fail_apply: bool,
    fail_restore: bool,
}
impl AudioPolicy for Policy {
    fn read(&mut self, key: &RouteKey) -> Result<Option<String>, RoutingError> {
        Ok(self.values.get(key).cloned())
    }
    fn compare_and_set(
        &mut self,
        key: &RouteKey,
        expected: Option<&str>,
        desired: Option<&str>,
    ) -> Result<WriteOutcome, RoutingError> {
        if self.values.get(key).map(String::as_str) != expected {
            return Ok(WriteOutcome::Conflict);
        }
        let applying = desired.is_some_and(|v| v.starts_with("jack-"));
        if (applying && self.fail_apply) || (!applying && self.fail_restore) {
            return Err(RoutingError::Policy("simulated Windows error".into()));
        }
        self.events
            .borrow_mut()
            .push(if applying { "route" } else { "restore" });
        if let Some(value) = desired {
            self.values.insert(key.clone(), value.into());
        } else {
            self.values.remove(key);
        }
        Ok(WriteOutcome::Applied)
    }
}
struct Engine {
    events: Events,
    live: bool,
    prepare_fail: bool,
    start_fail: bool,
    stop_fail: bool,
    health_fail: bool,
    health_fail_after_route: bool,
    gain_fail: bool,
    gains: BTreeMap<Uuid, f32>,
}
impl AudioEngine for Engine {
    type Prepared = EnginePlan;
    fn prepare(&mut self, p: &EnginePlan) -> Result<EnginePlan, String> {
        self.events.borrow_mut().push("prepare");
        if self.prepare_fail {
            Err("prepare failed".into())
        } else {
            Ok(p.clone())
        }
    }
    fn start(&mut self, p: EnginePlan) -> Result<(), String> {
        self.events.borrow_mut().push("start");
        self.live = true;
        self.gains = p.groups.iter().map(|g| (g.id, g.gain)).collect();
        if self.start_fail {
            Err("partial start failed".into())
        } else {
            Ok(())
        }
    }
    fn stop(&mut self) -> Result<(), String> {
        self.events.borrow_mut().push("stop");
        if self.stop_fail {
            Err("stop failed".into())
        } else {
            self.live = false;
            Ok(())
        }
    }
    fn healthy(&self) -> bool {
        self.live
            && !self.health_fail
            && !(self.health_fail_after_route && self.events.borrow().contains(&"route"))
    }
    fn set_group_gain(&mut self, id: Uuid, gain: f32) -> Result<(), String> {
        if self.gain_fail {
            Err("gain command failed".into())
        } else {
            self.gains.insert(id, gain);
            Ok(())
        }
    }
}
fn fixture() -> (
    RuntimeController<Policy, Store, Engine>,
    Settings,
    Inventory,
    Vec<AppIdentity>,
) {
    let events = Events::default();
    let policy = Policy {
        values: BTreeMap::new(),
        events: events.clone(),
        fail_apply: false,
        fail_restore: false,
    };
    let engine = Engine {
        events,
        live: false,
        prepare_fail: false,
        start_fail: false,
        stop_fail: false,
        health_fail: false,
        health_fail_after_route: false,
        gain_fail: false,
        gains: BTreeMap::new(),
    };
    let controller =
        RuntimeController::new(policy, Store::default(), engine, vec![Role::Multimedia]).unwrap();
    let mut settings = Settings::default();
    let mut endpoints = vec![
        Endpoint {
            id: "headset".into(),
            kind: EndpointKind::PhysicalRender,
            active: true,
        },
        Endpoint {
            id: "speakers".into(),
            kind: EndpointKind::PhysicalRender,
            active: true,
        },
    ];
    for group in &mut settings.groups {
        let id = format!("jack-{}", group.id.as_u128());
        group.virtual_endpoint_id = Some(id.clone());
        endpoints.push(Endpoint {
            id,
            kind: EndpointKind::VerifiedJackRender,
            active: true,
        });
    }
    settings.assign_app("game".into(), GAMING).unwrap();
    let apps = vec![AppIdentity {
        stable_id: "game".into(),
        executable_name: None,
        verified_publisher: None,
        app_user_model_id: None,
    }];
    (
        controller,
        settings,
        Inventory {
            endpoints,
            default_render: Some("headset".into()),
        },
        apps,
    )
}
fn key(id: &str) -> RouteKey {
    RouteKey::App {
        stable_app_id: id.into(),
        flow: Flow::Render,
        role: Role::Multimedia,
    }
}
#[test]
fn plan_shares_one_output_and_reports_missing_device_fallback() {
    let (_, mut s, i, _) = fixture();
    s.groups[0].physical_output_id = Some("gone".into());
    s.groups[1].physical_output_id = Some("speakers".into());
    let p = build_plan(&s, &i).unwrap();
    assert_eq!(p.outputs.len(), 2);
    assert_eq!(p.fallbacks.len(), 1);
    assert_eq!(
        p.outputs
            .iter()
            .find(|o| o.endpoint == "headset")
            .unwrap()
            .groups,
        vec![GAMING, MEDIA]
    );
    assert_eq!(s.groups[0].physical_output_id.as_deref(), Some("gone"));
}
#[test]
fn unverified_inactive_and_feedback_sources_never_start_or_route() {
    for variant in 0..3 {
        let (mut c, mut s, mut i, a) = fixture();
        match variant {
            0 => i.endpoints[2].kind = EndpointKind::PhysicalRender,
            1 => i.endpoints[2].active = false,
            _ => s.groups[0].physical_output_id = Some("jack-1".into()),
        }
        assert!(c.enable(&s, &i, &a).is_err());
        assert!(c.engine.events.borrow().is_empty());
        assert_eq!(c.status().state, RuntimeState::Off);
    }
}
#[test]
fn start_precedes_routes_and_restore_precedes_stop_without_erasing_settings() {
    let (mut c, s, i, a) = fixture();
    c.routes
        .policy
        .values
        .insert(key("game"), "speakers".into());
    c.enable(&s, &i, &a).unwrap();
    assert_eq!(c.status().state, RuntimeState::On);
    assert_eq!(*c.engine.events.borrow(), vec!["prepare", "start", "route"]);
    assert!(c.shutdown().unwrap().complete);
    assert_eq!(
        *c.engine.events.borrow(),
        vec!["prepare", "start", "route", "restore", "stop"]
    );
    assert_eq!(c.routes.policy.values[&key("game")], "speakers");
    assert_eq!(
        c.settings().unwrap().user_app_overrides,
        s.user_app_overrides
    );
    assert_eq!(c.settings().unwrap().groups, s.groups);
    assert!(!c.settings().unwrap().channel_separation_enabled);
}
#[test]
fn start_or_prepare_failure_cannot_claim_on_or_change_policy() {
    for prepare in [false, true] {
        let (mut c, s, i, a) = fixture();
        c.engine.prepare_fail = prepare;
        c.engine.start_fail = !prepare;
        assert!(c.enable(&s, &i, &a).is_err());
        assert_eq!(c.status().state, RuntimeState::Off);
        assert!(!c.engine.live);
        assert!(c.routes.policy.values.is_empty());
    }
}
#[test]
fn health_failure_during_policy_apply_is_recovered_before_on() {
    let (mut c, s, i, a) = fixture();
    c.engine.health_fail_after_route = true;
    assert!(c.enable(&s, &i, &a).is_err());
    assert_eq!(c.status().state, RuntimeState::Off);
    assert!(c.routes.policy.values.is_empty());
    assert!(!c.engine.live);
}
#[test]
fn pending_journal_blocks_start_and_can_be_recovered_before_enable() {
    let (mut c, s, i, a) = fixture();
    c.routes.apply(key("old"), "jack-1".into()).unwrap();
    c.engine.events.borrow_mut().clear();
    assert!(matches!(
        c.enable(&s, &i, &a),
        Err(RuntimeError::Routing(RoutingError::RecoveryNeeded))
    ));
    assert!(c.engine.events.borrow().is_empty());
    assert!(c.disable().unwrap().complete);
    c.enable(&s, &i, &a).unwrap();
    assert_eq!(c.status().state, RuntimeState::On);
}
#[test]
fn failed_route_apply_and_disk_sync_cleanup_without_fake_on() {
    for disk in [false, true] {
        let (mut c, s, i, a) = fixture();
        c.routes.store.fail_save = disk;
        c.routes.policy.fail_apply = !disk;
        assert!(c.enable(&s, &i, &a).is_err());
        assert_eq!(c.status().state, RuntimeState::Off);
        assert!(!c.engine.live);
        assert!(c.routes.policy.values.is_empty());
        assert!(c.routes.store.journal.changes.is_empty());
    }
}
#[test]
fn incomplete_restore_keeps_engine_until_retry_succeeds() {
    let (mut c, s, i, a) = fixture();
    c.enable(&s, &i, &a).unwrap();
    c.routes.policy.fail_restore = true;
    assert!(!c.disable().unwrap().complete);
    assert_eq!(c.status().state, RuntimeState::Recovering);
    assert!(c.engine.live);
    assert_eq!(c.routes.store.journal.changes.len(), 1);
    c.routes.policy.fail_restore = false;
    assert!(c.disable().unwrap().complete);
    assert!(!c.engine.live);
    assert_eq!(c.status().state, RuntimeState::Off);
}
#[test]
fn engine_failure_restores_apps_and_stop_failure_remains_retryable() {
    let (mut c, s, i, a) = fixture();
    c.enable(&s, &i, &a).unwrap();
    c.engine.health_fail = true;
    c.engine.stop_fail = true;
    assert!(c.poll_health().is_err());
    assert_eq!(c.status().state, RuntimeState::Faulted);
    assert!(c.routes.policy.values.is_empty());
    assert!(c.status().engine_running);
    c.engine.stop_fail = false;
    c.disable().unwrap();
    assert_eq!(c.status().state, RuntimeState::Off);
}
#[test]
fn new_apps_user_reassignment_and_app_departure_reconcile_owned_routes() {
    let (mut c, mut s, i, mut a) = fixture();
    s.assign_app("chat".into(), CHAT).unwrap();
    c.enable(&s, &i, &a).unwrap();
    a.push(AppIdentity {
        stable_id: "chat".into(),
        executable_name: None,
        verified_publisher: None,
        app_user_model_id: None,
    });
    c.reconcile_apps(&a).unwrap();
    assert_eq!(c.routes.policy.values[&key("chat")], "jack-2");
    c.assign_app("game".into(), MEDIA, &a).unwrap();
    assert_eq!(c.routes.policy.values[&key("game")], "jack-3");
    a.remove(1);
    c.reconcile_apps(&a).unwrap();
    assert!(!c.routes.policy.values.contains_key(&key("chat")));
    assert_eq!(c.routes.store.journal.changes.len(), 1);
    c.disable().unwrap();
    assert!(c.routes.policy.values.is_empty());
}
#[test]
fn external_choice_stays_suppressed_until_a_direct_user_assignment() {
    let (mut c, s, i, a) = fixture();
    c.enable(&s, &i, &a).unwrap();
    c.routes
        .policy
        .values
        .insert(key("game"), "external-speakers".into());
    c.reconcile_apps(&a).unwrap();
    c.reconcile_apps(&a).unwrap();
    assert_eq!(c.routes.policy.values[&key("game")], "external-speakers");
    assert_eq!(c.status().externally_overridden_routes, 1);
    c.assign_app("game".into(), CHAT, &a).unwrap();
    assert_eq!(c.routes.policy.values[&key("game")], "jack-2");
    c.disable().unwrap();
    assert_eq!(c.routes.policy.values[&key("game")], "external-speakers");
}
#[test]
fn unclassified_and_conflicting_app_identity_do_not_get_routed() {
    let (mut c, s, i, mut a) = fixture();
    a.push(AppIdentity {
        stable_id: "unknown".into(),
        executable_name: Some("fake-discord.exe".into()),
        verified_publisher: None,
        app_user_model_id: None,
    });
    c.enable(&s, &i, &a).unwrap();
    assert!(!c.routes.policy.values.contains_key(&key("unknown")));
    c.disable().unwrap();
    let mut trusted = a[1].clone();
    trusted.executable_name = Some("Discord.exe".into());
    trusted.verified_publisher = Some("Discord Inc.".into());
    a.push(trusted);
    assert!(c.enable(&s, &i, &a).is_err());
    assert!(c.routes.policy.values.is_empty());
}
#[test]
fn group_gain_is_committed_only_on_backend_success() {
    let (mut c, s, i, a) = fixture();
    c.enable(&s, &i, &a).unwrap();
    c.set_group_gain(CHAT, 0.4).unwrap();
    assert_eq!(c.settings().unwrap().groups[1].volume, 0.4);
    assert_eq!(c.settings().unwrap().groups[0].volume, 1.0);
    c.engine.gain_fail = true;
    assert!(c.set_group_gain(CHAT, 0.8).is_err());
    assert_eq!(c.settings().unwrap().groups[1].volume, 0.4);
    assert_eq!(c.status().state, RuntimeState::Off);
}
#[test]
fn failed_assignment_rolls_model_back_and_unowned_virtual_is_rejected() {
    let (mut c, s, i, a) = fixture();
    c.routes.policy.values.insert(key("game"), "jack-1".into());
    assert!(c.enable(&s, &i, &a).is_err());
    assert!(c.engine.events.borrow().is_empty());
    c.routes.policy.values.clear();
    c.enable(&s, &i, &a).unwrap();
    c.routes.policy.fail_apply = true;
    assert!(c.assign_app("game".into(), CHAT, &a).is_err());
    assert_eq!(c.settings().unwrap().user_app_overrides["game"], GAMING);
    assert_eq!(c.status().state, RuntimeState::Off);
}
