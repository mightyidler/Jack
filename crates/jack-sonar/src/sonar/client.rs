//! HTTP clients for the SteelSeries GG and Sonar local APIs.

use std::time::Duration;

use serde_json::Value;
use url::Url;

use crate::error::{Error, Result};
use crate::sonar::applications::{
    APPLICATION_ROUTING_PATH, parse_application_routing, set_application_route_path,
};
use crate::sonar::discovery::{
    self, DiscoveryOptions, SonarSubApp, sonar_base_url, validate_local_url,
};
use crate::sonar::models::{
    ApplicationSession, AudioDevice, Channel, MixerChannel, Route, VolumeState,
    parse_classic_volumes, parse_devices,
};
use crate::sonar::routing::{ROUTES_PATH, parse_routes, route_api_id, set_route_path_with_id};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CLASSIC_VOLUMES_PATH: &str = "volumeSettings/classic";
const VOLUME_VERIFICATION_TOLERANCE: f64 = 1e-6;

fn ensure_crypto_provider() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// HTTP client for the local GG endpoint.
///
/// GG serves its encrypted endpoint with a self-signed certificate, so
/// certificate validation is relaxed — but only after the endpoint has been
/// proven to be a loopback address, and only for this dedicated client.
pub fn build_gg_client(base_url: &Url) -> Result<reqwest::Client> {
    validate_local_url(base_url)?;
    ensure_crypto_provider();
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .tls_danger_accept_invalid_certs(true)
        .build()
        .map_err(|err| Error::Other(format!("could not create the GG HTTP client: {err}")))
}

/// Strict HTTP client used for Sonar's plain-HTTP local API.
pub fn build_sonar_client() -> Result<reqwest::Client> {
    ensure_crypto_provider();
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| Error::Other(format!("could not create the Sonar HTTP client: {err}")))
}

/// Query GG's `/subApps` endpoint.
pub async fn fetch_sub_apps(client: &reqwest::Client, gg_base_url: &Url) -> Result<SonarSubApp> {
    validate_local_url(gg_base_url)?;
    let url = join(gg_base_url, "subApps")?;
    tracing::debug!(%url, "querying GG sub-apps");

    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|err| Error::GgUnreachable {
            url: url.to_string(),
            detail: err.to_string(),
        })?;

    if !response.status().is_success() {
        return Err(Error::GgUnreachable {
            url: url.to_string(),
            detail: format!("GG returned HTTP {}", response.status()),
        });
    }

    let value: Value = response
        .json()
        .await
        .map_err(|err| Error::unexpected(format!("/subApps returned invalid JSON: {err}")))?;

    discovery::parse_sub_apps(&value)
}

/// Full discovery chain: coreProps.json → GG `/subApps` → Sonar base URL.
pub async fn discover_sonar_url(options: &DiscoveryOptions) -> Result<Url> {
    let (path, props) = discovery::load_core_props(options)?;
    tracing::debug!(core_props = %path.display(), "using coreProps.json");

    let gg_url = props.gg_base_url()?;
    tracing::debug!(%gg_url, "resolved GG endpoint");

    let client = build_gg_client(&gg_url)?;
    let sub_app = fetch_sub_apps(&client, &gg_url).await?;
    let sonar_url = sonar_base_url(&sub_app)?;
    tracing::debug!(%sonar_url, "resolved Sonar endpoint");

    Ok(sonar_url)
}

/// Client for Sonar's local HTTP API.
#[derive(Debug, Clone)]
pub struct SonarClient {
    base_url: Url,
    http: reqwest::Client,
}

impl SonarClient {
    /// Build a client for an already discovered Sonar endpoint.
    pub fn new(base_url: Url) -> Result<Self> {
        validate_local_url(&base_url)?;
        Ok(SonarClient {
            base_url,
            http: build_sonar_client()?,
        })
    }

    /// Discover Sonar and connect to it.
    pub async fn discover(options: &DiscoveryOptions) -> Result<Self> {
        let url = discover_sonar_url(options).await?;
        SonarClient::new(url)
    }

    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub(crate) async fn get_json(&self, path: &str) -> Result<Value> {
        let url = join(&self.base_url, path)?;
        tracing::debug!(%url, "GET");

        let response =
            self.http
                .get(url.clone())
                .send()
                .await
                .map_err(|err| Error::SonarUnreachable {
                    url: url.to_string(),
                    detail: err.to_string(),
                })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let folded = body.to_ascii_lowercase();
            if folded.contains("current mode") || folded.contains("cannot be called") {
                return Err(Error::unexpected(format!(
                    "GET {url} is unavailable in Sonar's current mode"
                )));
            }
            if status.is_server_error() {
                return Err(Error::SonarUnreachable {
                    url: url.to_string(),
                    detail: format!("Sonar returned HTTP {status}"),
                });
            }
            return Err(Error::unexpected(format!(
                "GET {url} returned HTTP {status}"
            )));
        }

        response
            .json()
            .await
            .map_err(|err| Error::unexpected(format!("GET {url} returned invalid JSON: {err}")))
    }

    async fn put_empty(&self, path: &str) -> Result<()> {
        let url = join(&self.base_url, path)?;
        tracing::debug!(%url, "PUT");

        let response =
            self.http
                .put(url.clone())
                .send()
                .await
                .map_err(|err| Error::SonarUnreachable {
                    url: url.to_string(),
                    detail: err.to_string(),
                })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let folded = body.to_ascii_lowercase();
            if folded.contains("current mode") || folded.contains("cannot be called") {
                return Err(Error::unexpected(format!(
                    "PUT {url} is unavailable in Sonar's current mode"
                )));
            }
            if status.is_server_error() {
                return Err(Error::SonarUnreachable {
                    url: url.to_string(),
                    detail: format!("Sonar returned HTTP {status}"),
                });
            }
            return Err(Error::unexpected(format!(
                "PUT {url} returned HTTP {status}"
            )));
        }
        Ok(())
    }

    /// Every endpoint Sonar knows about, including its own virtual devices.
    pub async fn devices(&self) -> Result<Vec<AudioDevice>> {
        let value = self.get_json("audioDevices").await?;
        parse_devices(&value)
    }

    /// Current channel routing, without resolved device names.
    pub async fn routes(&self) -> Result<Vec<Route>> {
        let value = self.get_json(ROUTES_PATH).await?;
        parse_routes(&value)
    }

    pub async fn volumes(&self) -> Result<Vec<VolumeState>> {
        let value = self.get_json(CLASSIC_VOLUMES_PATH).await?;
        parse_classic_volumes(&value)
    }

    pub async fn applications(&self) -> Result<Vec<ApplicationSession>> {
        let value = self.get_json(APPLICATION_ROUTING_PATH).await?;
        Ok(parse_application_routing(&value)?.sessions)
    }

    pub async fn set_application_route(&self, process_id: u32, channel: Channel) -> Result<()> {
        let current = parse_application_routing(&self.get_json(APPLICATION_ROUTING_PATH).await?)?;
        if !current.contains_process(process_id) {
            return Err(Error::ApplicationSessionStale { process_id });
        }
        let target = current.target_device(channel)?.to_string();
        self.put_empty(&set_application_route_path(&target, process_id))
            .await?;

        // Session recreation is asynchronous. Poll readback, never replay the PUT.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let actual = loop {
            let actual =
                parse_application_routing(&self.get_json(APPLICATION_ROUTING_PATH).await?)?;
            if actual.process_is_on(process_id, channel) {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                break actual;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        if !actual.contains_process(process_id) {
            return Err(Error::ApplicationSessionStale { process_id });
        }
        let route = actual
            .sessions
            .iter()
            .find(|session| session.process_id == process_id)
            .map(|session| session.route.display_name().to_string());
        Err(Error::ApplicationRouteVerificationFailed {
            process_id,
            expected: channel.display_name().to_string(),
            actual: route,
        })
    }

    pub async fn set_volume(&self, channel: MixerChannel, volume: f64) -> Result<()> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(Error::InvalidArguments(
                "Volume must be between 0 and 100 percent.".to_string(),
            ));
        }
        self.put_empty(&format!(
            "{CLASSIC_VOLUMES_PATH}/{}/Volume/{volume}",
            channel.api_id()
        ))
        .await?;

        let actual = self
            .volumes()
            .await?
            .into_iter()
            .find(|state| state.channel == channel)
            .ok_or_else(|| {
                Error::unexpected(format!(
                    "Sonar did not report {} volume after the change",
                    channel.display_name()
                ))
            })?;
        // Sonar stores the value as a 32-bit float, so values such as 0.3 do
        // not round-trip exactly when decoded into Rust's f64.
        if (actual.volume - volume).abs() <= VOLUME_VERIFICATION_TOLERANCE {
            Ok(())
        } else {
            Err(Error::unexpected(format!(
                "Sonar did not apply the {} volume change: expected {:.2}%, got {:.2}%",
                channel.display_name(),
                volume * 100.0,
                actual.percent()
            )))
        }
    }

    pub async fn set_muted(&self, channel: MixerChannel, muted: bool) -> Result<()> {
        self.put_empty(&format!(
            "{CLASSIC_VOLUMES_PATH}/{}/Mute/{muted}",
            channel.api_id()
        ))
        .await?;

        let actual = self
            .volumes()
            .await?
            .into_iter()
            .find(|state| state.channel == channel)
            .ok_or_else(|| {
                Error::unexpected(format!(
                    "Sonar did not report {} mute state after the change",
                    channel.display_name()
                ))
            })?;
        if actual.muted == muted {
            Ok(())
        } else {
            Err(Error::unexpected(format!(
                "Sonar did not apply the {} mute change",
                channel.display_name()
            )))
        }
    }

    /// Point a channel at a device, then verify Sonar applied the change.
    pub async fn set_route(&self, channel: Channel, device_id: &str) -> Result<()> {
        let current_routes = self.get_json(ROUTES_PATH).await?;
        let api_id = route_api_id(&current_routes, channel)?;
        let path = set_route_path_with_id(&api_id, device_id);
        tracing::debug!(channel = channel.as_str(), "setting route");
        self.put_empty(&path).await?;

        let routes = self.routes().await?;
        let actual = routes
            .iter()
            .find(|route| route.channel == channel)
            .map(|route| route.device_id.clone());

        if actual.as_deref() == Some(device_id) {
            Ok(())
        } else {
            Err(Error::RouteVerificationFailed {
                channel: channel.display_name().to_string(),
                expected: device_id.to_string(),
                actual,
            })
        }
    }
    /// GG121's own frontend uses this endpoint for its optional channel controls.
    pub async fn set_optional_source_enabled(&self, channel: Channel, enabled: bool) -> Result<()> {
        if !matches!(channel, Channel::Media | Channel::Aux) {
            return Err(Error::Other(
                "Only optional Media/Aux sources may be changed".into(),
            ));
        }
        self.put_empty(&format!(
            "audioInitialization/{}/isEnabled/{enabled}",
            channel.as_str()
        ))
        .await
    }
}

/// Join a relative path onto a validated base URL.
fn join(base: &Url, path: &str) -> Result<Url> {
    let base_text = base.as_str().trim_end_matches('/').to_string();
    let url = Url::parse(&format!("{base_text}/{path}"))
        .map_err(|err| Error::Other(format!("could not build request URL: {err}")))?;
    validate_local_url(&url)?;
    Ok(url)
}
