//! In-process Sonar service. Sonar owns audio state; Jack never starts an engine.
pub mod error;
mod platform;
pub mod sonar;
#[cfg(test)]
mod tests;

use error::{Error, Result};
use serde::{Deserialize, Serialize};
use sonar::{client::SonarClient, discovery::DiscoveryOptions, models::*};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct State {
    pub connection: String,
    pub message: Option<String>,
    pub mode: Option<String>,
    pub volumes: Vec<VolumeState>,
    pub routes: Vec<Route>,
    pub devices: Vec<AudioDevice>,
    pub applications: Vec<ApplicationSession>,
    pub sources: Vec<Source>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub channel: Channel,
    pub endpoint_id: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            connection: "disconnected".into(),
            message: None,
            mode: None,
            volumes: vec![],
            routes: vec![],
            devices: vec![],
            applications: vec![],
            sources: vec![],
        }
    }
}
impl State {
    pub fn available(&self) -> bool {
        self.connection == "connected" && self.mode.as_deref() == Some("classic")
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Volume {
        channel: MixerChannel,
        level: f64,
        expected: f64,
    },
    Mute {
        channel: MixerChannel,
        muted: bool,
        expected: bool,
    },
    Route {
        channel: Channel,
        device_id: String,
        expected: String,
    },
    Source {
        channel: Channel,
        enabled: bool,
        expected: bool,
    },
    Application {
        process_id: u32,
        process_name: String,
        channel: Channel,
    },
}

/// Owned by the existing control worker, with bounded I/O and rediscovery backoff.
pub struct Service {
    runtime: tokio::runtime::Runtime,
    client: Option<SonarClient>,
    state: State,
    next_retry: Instant,
    backoff: Duration,
}
impl Service {
    /// Read-only diagnostics; never sent to the UI or saved as a connection setting.
    pub fn endpoint_for_diagnostics(&self) -> Option<String> {
        self.client.as_ref().map(|c| c.base_url().to_string())
    }
    /// Reconstruct a previously discovered cache for a read-only recovery probe.
    /// This is never exposed through UI actions or stored connection settings.
    #[doc(hidden)]
    pub fn resume_cache_for_diagnostics(endpoint: &str) -> Result<Self> {
        let mut service = Self::new()?;
        let url = url::Url::parse(endpoint).map_err(|error| Error::Other(error.to_string()))?;
        service.client = Some(SonarClient::new(url)?);
        Ok(service)
    }
    pub fn new() -> Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| Error::Other(e.to_string()))?,
            client: None,
            state: State::default(),
            next_retry: Instant::now(),
            backoff: Duration::from_secs(1),
        })
    }
    fn failed(&mut self, error: Error) {
        let connection = match error {
            Error::SonarDisabled => "disabled",
            Error::SonarNotReady | Error::SonarNotRunning => "waiting",
            Error::UnexpectedApi { .. } => "incompatible",
            _ => "disconnected",
        };
        self.state = State {
            connection: connection.into(),
            message: Some(format!(
                "{error}{}",
                error.detail().map(|d| format!(" {d}")).unwrap_or_default()
            )),
            ..State::default()
        };
        self.client = None;
        self.next_retry = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(Duration::from_secs(30));
    }
    pub fn refresh(&mut self, force: bool) -> State {
        self.refresh_with_options(force, &DiscoveryOptions::resolve(None))
    }
    fn refresh_with_options(&mut self, force: bool, options: &DiscoveryOptions) -> State {
        if force {
            self.client = None;
            self.next_retry = Instant::now();
            self.backoff = Duration::from_secs(1);
        }
        if self.client.is_none() && Instant::now() < self.next_retry {
            return self.state.clone();
        }
        let result = self.runtime.block_on(async {
            if self.client.is_none() {
                self.client = Some(SonarClient::discover(options).await?);
            }
            read_state(self.client.as_ref().expect("discovered")).await
        });
        match result {
            Ok(state) => {
                self.state = state;
                self.backoff = Duration::from_secs(1);
            }
            Err(error) => self.failed(error),
        }
        self.state.clone()
    }
    /// Idle windows do not poll healthy audio state; disconnected/unsupported
    /// installations still recover in the background at a low frequency.
    pub fn maintain_connection(&mut self) {
        if !self.state.available() {
            self.refresh(false);
        }
    }
    /// Slider updates need only mode and volume readback, not device/app enumeration.
    /// Never queue/replay a write after disconnect or an intervening external change.
    pub fn set_volume_live(
        &mut self,
        channel: MixerChannel,
        level: f64,
        expected: f64,
    ) -> Result<f64> {
        let result = self.runtime.block_on(async {
            let client = self
                .client
                .as_ref()
                .ok_or_else(|| Error::Other("Sonar is disconnected".into()))?;
            if client.get_json("mode").await?.as_str() != Some("classic") {
                return Err(Error::Other("Sonar Classic mode is not ready".into()));
            }
            let before = client
                .volumes()
                .await?
                .into_iter()
                .find(|v| v.channel == channel)
                .ok_or_else(|| Error::unexpected("channel missing"))?;
            if !expected.is_finite() || (before.volume - expected).abs() > 1e-6 {
                return Err(Error::Other(
                    "Volume changed externally; refresh before retrying".into(),
                ));
            }
            client.set_volume(channel, level).await?;
            Ok(level)
        });
        if result.as_ref().is_err_and(|e| e.is_stale_connection()) {
            self.client = None;
            self.next_retry = Instant::now();
        }
        if let Ok(level) = result
            && let Some(volume) = self.state.volumes.iter_mut().find(|v| v.channel == channel)
        {
            volume.volume = level;
        }
        result
    }
    pub fn execute(&mut self, command: Command) -> Result<()> {
        // Re-read mode and values before writing; never replay a mutation on reconnect.
        let result = self.runtime.block_on(async {
            let client = self.client.as_ref().ok_or_else(|| {
                Error::Other("Sonar is disconnected; retry connection first".into())
            })?;
            let state = read_state(client).await?;
            if !state.available() {
                return Err(Error::Other(
                    "This Sonar mode is not supported by Jack yet".into(),
                ));
            }
            match command {
                Command::Volume {
                    channel,
                    level,
                    expected,
                } => {
                    let current = state
                        .volumes
                        .iter()
                        .find(|v| v.channel == channel)
                        .ok_or_else(|| Error::unexpected("channel missing"))?;
                    if !expected.is_finite() || (current.volume - expected).abs() > 1e-6 {
                        return Err(Error::Other(
                            "Volume changed externally; refresh before retrying".into(),
                        ));
                    }
                    client.set_volume(channel, level).await?;
                }
                Command::Mute {
                    channel,
                    muted,
                    expected,
                } => {
                    if !state
                        .volumes
                        .iter()
                        .any(|v| v.channel == channel && v.muted == expected)
                    {
                        return Err(Error::Other(
                            "Mute changed externally; refresh before retrying".into(),
                        ));
                    }
                    client.set_muted(channel, muted).await?;
                }
                Command::Route {
                    channel,
                    device_id,
                    expected,
                } => {
                    if !state
                        .routes
                        .iter()
                        .any(|r| r.channel == channel && r.device_id == expected)
                    {
                        return Err(Error::Other(
                            "Output changed externally; refresh before retrying".into(),
                        ));
                    }
                    if !state
                        .devices
                        .iter()
                        .any(|d| d.id == device_id && d.enabled && d.role == channel.role())
                    {
                        return Err(Error::Other(
                            "Device disconnected or has wrong direction".into(),
                        ));
                    }
                    client.set_route(channel, &device_id).await?;
                }
                Command::Application {
                    process_id,
                    process_name,
                    channel,
                } => {
                    if channel == Channel::Microphone
                        || !state.applications.iter().any(|a| {
                            a.process_id == process_id
                                && a.process_name == process_name
                                && !a.routing_error
                        })
                    {
                        return Err(Error::ApplicationSessionStale { process_id });
                    }
                    client.set_application_route(process_id, channel).await?;
                }
                Command::Source {
                    channel,
                    enabled,
                    expected,
                } => {
                    let current = state.sources.iter().any(|source| source.channel == channel);
                    if current == enabled {
                        return Ok(());
                    }
                    if current != expected {
                        return Err(Error::Other(
                            "Channel availability changed externally".into(),
                        ));
                    }
                    client.set_optional_source_enabled(channel, enabled).await?;
                    let deadline = Instant::now() + Duration::from_secs(5);
                    loop {
                        let actual = read_state(client).await?;
                        if actual.available()
                            && actual
                                .sources
                                .iter()
                                .any(|source| source.channel == channel)
                                == enabled
                        {
                            break;
                        }
                        if Instant::now() >= deadline {
                            return Err(Error::Other(
                                "Channel availability readback failed".into(),
                            ));
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
            Ok(())
        });
        if result.as_ref().is_err_and(|e| e.is_stale_connection()) {
            self.client = None;
            self.next_retry = Instant::now();
        }
        result
    }
}
async fn read_state(client: &SonarClient) -> Result<State> {
    let mode = client
        .get_json("mode")
        .await?
        .as_str()
        .ok_or_else(|| Error::unexpected("mode is not a string"))?
        .to_string();
    if mode != "classic" {
        return Ok(State {
            connection: "unsupported_mode".into(),
            message: Some(
                "Streamer mode requires separate mix controls; use Classic mode in GG for now."
                    .into(),
            ),
            mode: Some(mode),
            ..State::default()
        });
    }
    let volumes = client.volumes().await?;
    let all_devices = client.devices().await?;
    let devices: Vec<_> = all_devices
        .iter()
        .filter(|d| d.is_physical())
        .cloned()
        .collect();
    let mut routes = client.routes().await?;
    sonar::routing::resolve_route_names(&mut routes, &devices);
    let routing = sonar::applications::parse_application_routing(
        &client.get_json("AudioDeviceRouting").await?,
    )?;
    let sources = [Channel::Game, Channel::Chat, Channel::Media, Channel::Aux]
        .into_iter()
        .filter_map(|channel| {
            routing
                .target_device(channel)
                .ok()
                .filter(|id| {
                    all_devices
                        .iter()
                        .any(|d| d.id == *id && d.enabled && d.virtual_device)
                })
                .map(|id| Source {
                    channel,
                    endpoint_id: id.into(),
                })
        })
        .collect();
    let applications = routing.sessions;
    // Avoid publishing a snapshot assembled across a mode transition.
    if client.get_json("mode").await?.as_str() != Some(mode.as_str()) {
        return Err(Error::unexpected("Sonar mode changed while reading; retry"));
    }
    Ok(State {
        connection: "connected".into(),
        message: None,
        mode: Some(mode),
        volumes,
        routes,
        devices,
        applications,
        sources,
    })
}
