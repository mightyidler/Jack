//! Control-thread runtime protocol. Windows engine/endpoint verification is a backend
//! contract, not inferred from friendly names or a successful model transition.
use crate::{AppIdentity, Classification, ModelError, Settings, classify, routing::*};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    PhysicalRender,
    VerifiedJackRender,
}
/// Backend must verify PnP/driver provenance before assigning VerifiedJackRender.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub id: String,
    pub kind: EndpointKind,
    pub active: bool,
}
pub struct Inventory {
    pub endpoints: Vec<Endpoint>,
    pub default_render: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupPlan {
    pub id: Uuid,
    pub source: String,
    pub output: String,
    pub gain: f32,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputPlan {
    pub endpoint: String,
    pub groups: Vec<Uuid>,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutputFallback {
    pub group: Uuid,
    pub requested: String,
    pub using: String,
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EnginePlan {
    pub groups: Vec<GroupPlan>,
    pub outputs: Vec<OutputPlan>,
    pub fallbacks: Vec<OutputFallback>,
}
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error(transparent)]
    Routing(#[from] RoutingError),
    #[error("audio engine: {0}")]
    Engine(String),
    #[error("runtime plan: {0}")]
    Plan(&'static str),
    #[error("operation requires a different runtime state")]
    State,
}
pub fn build_plan(settings: &Settings, inventory: &Inventory) -> Result<EnginePlan, RuntimeError> {
    settings.validate()?;
    settings.can_activate(true)?;
    let mut endpoints = BTreeMap::new();
    for endpoint in &inventory.endpoints {
        if endpoint.id.trim().is_empty()
            || endpoints.insert(endpoint.id.as_str(), endpoint).is_some()
        {
            return Err(RuntimeError::Plan("invalid/duplicate endpoint inventory"));
        }
    }
    let mut plan = EnginePlan {
        groups: vec![],
        outputs: vec![],
        fallbacks: vec![],
    };
    let mut outputs: BTreeMap<String, Vec<Uuid>> = BTreeMap::new();
    for group in &settings.groups {
        let source = group
            .virtual_endpoint_id
            .as_deref()
            .ok_or(RuntimeError::Plan("missing virtual source"))?;
        if !endpoints
            .get(source)
            .is_some_and(|e| e.active && e.kind == EndpointKind::VerifiedJackRender)
        {
            return Err(RuntimeError::Plan(
                "virtual source is unavailable or unverified",
            ));
        }
        let selected = group.physical_output_id.as_deref();
        if selected
            .and_then(|id| endpoints.get(id))
            .is_some_and(|e| e.kind == EndpointKind::VerifiedJackRender)
        {
            return Err(RuntimeError::Plan(
                "virtual source cannot be a physical output",
            ));
        }
        let selected_active = selected
            .and_then(|id| endpoints.get(id))
            .filter(|e| e.active && e.kind == EndpointKind::PhysicalRender);
        let output = if let Some(e) = selected_active {
            e.id.clone()
        } else {
            let fallback = inventory
                .default_render
                .as_deref()
                .and_then(|id| endpoints.get(id))
                .filter(|e| e.active && e.kind == EndpointKind::PhysicalRender)
                .ok_or(RuntimeError::Plan("no active physical output/default"))?;
            if let Some(requested) = selected {
                plan.fallbacks.push(OutputFallback {
                    group: group.id,
                    requested: requested.into(),
                    using: fallback.id.clone(),
                });
            }
            fallback.id.clone()
        };
        outputs.entry(output.clone()).or_default().push(group.id);
        plan.groups.push(GroupPlan {
            id: group.id,
            source: source.into(),
            output,
            gain: group.volume,
        });
    }
    plan.outputs = outputs
        .into_iter()
        .map(|(endpoint, groups)| OutputPlan { endpoint, groups })
        .collect();
    Ok(plan)
}
pub trait AudioEngine {
    type Prepared;
    /// Recheck active source provenance/output formats and acquire all resources.
    /// Partial failures must release the prepared resources. No policy writes here.
    fn prepare(&mut self, plan: &EnginePlan) -> Result<Self::Prepared, String>;
    fn start(&mut self, prepared: Self::Prepared) -> Result<(), String>;
    /// Must attempt to stop every owned worker/client, even when one stop fails.
    fn stop(&mut self) -> Result<(), String>;
    fn healthy(&self) -> bool;
    fn set_group_gain(&mut self, group: Uuid, gain: f32) -> Result<(), String>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Off,
    Starting,
    On,
    Recovering,
    Faulted,
}
#[derive(Debug, Serialize)]
pub struct RuntimeStatus {
    pub state: RuntimeState,
    pub engine_running: bool,
    pub engine_healthy: bool,
    pub last_error: Option<String>,
    pub externally_overridden_routes: usize,
}
pub struct RuntimeController<P, S, E> {
    routes: RouteController<P, S>,
    engine: E,
    roles: Vec<Role>,
    state: RuntimeState,
    running: bool,
    settings: Option<Settings>,
    plan: Option<EnginePlan>,
    suppressed: BTreeSet<RouteKey>,
    last_error: Option<String>,
}
impl<P: AudioPolicy, S: JournalStore, E: AudioEngine> RuntimeController<P, S, E> {
    /// Roles are explicit; this protocol does not decide Communications policy.
    /// Caller must hold an exclusive process/journal lease throughout its lifetime.
    pub fn new(policy: P, store: S, engine: E, roles: Vec<Role>) -> Result<Self, RuntimeError> {
        if roles.is_empty() || roles.iter().collect::<BTreeSet<_>>().len() != roles.len() {
            return Err(RuntimeError::Plan("empty/duplicate role selection"));
        }
        Ok(Self {
            routes: RouteController { policy, store },
            engine,
            roles,
            state: RuntimeState::Off,
            running: false,
            settings: None,
            plan: None,
            suppressed: BTreeSet::new(),
            last_error: None,
        })
    }
    pub fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            state: self.state,
            engine_running: self.running,
            engine_healthy: self.running && self.engine.healthy(),
            last_error: self.last_error.clone(),
            externally_overridden_routes: self.suppressed.len(),
        }
    }
    pub fn settings(&self) -> Option<&Settings> {
        self.settings.as_ref()
    }
    pub fn plan(&self) -> Option<&EnginePlan> {
        self.plan.as_ref()
    }
    /// Backend binding/read hook only. Policy mutations must stay in the journal.
    pub fn with_policy_adapter<R>(&mut self, operation: impl FnOnce(&mut P) -> R) -> R {
        operation(&mut self.routes.policy)
    }
    pub fn enable(
        &mut self,
        settings: &Settings,
        inventory: &Inventory,
        apps: &[AppIdentity],
    ) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::Off {
            return Err(RuntimeError::State);
        }
        if !self.routes.store.load()?.changes.is_empty() {
            return Err(RoutingError::RecoveryNeeded.into());
        }
        let plan = build_plan(settings, inventory)?;
        let desired = desired_routes(settings, &plan, apps, &self.roles)?;
        // An unowned pre-existing virtual override has no known original to restore.
        for (key, target) in &desired {
            if self.routes.policy.read(key)?.as_deref() == Some(target.as_str()) {
                return Err(RuntimeError::Plan(
                    "app already uses unowned virtual route; reconcile its original first",
                ));
            }
        }
        self.state = RuntimeState::Starting;
        let prepared = match self.engine.prepare(&plan) {
            Ok(p) => p,
            Err(e) => {
                self.state = RuntimeState::Off;
                self.last_error = Some(e.clone());
                return Err(RuntimeError::Engine(e));
            }
        };
        if let Err(error) = self.engine.start(prepared) {
            let stop = self.engine.stop();
            self.running = stop.is_err();
            self.state = if stop.is_ok() {
                RuntimeState::Off
            } else {
                RuntimeState::Faulted
            };
            self.last_error = Some(format!("start failed: {error}; cleanup: {stop:?}"));
            return Err(RuntimeError::Engine(error));
        }
        self.running = true;
        self.settings = Some(settings.clone());
        self.plan = Some(plan);
        self.suppressed.clear();
        if !self.engine.healthy() {
            return self.abort(RuntimeError::Engine("engine did not become healthy".into()));
        }
        if let Err(e) = self.routes.apply_batch(desired.into_iter().collect()) {
            return self.abort(e.into());
        }
        if !self.engine.healthy() {
            return self.abort(RuntimeError::Engine(
                "engine failed while applying routes".into(),
            ));
        }
        self.settings
            .as_mut()
            .expect("settings installed")
            .channel_separation_enabled = true;
        self.state = RuntimeState::On;
        self.last_error = None;
        Ok(())
    }
    fn abort(&mut self, error: RuntimeError) -> Result<(), RuntimeError> {
        let message = error.to_string();
        let recovery = self.disable();
        self.last_error = Some(format!(
            "{message}; recovery: {}",
            match recovery {
                Ok(r) => format!("complete={}", r.complete),
                Err(e) => e.to_string(),
            }
        ));
        Err(error)
    }
    /// Restore policy FIRST. If any route is pending, keep a healthy engine running.
    /// A failed stop remains Faulted/running until cleanup is retried.
    pub fn disable(&mut self) -> Result<RecoveryReport, RuntimeError> {
        self.state = RuntimeState::Recovering;
        let report = match self.routes.restore_best_effort() {
            Ok(r) => r,
            Err(e) => {
                self.last_error = Some(e.to_string());
                return Err(e.into());
            }
        };
        if !report.complete {
            self.last_error = Some(format!(
                "{} owned routes pending restoration",
                report.pending.len()
            ));
            return Ok(report);
        }
        if self.running
            && let Err(e) = self.engine.stop()
        {
            self.state = RuntimeState::Faulted;
            self.last_error = Some(e.clone());
            return Err(RuntimeError::Engine(e));
        }
        self.running = false;
        self.state = RuntimeState::Off;
        self.plan = None;
        self.suppressed.clear();
        self.last_error = None;
        if let Some(settings) = &mut self.settings {
            settings.channel_separation_enabled = false;
        }
        Ok(report)
    }
    /// Explicit shutdown/recovery must succeed before the application exits.
    pub fn shutdown(&mut self) -> Result<RecoveryReport, RuntimeError> {
        self.disable()
    }
    pub fn poll_health(&mut self) -> Result<Option<RecoveryReport>, RuntimeError> {
        if self.state == RuntimeState::On && !self.engine.healthy() {
            let report = self.disable()?;
            self.last_error = Some("engine health failure triggered policy restoration".into());
            return Ok(Some(report));
        }
        Ok(None)
    }
    /// Apps without trusted classification stay on Windows. PID binding is backend work.
    pub fn reconcile_apps(&mut self, apps: &[AppIdentity]) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::On {
            return Err(RuntimeError::State);
        }
        if !self.engine.healthy() {
            return self.abort(RuntimeError::Engine(
                "engine health failed before routing".into(),
            ));
        }
        let settings = self.settings.as_ref().expect("enabled settings");
        let plan = self.plan.as_ref().expect("enabled plan");
        let desired = desired_routes(settings, plan, apps, &self.roles)?;
        let journal = self.routes.store.load()?;
        let mut release = BTreeSet::new();
        for change in &journal.changes {
            if matches!(change.key, RouteKey::App { .. }) {
                if self.routes.policy.read(&change.key)?.as_deref() != Some(change.applied.as_str())
                {
                    self.suppressed.insert(change.key.clone());
                    release.insert(change.key.clone());
                } else if !desired.contains_key(&change.key) {
                    release.insert(change.key.clone());
                }
            }
        }
        let recovery = match self.routes.restore_keys_best_effort(&release) {
            Ok(r) => r,
            Err(e) => return self.abort(e.into()),
        };
        if !recovery.complete {
            return self.abort(RoutingError::RecoveryNeeded.into());
        }
        let requests = desired
            .into_iter()
            .filter(|(k, _)| !self.suppressed.contains(k))
            .collect();
        if let Err(e) = self.routes.apply_batch(requests) {
            return self.abort(e.into());
        }
        Ok(())
    }
    /// Call only for a direct user app assignment action, not on every monitor tick.
    pub fn assign_app(
        &mut self,
        stable_id: String,
        group: Uuid,
        apps: &[AppIdentity],
    ) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::On {
            return Err(RuntimeError::State);
        }
        let mut changed = self.settings.as_ref().expect("enabled settings").clone();
        changed.assign_app(stable_id.clone(), group)?;
        let previous_settings = self.settings.clone();
        let previous_suppressed = self.suppressed.clone();
        self.settings = Some(changed);
        self.suppressed.retain(
            |key| !matches!(key,RouteKey::App {stable_app_id,..} if stable_app_id==&stable_id),
        );
        if let Err(error) = self.reconcile_apps(apps) {
            self.settings = previous_settings;
            if let Some(settings) = &mut self.settings {
                settings.channel_separation_enabled =
                    self.state == RuntimeState::On || self.state == RuntimeState::Recovering;
            }
            if self.state == RuntimeState::On {
                self.suppressed = previous_suppressed;
            }
            return Err(error);
        }
        Ok(())
    }
    pub fn set_group_gain(&mut self, group: Uuid, gain: f32) -> Result<(), RuntimeError> {
        if self.state != RuntimeState::On {
            return Err(RuntimeError::State);
        }
        if !self.engine.healthy() {
            return self.abort(RuntimeError::Engine(
                "engine health failed before gain change".into(),
            ));
        }
        if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
            return Err(ModelError::Volume.into());
        }
        if !self
            .settings
            .as_ref()
            .expect("enabled settings")
            .groups
            .iter()
            .any(|g| g.id == group)
        {
            return Err(ModelError::Group.into());
        }
        if let Err(error) = self.engine.set_group_gain(group, gain) {
            return self.abort(RuntimeError::Engine(error));
        }
        self.settings
            .as_mut()
            .expect("enabled settings")
            .groups
            .iter_mut()
            .find(|g| g.id == group)
            .expect("validated group")
            .volume = gain;
        self.plan
            .as_mut()
            .expect("enabled plan")
            .groups
            .iter_mut()
            .find(|g| g.id == group)
            .expect("prepared group")
            .gain = gain;
        Ok(())
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
fn desired_routes(
    settings: &Settings,
    plan: &EnginePlan,
    apps: &[AppIdentity],
    roles: &[Role],
) -> Result<BTreeMap<RouteKey, String>, RuntimeError> {
    let mut identities = BTreeMap::new();
    let mut desired = BTreeMap::new();
    for app in apps {
        if app.stable_id.trim().is_empty() {
            return Err(RuntimeError::Plan("empty app identity"));
        }
        let classification = classify(app, settings);
        if let Some(previous) = identities.insert(&app.stable_id, classification)
            && previous != classification
        {
            return Err(RuntimeError::Plan(
                "conflicting live identities; cannot aggregate this app",
            ));
        }
        let group = match classification {
            Classification::User(g) | Classification::BuiltIn(g) => g,
            Classification::Unclassified => continue,
        };
        let source = &plan
            .groups
            .iter()
            .find(|g| g.id == group)
            .ok_or(RuntimeError::Plan("app group has no engine source"))?
            .source;
        for role in roles {
            desired.insert(
                RouteKey::App {
                    stable_app_id: app.stable_id.clone(),
                    flow: Flow::Render,
                    role: *role,
                },
                source.clone(),
            );
        }
    }
    Ok(desired)
}
