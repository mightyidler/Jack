//! All COM objects, registry changes and settings writes stay on this MTA owner.
use jack_core::{
    Settings, Theme,
    routing::{AudioPolicy, FileJournal, Flow, Role, RouteController, RouteKey, WriteOutcome},
};
use jack_windows_audio::{
    AudioMonitor, Snapshot,
    control::{ControlLease, current_user_root},
    policy::WindowsAudioPolicy,
    volume::EndpointVolumeControl,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Mutex, mpsc},
    thread,
};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::*,
    },
    core::w,
};

#[derive(Serialize)]
pub struct ViewState {
    pub settings: Settings,
    pub audio: Snapshot,
    pub separation_available: bool,
    pub sonar: jack_sonar::State,
    pub app_routing: Vec<crate::sonar_routing::AppView>,
    pub routing_warnings: Vec<String>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Read,
    Sonar {
        command: jack_sonar::Command,
    },
    SonarRetry,
    OutputSync {
        enabled: bool,
    },
    // Retained for recovery diagnostics; no desktop IPC or product mode switch.
    #[serde(skip)]
    Separation {
        enabled: bool,
    },
    AddGroup,
    RemoveGroup {
        group_id: uuid::Uuid,
    },
    AssignApplication {
        process_id: u32,
        process_name: String,
        channel: Option<jack_sonar::sonar::models::Channel>,
        automatic: bool,
    },
    Preferences {
        theme: Theme,
        language: String,
        launch_at_login: bool,
    },
    TrayShortcut {
        enabled: bool,
        key: Option<String>,
        modifiers: u32,
    },
    TrayVolumeNumbers {
        enabled: bool,
    },
    Reset,
    #[serde(skip)]
    InitializeStartup,
    #[serde(skip)]
    UpdatePause {
        paused: bool,
    },
    // Historical native verification only; physical device pickers now use Sonar.
    #[serde(skip)]
    Select {
        flow: Flow,
        endpoint: String,
        expected: Option<String>,
    },
    #[serde(skip)]
    Volume {
        flow: Flow,
        endpoint: String,
        level: f32,
        expected: f32,
        expected_muted: bool,
    },
    Shutdown,
}
type Reply = Result<ViewState, String>;
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveVolumeTarget {
    Sonar {
        channel: jack_sonar::sonar::models::Channel,
    },
}
enum Request {
    Action {
        action: Action,
        reply: mpsc::SyncSender<Reply>,
    },
    LiveVolume {
        target: LiveVolumeTarget,
        level: f64,
        expected: f64,
        reply: mpsc::SyncSender<Result<f64, String>>,
    },
}
pub struct Broker {
    sender: mpsc::SyncSender<Request>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}
impl Broker {
    pub fn start() -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel::<Request>(16);
        let (ready, initialized) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("jack-audio-control".into())
            .spawn(move || {
                let mut owner = match Owner::new() {
                    Ok(owner) => {
                        let _ = ready.send(Ok(()));
                        owner
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                loop {
                    let request = match receiver.recv_timeout(std::time::Duration::from_secs(10)) {
                        Ok(request) => request,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // Reconcile hidden-window app starts at a lower rate.
                            let _ = owner.view();
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    match request {
                        Request::Action { action, reply } => {
                            let shutdown = matches!(action, Action::Shutdown);
                            let result = owner.handle(action);
                            let done = shutdown && result.is_ok();
                            let _ = reply.send(result);
                            if done {
                                break;
                            }
                        }
                        Request::LiveVolume {
                            target,
                            level,
                            expected,
                            reply,
                        } => {
                            let _ = reply.send(owner.live_volume(target, level, expected));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        if let Err(error) = initialized.recv().map_err(|e| e.to_string())? {
            let _ = worker.join();
            return Err(error);
        }
        Ok(Self {
            sender,
            worker: Mutex::new(Some(worker)),
        })
    }
    pub fn call(&self, action: Action) -> Reply {
        let shutdown = matches!(action, Action::Shutdown);
        let (reply, result) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::Action { action, reply })
            .map_err(|e| e.to_string())?;
        let result = result.recv().map_err(|e| e.to_string())?;
        if shutdown
            && result.is_ok()
            && let Some(worker) = self.worker.lock().map_err(|e| e.to_string())?.take()
        {
            worker
                .join()
                .map_err(|_| "audio control thread panicked".to_string())?;
        }
        result
    }
    pub fn live_volume(
        &self,
        target: LiveVolumeTarget,
        level: f64,
        expected: f64,
    ) -> Result<f64, String> {
        let (reply, result) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::LiveVolume {
                target,
                level,
                expected,
                reply,
            })
            .map_err(|e| e.to_string())?;
        result.recv().map_err(|e| e.to_string())?
    }
}
struct Owner {
    routes: RouteController<WindowsAudioPolicy, FileJournal>,
    monitor: AudioMonitor,
    settings: Settings,
    settings_path: PathBuf,
    launch_command: String,
    _lease: ControlLease,
    sonar: jack_sonar::Service,
    app_routing: crate::sonar_routing::AppRouting,
    app_icons: crate::program_icons::IconCache,
    update_paused: bool,
}
impl Owner {
    fn live_volume(
        &mut self,
        target: LiveVolumeTarget,
        level: f64,
        expected: f64,
    ) -> Result<f64, String> {
        if self.update_paused {
            return Err("Jack is preparing an update".into());
        }
        if !level.is_finite() || !(0.0..=1.0).contains(&level) || !expected.is_finite() {
            return Err("Invalid volume".into());
        }
        match target {
            LiveVolumeTarget::Sonar { channel } => {
                if channel != jack_sonar::sonar::models::Channel::Microphone
                    && !self.settings.groups.iter().any(|g| {
                        g.output_channel()
                            .is_some_and(|c| crate::sonar_routing::channel(c) == channel)
                    })
                {
                    return Err("The audio group no longer exists".into());
                }
                let mixer = match channel {
                    jack_sonar::sonar::models::Channel::Game => {
                        jack_sonar::sonar::models::MixerChannel::Game
                    }
                    jack_sonar::sonar::models::Channel::Chat => {
                        jack_sonar::sonar::models::MixerChannel::Chat
                    }
                    jack_sonar::sonar::models::Channel::Media => {
                        jack_sonar::sonar::models::MixerChannel::Media
                    }
                    jack_sonar::sonar::models::Channel::Aux => {
                        jack_sonar::sonar::models::MixerChannel::Aux
                    }
                    jack_sonar::sonar::models::Channel::Microphone => {
                        jack_sonar::sonar::models::MixerChannel::Microphone
                    }
                };
                self.sonar
                    .set_volume_live(mixer, level, expected)
                    .map_err(|e| e.to_string())
            }
        }
    }
    fn new() -> Result<Self, String> {
        let root = current_user_root().map_err(|e| e.to_string())?;
        let lease = ControlLease::acquire(&root.join("control.lock")).map_err(|e| e.to_string())?;
        let policy = WindowsAudioPolicy::new().map_err(|e| e.to_string())?;
        let mut monitor = AudioMonitor::new().map_err(|e| e.to_string())?;
        let mut routes = RouteController {
            policy,
            store: FileJournal {
                path: root.join("routes.json"),
            },
        };
        for session in monitor.snapshot().map_err(|e| e.to_string())?.sessions {
            if session.pid != 0 {
                let _ = routes.policy.bind_process(session.pid);
            }
        }
        let report = routes.restore_best_effort().map_err(|e| e.to_string())?;
        let mut app_routing = crate::sonar_routing::AppRouting::default();
        app_routing.remember_recovery(&report);
        let settings_path = root.join("settings.json");
        let mut settings = jack_core::settings::load_recovering(&settings_path)
            .map_err(|e| e.to_string())?
            .settings;
        let launch_command = format!(
            "\"{}\" --background",
            std::env::current_exe()
                .map_err(|e| e.to_string())?
                .display()
        );
        // The desktop product now always uses groups, including legacy OFF profiles.
        settings.channel_separation_enabled = true;
        let fixed = fixed_builtin_groups(&settings);
        if fixed != settings {
            jack_core::settings::save(&settings_path, &fixed).map_err(|e| e.to_string())?;
            settings = fixed;
        }
        Ok(Self {
            routes,
            monitor,
            settings,
            settings_path,
            launch_command,
            _lease: lease,
            sonar: jack_sonar::Service::new().map_err(|e| e.to_string())?,
            app_routing,
            app_icons: Default::default(),
            update_paused: false,
        })
    }
    fn view(&mut self) -> Reply {
        self.routes.policy.prune_exited_processes();
        let mut sonar = self.sonar.refresh(false);
        if self.update_paused {
            return Ok(self.passive_view(sonar));
        }
        let mut remembered = self.settings.clone();
        if crate::group_outputs::remember(&mut remembered, &sonar) {
            self.preferences(remembered)?;
        }
        let audio = self.monitor.snapshot().map_err(|e| e.to_string())?;
        self.app_routing.refresh_inventory(&mut sonar);
        if let Some(next) = synchronized_optional_groups(&self.settings, &sonar)? {
            for group in &self.settings.groups {
                if !next.groups.iter().any(|remaining| remaining.id == group.id)
                    && let Some(target) = group.output_channel()
                {
                    self.app_routing.restore_channel(
                        &mut self.routes,
                        &sonar,
                        crate::sonar_routing::channel(target),
                        &self.settings,
                        &audio,
                    )?;
                }
            }
            // This mirrors an external GG choice; never writes back to GG.
            self.preferences(next)?;
        }
        self.app_routing.reconcile(
            &mut self.routes,
            &mut self.sonar,
            &self.settings,
            &audio,
            &sonar,
        );
        // Routing may recreate audio sessions; publish current effective routes.
        if self.app_routing.apps.iter().any(|app| app.pending) {
            sonar = self.sonar.refresh(false);
            self.app_routing.refresh_inventory(&mut sonar);
            for app in &mut self.app_routing.apps {
                if app.pending
                    && sonar.applications.iter().any(|a| {
                        a.process_id == app.process_id && a.route.channel() == app.requested
                    })
                {
                    app.pending = false;
                    app.message = None;
                }
            }
        }
        for app in &mut self.app_routing.apps {
            if let Some(path) = audio
                .sessions
                .iter()
                .find(|session| session.pid == app.process_id)
                .and_then(|session| session.executable_path.as_deref())
                .map(str::to_owned)
                .or_else(|| {
                    jack_windows_audio::identity::process_executable_path(app.process_id).ok()
                })
            {
                app.icon_data_url = self
                    .app_icons
                    .get_or_extract(&path, || crate::program_icons::data_url(&path));
            }
        }
        Ok(ViewState {
            settings: self.settings.clone(),
            audio,
            separation_available: sonar.available(),
            sonar,
            app_routing: self.app_routing.apps.clone(),
            routing_warnings: self.app_routing.warnings.clone(),
        })
    }
    // Shutdown/update recovery has already completed. A UI snapshot must not
    // reapply rules, save preferences or turn a monitor error into a failed quit.
    fn passive_view(&mut self, sonar: jack_sonar::State) -> ViewState {
        let audio = self.monitor.snapshot().unwrap_or_else(|error| Snapshot {
            endpoints: vec![],
            sessions: vec![],
            warnings: vec![error.to_string()],
            monitor: None,
        });
        ViewState {
            settings: self.settings.clone(),
            audio,
            separation_available: sonar.available(),
            sonar,
            app_routing: vec![],
            routing_warnings: self.app_routing.warnings.clone(),
        }
    }
    fn preferences(&mut self, next: Settings) -> Result<(), String> {
        self.apply_preferences(next, false)
    }
    fn apply_preferences(
        &mut self,
        next: Settings,
        synchronize_startup: bool,
    ) -> Result<(), String> {
        if !matches!(next.language.as_str(), "ko" | "en") {
            return Err("unsupported language".into());
        }
        let previous = read_startup()?;
        let desired = next.launch_at_login.then_some(self.launch_command.as_str());
        let change_startup =
            synchronize_startup || next.launch_at_login != self.settings.launch_at_login;
        if change_startup && previous.as_deref() != desired {
            write_startup(desired)?;
            if read_startup()?.as_deref() != desired {
                return Err("Windows startup setting could not be confirmed".into());
            }
        }
        if let Err(error) = jack_core::settings::save(&self.settings_path, &next) {
            if change_startup && read_startup()?.as_deref() == desired {
                write_startup(previous.as_deref())?;
            }
            return Err(format!("Settings save failed: {error}"));
        }
        self.settings = next;
        Ok(())
    }
    fn handle(&mut self, action: Action) -> Reply {
        if self.update_paused
            && !matches!(
                action,
                Action::Read | Action::Shutdown | Action::UpdatePause { .. }
            )
        {
            return Err("Jack is preparing an update".into());
        }
        match action {
            Action::InitializeStartup => {
                self.apply_preferences(initial_startup_preferences(&self.settings), true)?;
            }
            Action::UpdatePause { paused } => {
                if paused && !self.update_paused {
                    let sonar = self.sonar.refresh(false);
                    self.app_routing.restore_live(&mut self.routes, &sonar)?;
                }
                self.update_paused = paused;
            }
            Action::Read => {}
            Action::Sonar { command } => {
                if let jack_sonar::Command::Route {
                    channel,
                    device_id,
                    expected,
                } = command
                {
                    let next = crate::group_outputs::select(
                        &mut self.sonar,
                        &self.settings,
                        channel,
                        &device_id,
                        &expected,
                        |next| {
                            if next != &self.settings {
                                jack_core::settings::save(&self.settings_path, next)
                                    .map_err(|error| format!("Settings save failed: {error}"))?;
                            }
                            Ok(())
                        },
                    )?;
                    self.settings = next;
                } else {
                    self.sonar.execute(command).map_err(|e| e.to_string())?;
                }
            }
            Action::SonarRetry => {
                self.sonar.refresh(true);
            }
            Action::OutputSync { enabled } => {
                let next = crate::group_outputs::set_linked(
                    &mut self.sonar,
                    &self.settings,
                    enabled,
                    |next| {
                        jack_core::settings::save(&self.settings_path, next)
                            .map_err(|error| format!("Settings save failed: {error}"))
                    },
                )?;
                self.settings = next;
            }
            Action::Separation { enabled } => {
                if !enabled {
                    let sonar = self.sonar.refresh(false);
                    self.app_routing.restore_live(&mut self.routes, &sonar)?;
                }
                let mut next = self.settings.clone();
                next.channel_separation_enabled = enabled;
                self.preferences(next)?;
            }
            Action::AddGroup => {
                let sonar = self.sonar.refresh(false);
                if !self.settings.channel_separation_enabled || !sonar.available() {
                    return Err(
                        "Enable channel separation with Sonar ready before adding a group".into(),
                    );
                }
                let target = jack_core::OutputChannel::Aux;
                if self
                    .settings
                    .groups
                    .iter()
                    .any(|g| g.output_channel() == Some(target))
                {
                    return Err("Maximum four groups".into());
                }
                let mut next = self.settings.clone();
                let name = auxiliary_group_name(&next.language);
                next.add_sonar_group(name.into(), target)
                    .map_err(|e| e.to_string())?;
                let channel = crate::sonar_routing::channel(target);
                let enable_source = !sonar.sources.iter().any(|source| source.channel == channel);
                let mut initial_route = None;
                let result = (|| {
                    if enable_source {
                        self.sonar
                            .execute(jack_sonar::Command::Source {
                                channel,
                                enabled: true,
                                expected: false,
                            })
                            .map_err(|e| e.to_string())?;
                    }
                    let current = self.sonar.refresh(false);
                    initial_route = initial_group_route(&self.settings, &current, target)?;
                    if let Some((previous, inherited)) = &initial_route {
                        self.sonar
                            .execute(jack_sonar::Command::Route {
                                channel,
                                device_id: inherited.clone(),
                                expected: previous.clone(),
                            })
                            .map_err(|e| e.to_string())?;
                    }
                    self.preferences(next)
                })();
                if let Err(error) = result {
                    let mut errors = vec![error];
                    if let Some((previous, inherited)) = initial_route
                        && self
                            .sonar
                            .refresh(false)
                            .routes
                            .iter()
                            .any(|route| route.channel == channel && route.device_id == inherited)
                        && let Err(rollback) = self.sonar.execute(jack_sonar::Command::Route {
                            channel,
                            device_id: previous,
                            expected: inherited,
                        })
                    {
                        errors.push(format!("new group output recovery failed: {rollback}"));
                    }
                    if enable_source
                        && let Err(rollback) = self.sonar.execute(jack_sonar::Command::Source {
                            channel,
                            enabled: false,
                            expected: true,
                        })
                    {
                        errors.push(format!("new group channel recovery failed: {rollback}"));
                    }
                    return Err(errors.join("; "));
                }
            }
            Action::RemoveGroup { group_id } => {
                if !self.settings.groups.iter().any(|g| {
                    g.id == group_id && g.output_channel() == Some(jack_core::OutputChannel::Aux)
                }) {
                    return Err("The three built-in audio groups are fixed".into());
                }
                let mut next = self.settings.clone();
                let target = next
                    .remove_sonar_group(group_id)
                    .map_err(|e| e.to_string())?;
                let sonar = self.sonar.refresh(false);
                let target = match target {
                    jack_core::OutputChannel::Game => jack_sonar::sonar::models::Channel::Game,
                    jack_core::OutputChannel::Chat => jack_sonar::sonar::models::Channel::Chat,
                    jack_core::OutputChannel::Media => jack_sonar::sonar::models::Channel::Media,
                    jack_core::OutputChannel::Aux => jack_sonar::sonar::models::Channel::Aux,
                };
                self.app_routing.restore_channel(
                    &mut self.routes,
                    &sonar,
                    target,
                    &self.settings,
                    &self.monitor.snapshot().map_err(|e| e.to_string())?,
                )?;
                // GG exposes removable input channels only for Media and Aux.
                // Restore owned app policies before turning their source off.
                let disable_source =
                    matches!(
                        target,
                        jack_sonar::sonar::models::Channel::Media
                            | jack_sonar::sonar::models::Channel::Aux
                    ) && sonar.sources.iter().any(|source| source.channel == target);
                if disable_source {
                    self.sonar
                        .execute(jack_sonar::Command::Source {
                            channel: target,
                            enabled: false,
                            expected: true,
                        })
                        .map_err(|e| e.to_string())?;
                }
                if let Err(error) = self.preferences(next) {
                    if disable_source
                        && let Err(rollback) = self.sonar.execute(jack_sonar::Command::Source {
                            channel: target,
                            enabled: true,
                            expected: false,
                        })
                    {
                        return Err(format!(
                            "{error}; group channel recovery failed: {rollback}"
                        ));
                    }
                    return Err(error);
                }
            }
            Action::AssignApplication {
                process_id,
                process_name,
                channel,
                automatic,
            } => {
                if !self.settings.channel_separation_enabled {
                    return Err("Enable channel separation before assigning apps".into());
                }
                let sonar = self.sonar.refresh(false);
                if !sonar
                    .applications
                    .iter()
                    .any(|app| app.process_id == process_id && app.process_name == process_name)
                {
                    return Err("Audio session changed; refresh before assigning".into());
                }
                let identity = self.app_routing.identity(process_id)?;
                let mut next = self.settings.clone();
                self.app_routing.allow(&identity.stable_id);
                if let Some(channel) = channel {
                    let output = crate::sonar_routing::output(channel)?;
                    if !next
                        .groups
                        .iter()
                        .any(|group| group.output_channel() == Some(output))
                    {
                        return Err("The selected group no longer exists".into());
                    }
                    self.app_routing.apply(
                        &mut self.routes,
                        &mut self.sonar,
                        &sonar,
                        process_id,
                        &process_name,
                        channel,
                    )?;
                    next.sonar_app_overrides.insert(
                        identity.stable_id.clone(),
                        crate::sonar_routing::output(channel)?,
                    );
                    next.sonar_auto_exclusions.remove(&identity.stable_id);
                } else {
                    self.app_routing.clear(&mut self.routes, process_id)?;
                    next.sonar_app_overrides.remove(&identity.stable_id);
                    if automatic {
                        next.sonar_auto_exclusions.remove(&identity.stable_id);
                    } else {
                        next.sonar_auto_exclusions
                            .insert(identity.stable_id.clone());
                    }
                }
                if let Err(error) = self.preferences(next) {
                    self.app_routing.clear(&mut self.routes, process_id)?;
                    return Err(error);
                }
            }
            Action::Preferences {
                theme,
                language,
                launch_at_login,
            } => {
                let mut next = self.settings.clone();
                next.theme = theme;
                next.language = language;
                next.launch_at_login = launch_at_login;
                next.startup_preference_initialized = true;
                self.preferences(next)?;
            }
            Action::TrayVolumeNumbers { enabled } => {
                let mut next = self.settings.clone();
                next.tray_volume_numbers_always_visible = enabled;
                self.preferences(next)?;
            }
            Action::TrayShortcut {
                enabled,
                key,
                modifiers,
            } => {
                let mut next = self.settings.clone();
                next.tray_shortcut = jack_core::TrayShortcut {
                    enabled,
                    key,
                    modifiers,
                };
                self.preferences(next)?;
            }
            Action::Reset => {
                let sonar = self.sonar.refresh(false);
                self.app_routing.restore_live(&mut self.routes, &sonar)?;
                self.preferences(fixed_builtin_groups(&Settings {
                    channel_separation_enabled: true,
                    startup_preference_initialized: true,
                    ..Settings::default()
                }))?;
            }
            Action::Select {
                flow,
                endpoint,
                expected,
            } => {
                choose_default(
                    &mut self.routes.policy,
                    flow,
                    &endpoint,
                    expected.as_deref(),
                )?;
            }
            Action::Volume {
                flow,
                endpoint,
                level,
                expected,
                expected_muted,
            } => {
                let snapshot = self.monitor.snapshot().map_err(|e| e.to_string())?;
                let flow_name = match flow {
                    Flow::Render => "render",
                    Flow::Capture => "capture",
                };
                if !snapshot
                    .endpoints
                    .iter()
                    .any(|e| e.id == endpoint && e.flow == flow_name)
                {
                    return Err("device disconnected or has wrong flow".into());
                }
                let control = EndpointVolumeControl::new(&endpoint).map_err(|e| e.to_string())?;
                let current = control.state().map_err(|e| e.to_string())?;
                if !expected.is_finite()
                    || (current.level - expected).abs() > 0.0001
                    || current.muted != expected_muted
                {
                    return Err("volume changed externally; refresh before retrying".into());
                }
                control.set_level(level).map_err(|e| e.to_string())?;
                let after = control.state().map_err(|e| e.to_string())?;
                if (after.level - level).abs() > 0.0001 || after.muted != expected_muted {
                    return Err("volume readback mismatch".into());
                }
            }
            Action::Shutdown => {
                let sonar = self.sonar.refresh(false);
                self.app_routing.restore_live(&mut self.routes, &sonar)?;
                return Ok(self.passive_view(sonar));
            }
        }
        self.view()
    }
}
fn initial_startup_preferences(settings: &Settings) -> Settings {
    let mut next = settings.clone();
    if !next.startup_preference_initialized {
        next.launch_at_login = true;
        next.startup_preference_initialized = true;
    }
    next
}
fn auxiliary_group_name(language: &str) -> &'static str {
    if language == "en" { "Aux" } else { "보조" }
}

/// Latest product rule: Game/Chat/Media have canonical identity/title/icon;
/// migrate old reused Media groups without losing their app/output/volume data.
fn fixed_builtin_groups(settings: &Settings) -> Settings {
    use jack_core::{CHAT, GAMING, MEDIA, OutputChannel};
    let mut next = settings.clone();
    let defaults = Settings::default();
    let mut remapped = std::collections::BTreeMap::new();
    next.groups = [
        (GAMING, OutputChannel::Game, "게이밍", "Frame-1"),
        (CHAT, OutputChannel::Chat, "채팅", "Frame-2"),
        (MEDIA, OutputChannel::Media, "미디어", "Frame-3"),
    ]
    .into_iter()
    .map(|(id, channel, name, icon)| {
        let mut group = settings
            .groups
            .iter()
            .find(|g| g.output_channel() == Some(channel))
            .cloned()
            .unwrap_or_else(|| defaults.groups.iter().find(|g| g.id == id).unwrap().clone());
        remapped.insert(group.id, id);
        group.id = id;
        group.display_name = name.into();
        group.icon_asset_id = Some(icon.into());
        group.sonar_channel = Some(channel);
        group
    })
    .collect();
    if let Some(aux) = settings
        .groups
        .iter()
        .find(|g| g.output_channel() == Some(OutputChannel::Aux))
    {
        let mut aux = aux.clone();
        if matches!(
            aux.display_name.as_str(),
            "새 그룹" | "New group" | "New Group" | "보조" | "Aux"
        ) {
            aux.display_name = auxiliary_group_name(&settings.language).into();
        }
        if [GAMING, CHAT, MEDIA].contains(&aux.id) {
            let previous = aux.id;
            aux.id = uuid::Uuid::from_u128(4);
            remapped.insert(previous, aux.id);
        }
        next.groups.push(aux);
    }
    next.user_app_overrides.retain(|_, id| {
        if let Some(canonical) = remapped.get(id) {
            *id = *canonical;
        }
        next.groups.iter().any(|g| g.id == *id)
    });
    next
}
/// Permanent explicit user choice. Only Console/Multimedia; never Communications.
/// Mirror optional GG channels only from a complete, ready Classic snapshot.
fn synchronized_optional_groups(
    settings: &Settings,
    state: &jack_sonar::State,
) -> Result<Option<Settings>, String> {
    use jack_core::OutputChannel;
    use jack_sonar::sonar::models::{Channel, MixerChannel};
    if !settings.channel_separation_enabled
        || !state.available()
        || ![Channel::Game, Channel::Chat].iter().all(|channel| {
            state
                .sources
                .iter()
                .any(|source| source.channel == *channel)
        })
        || ![Channel::Media, Channel::Aux]
            .iter()
            .all(|channel| state.routes.iter().any(|route| route.channel == *channel))
        || ![MixerChannel::Media, MixerChannel::Aux]
            .iter()
            .all(|channel| {
                state
                    .volumes
                    .iter()
                    .any(|volume| volume.channel == *channel)
            })
    {
        return Ok(None);
    }
    let mut next = settings.clone();
    let mut changed = false;
    // The three built-in groups remain fixed; only Aux mirrors GG add/delete.
    {
        let target = OutputChannel::Aux;
        if !state
            .sources
            .iter()
            .any(|source| source.channel == crate::sonar_routing::channel(target))
            && let Some(id) = next
                .groups
                .iter()
                .find(|group| group.output_channel() == Some(target))
                .map(|group| group.id)
        {
            next.remove_sonar_group(id)
                .map_err(|error| error.to_string())?;
            changed = true;
        }
    }
    {
        let target = OutputChannel::Aux;
        if state
            .sources
            .iter()
            .any(|source| source.channel == crate::sonar_routing::channel(target))
            && !next
                .groups
                .iter()
                .any(|group| group.output_channel() == Some(target))
        {
            let name = auxiliary_group_name(&settings.language);
            next.add_sonar_group(name.into(), target)
                .map_err(|error| error.to_string())?;
            changed = true;
        }
    }
    Ok(changed.then_some(next))
}
/// Inherit a connected output from an existing group, rather than unused channel history.
fn initial_group_route(
    settings: &Settings,
    state: &jack_sonar::State,
    target: jack_core::OutputChannel,
) -> Result<Option<(String, String)>, String> {
    let target = crate::sonar_routing::channel(target);
    if !state.available() || !state.sources.iter().any(|source| source.channel == target) {
        return Err("The new group's channel is not ready".into());
    }
    if settings.groups.is_empty() {
        return Ok(None);
    }
    let previous = state
        .routes
        .iter()
        .find(|route| route.channel == target)
        .ok_or("Sonar did not report the new group's output")?;
    let inherited = settings
        .groups
        .iter()
        .filter_map(|group| group.output_channel())
        .map(crate::sonar_routing::channel)
        .filter_map(|channel| state.routes.iter().find(|route| route.channel == channel))
        .find(|route| {
            state.devices.iter().any(|device| {
                device.id == route.device_id
                    && device.enabled
                    && device.role == jack_sonar::sonar::models::DeviceRole::Playback
            })
        })
        .ok_or("No connected output is selected in the existing groups")?;
    Ok((previous.device_id != inherited.device_id)
        .then(|| (previous.device_id.clone(), inherited.device_id.clone())))
}
fn choose_default<P: AudioPolicy>(
    policy: &mut P,
    flow: Flow,
    target: &str,
    expected: Option<&str>,
) -> Result<(), String> {
    let keys = [Role::Console, Role::Multimedia].map(|role| RouteKey::Default { flow, role });
    let before = keys
        .iter()
        .map(|k| policy.read(k))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if before[0].as_deref() != expected {
        return Err("default changed externally; refresh before retrying".into());
    }
    let mut owned = Vec::new();
    let operation = (|| {
        for (i, key) in keys.iter().enumerate() {
            let current = policy.read(key).map_err(|e| e.to_string())?;
            if current.as_deref() == Some(target) {
                if before[i].as_deref() != Some(target) {
                    owned.push(i);
                }
                continue;
            }
            if current != before[i] {
                return Err("default changed during selection".into());
            }
            match policy
                .compare_and_set(key, current.as_deref(), Some(target))
                .map_err(|e| e.to_string())?
            {
                WriteOutcome::Applied => owned.push(i),
                WriteOutcome::Conflict => return Err("default changed during selection".into()),
            }
        }
        for key in &keys {
            if policy.read(key).map_err(|e| e.to_string())?.as_deref() != Some(target) {
                return Err("default readback mismatch".into());
            }
        }
        Ok(())
    })();
    if let Err(error) = operation {
        let mut errors = vec![error];
        for i in owned.into_iter().rev() {
            if let Err(error) = policy.compare_and_set(&keys[i], Some(target), before[i].as_deref())
            {
                errors.push(format!("rollback: {error}"));
            }
        }
        return Err(errors.join("; "));
    }
    Ok(())
}
const RUN_KEY: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
fn read_startup() -> Result<Option<String>, String> {
    let mut data = vec![0u16; 32768];
    let mut bytes = (data.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            w!("Jack"),
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status
        .ok()
        .map_err(|e| format!("Windows startup read failed: {e}"))?;
    let len = (bytes as usize / 2).saturating_sub(1);
    String::from_utf16(&data[..len])
        .map(Some)
        .map_err(|e| format!("Windows startup read failed: {e}"))
}
fn write_startup(value: Option<&str>) -> Result<(), String> {
    let status = match value {
        Some(value) => {
            let data: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    RUN_KEY,
                    w!("Jack"),
                    REG_SZ.0,
                    Some(data.as_ptr().cast()),
                    (data.len() * 2) as u32,
                )
            }
        }
        None => unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Jack")) },
    };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_SUCCESS {
        Ok(())
    } else {
        status
            .ok()
            .map_err(|e| format!("Windows startup write failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_default_on_migrates_legacy_preferences_once_and_preserves_explicit_off() {
        let legacy = Settings {
            launch_at_login: false,
            ..Settings::default()
        };
        let migrated = initial_startup_preferences(&legacy);
        assert!(migrated.launch_at_login);
        assert!(migrated.startup_preference_initialized);
        assert_eq!(migrated.groups, legacy.groups);
        assert_eq!(migrated.sonar_app_overrides, legacy.sonar_app_overrides);
        let explicit_off = Settings {
            launch_at_login: false,
            ..migrated
        };
        let saved = serde_json::to_vec(&explicit_off).unwrap();
        let loaded = serde_json::from_slice(&saved).unwrap();
        assert_eq!(initial_startup_preferences(&loaded), explicit_off);
    }
    use jack_core::routing::RoutingError;
    use std::collections::BTreeMap;
    struct Policy {
        values: BTreeMap<RouteKey, Option<String>>,
        coupled: bool,
        fail_multimedia: bool,
    }
    impl AudioPolicy for Policy {
        fn read(&mut self, key: &RouteKey) -> Result<Option<String>, RoutingError> {
            Ok(self.values[key].clone())
        }
        fn compare_and_set(
            &mut self,
            key: &RouteKey,
            expected: Option<&str>,
            desired: Option<&str>,
        ) -> Result<WriteOutcome, RoutingError> {
            if self.read(key)?.as_deref() != expected {
                return Ok(WriteOutcome::Conflict);
            }
            if self.fail_multimedia
                && matches!(
                    key,
                    RouteKey::Default {
                        role: Role::Multimedia,
                        ..
                    }
                )
            {
                return Err(RoutingError::Readback);
            }
            self.values.insert(key.clone(), desired.map(String::from));
            if self.coupled
                && let RouteKey::Default {
                    flow,
                    role: Role::Console,
                } = key
            {
                self.values.insert(
                    RouteKey::Default {
                        flow: *flow,
                        role: Role::Multimedia,
                    },
                    desired.map(String::from),
                );
            }
            Ok(WriteOutcome::Applied)
        }
    }
    fn policy(coupled: bool) -> Policy {
        Policy {
            values: [Role::Console, Role::Multimedia, Role::Communications]
                .map(|role| {
                    (
                        RouteKey::Default {
                            flow: Flow::Render,
                            role,
                        },
                        Some(
                            if role == Role::Communications {
                                "chat"
                            } else {
                                "old"
                            }
                            .into(),
                        ),
                    )
                })
                .into(),
            coupled,
            fail_multimedia: false,
        }
    }
    #[test]
    fn explicit_choice_handles_coupling_and_preserves_communications() {
        let mut p = policy(true);
        choose_default(&mut p, Flow::Render, "new", Some("old")).unwrap();
        assert_eq!(
            p.values[&RouteKey::Default {
                flow: Flow::Render,
                role: Role::Communications
            }]
                .as_deref(),
            Some("chat")
        );
        for role in [Role::Console, Role::Multimedia] {
            assert_eq!(
                p.values[&RouteKey::Default {
                    flow: Flow::Render,
                    role
                }]
                    .as_deref(),
                Some("new")
            );
        }
    }
    #[test]
    fn stale_choice_writes_nothing() {
        let mut p = policy(false);
        let before = p.values.clone();
        assert!(choose_default(&mut p, Flow::Render, "new", Some("stale")).is_err());
        assert_eq!(p.values, before);
    }
    #[test]
    fn gg_optional_groups_import_remove_and_preserve_existing_metadata() {
        use jack_core::OutputChannel;
        use jack_sonar::sonar::models::Channel;
        let settings = Settings {
            channel_separation_enabled: true,
            ..Default::default()
        };
        let mut state = ready_optional_channels();
        state.sources.push(jack_sonar::Source {
            channel: Channel::Aux,
            endpoint_id: "aux".into(),
        });
        let mut imported = synchronized_optional_groups(&settings, &state)
            .unwrap()
            .unwrap();
        assert_eq!(imported.groups.len(), 4);
        assert_eq!(
            serde_json::to_value(&imported.groups[..3]).unwrap(),
            serde_json::to_value(&settings.groups).unwrap()
        );
        assert!(
            synchronized_optional_groups(&imported, &state)
                .unwrap()
                .is_none()
        );
        imported
            .sonar_app_overrides
            .insert("path:aux-app".into(), OutputChannel::Aux);
        imported
            .sonar_app_overrides
            .insert("path:music".into(), OutputChannel::Media);
        state
            .sources
            .retain(|source| source.channel != Channel::Aux);
        let removed = synchronized_optional_groups(&imported, &state)
            .unwrap()
            .unwrap();
        assert_eq!(removed.groups.len(), 3);
        assert!(!removed.sonar_app_overrides.contains_key("path:aux-app"));
        assert!(removed.sonar_auto_exclusions.contains("path:aux-app"));
        assert_eq!(
            removed.sonar_app_overrides["path:music"],
            OutputChannel::Media
        );
        assert!(
            synchronized_optional_groups(&removed, &state)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn external_media_disable_preserves_fixed_group_and_aux_uses_fourth_slot() {
        use jack_core::OutputChannel;
        use jack_sonar::sonar::models::Channel;
        let settings = fixed_builtin_groups(&Settings {
            channel_separation_enabled: true,
            ..Default::default()
        });
        let mut state = ready_optional_channels();
        state
            .sources
            .retain(|source| source.channel != Channel::Media);
        state.sources.push(jack_sonar::Source {
            channel: Channel::Aux,
            endpoint_id: "aux".into(),
        });
        let switched = synchronized_optional_groups(&settings, &state)
            .unwrap()
            .unwrap();
        assert_eq!(switched.groups.len(), 4);
        assert!(
            switched
                .groups
                .iter()
                .any(|group| group.output_channel() == Some(OutputChannel::Media))
        );
        state.sources.push(jack_sonar::Source {
            channel: Channel::Media,
            endpoint_id: "media".into(),
        });
        assert!(
            synchronized_optional_groups(&switched, &state)
                .unwrap()
                .is_none()
        );
        let media = switched
            .groups
            .iter()
            .find(|group| group.output_channel() == Some(OutputChannel::Media))
            .unwrap();
        assert_eq!(media.id, jack_core::MEDIA);
        assert_eq!(media.icon_asset_id.as_deref(), Some("Frame-3"));
        assert_eq!(switched.groups.len(), 4);
    }
    #[test]
    fn legacy_reused_media_group_becomes_fixed_without_losing_assignments_or_output() {
        let mut legacy = Settings::default();
        let old = uuid::Uuid::new_v4();
        legacy.groups[2].id = old;
        legacy.groups[2].sonar_channel = Some(jack_core::OutputChannel::Media);
        legacy.groups[2].display_name = "새 그룹".into();
        legacy.groups[2].icon_asset_id = Some("Frame-1".into());
        legacy.groups[2].physical_output_id = Some("selected-output".into());
        legacy.groups[2].volume = 0.42;
        legacy.user_app_overrides.insert("path:player".into(), old);
        legacy
            .sonar_app_overrides
            .insert("path:player".into(), jack_core::OutputChannel::Media);
        let fixed = fixed_builtin_groups(&legacy);
        assert_eq!(fixed.groups[2].id, jack_core::MEDIA);
        assert_eq!(fixed.groups[2].display_name, "미디어");
        assert_eq!(fixed.groups[2].icon_asset_id.as_deref(), Some("Frame-3"));
        assert_eq!(
            fixed.groups[2].physical_output_id,
            legacy.groups[2].physical_output_id
        );
        assert_eq!(fixed.groups[2].volume, 0.42);
        assert_eq!(fixed.user_app_overrides["path:player"], jack_core::MEDIA);
        assert_eq!(fixed.sonar_app_overrides, legacy.sonar_app_overrides);
        fixed.validate().unwrap();
        assert_eq!(fixed_builtin_groups(&fixed), fixed);
    }
    #[test]
    fn gg_disconnection_incomplete_snapshot_and_separation_off_keep_saved_groups() {
        let settings = Settings {
            channel_separation_enabled: true,
            ..Default::default()
        };
        let mut state = ready_optional_channels();
        state.sources.clear();
        assert!(
            synchronized_optional_groups(&settings, &state)
                .unwrap()
                .is_none()
        );
        for connection in ["disconnected", "waiting", "unsupported_mode"] {
            state = ready_optional_channels();
            state.connection = connection.into();
            assert!(
                synchronized_optional_groups(&settings, &state)
                    .unwrap()
                    .is_none()
            );
        }
        state = ready_optional_channels();
        state.volumes.clear();
        assert!(
            synchronized_optional_groups(&settings, &state)
                .unwrap()
                .is_none()
        );
        state = ready_optional_channels();
        state.routes.clear();
        assert!(
            synchronized_optional_groups(&settings, &state)
                .unwrap()
                .is_none()
        );
        state = ready_optional_channels();
        state
            .sources
            .retain(|source| source.channel != jack_sonar::sonar::models::Channel::Media);
        assert!(
            synchronized_optional_groups(&Settings::default(), &state)
                .unwrap()
                .is_none()
        );
    }
    fn ready_optional_channels() -> jack_sonar::State {
        use jack_sonar::sonar::models::{Channel, MixerChannel, Route, VolumeState};
        jack_sonar::State {
            connection: "connected".into(),
            mode: Some("classic".into()),
            sources: [Channel::Game, Channel::Chat, Channel::Media]
                .map(|channel| jack_sonar::Source {
                    channel,
                    endpoint_id: format!("{channel:?}"),
                })
                .to_vec(),
            routes: [Channel::Media, Channel::Aux]
                .map(|channel| Route {
                    channel,
                    device_id: "physical".into(),
                    device_name: Some("Existing GG output".into()),
                })
                .to_vec(),
            volumes: [MixerChannel::Media, MixerChannel::Aux]
                .map(|channel| VolumeState {
                    channel,
                    volume: 0.63,
                    muted: false,
                })
                .to_vec(),
            ..Default::default()
        }
    }
    #[test]
    fn new_group_uses_existing_selected_output_instead_of_aux_history() {
        use jack_sonar::sonar::models::{AudioDevice, Channel, DeviceRole, Route};
        let settings = Settings::default();
        let mut state = jack_sonar::State {
            connection: "connected".into(),
            mode: Some("classic".into()),
            ..Default::default()
        };
        state.sources.push(jack_sonar::Source {
            channel: Channel::Aux,
            endpoint_id: "aux-source".into(),
        });
        state.devices = ["existing-output", "old-aux-output"]
            .map(|id| AudioDevice {
                id: id.into(),
                name: id.into(),
                role: DeviceRole::Playback,
                enabled: true,
                virtual_device: false,
            })
            .to_vec();
        state.routes = [
            (Channel::Game, "existing-output"),
            (Channel::Chat, "old-aux-output"),
            (Channel::Aux, "old-aux-output"),
        ]
        .map(|(channel, id)| Route {
            channel,
            device_id: id.into(),
            device_name: None,
        })
        .to_vec();
        assert_eq!(
            initial_group_route(&settings, &state, jack_core::OutputChannel::Aux).unwrap(),
            Some(("old-aux-output".into(), "existing-output".into()))
        );
        state.devices[0].enabled = false;
        assert_eq!(
            initial_group_route(&settings, &state, jack_core::OutputChannel::Aux).unwrap(),
            None
        );
        state.devices[1].enabled = false;
        assert!(initial_group_route(&settings, &state, jack_core::OutputChannel::Aux).is_err());
        state.devices[1].enabled = true;
        state.sources.clear();
        assert!(initial_group_route(&settings, &state, jack_core::OutputChannel::Aux).is_err());
    }
    #[test]
    fn failed_second_role_restores_owned_first_role() {
        let mut p = policy(false);
        let before = p.values.clone();
        p.fail_multimedia = true;
        assert!(choose_default(&mut p, Flow::Render, "new", Some("old")).is_err());
        assert_eq!(p.values, before);
    }
}
