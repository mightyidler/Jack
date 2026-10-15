//! Runtime discovery of the SteelSeries GG endpoint and the Sonar sub-app.
//!
//! Nothing here is ever persisted: ports are dynamic and must be rediscovered.

use std::path::{Path, PathBuf};

use serde_json::Value;
use url::{Host, Url};

use crate::error::{Error, Result};
use crate::platform;

/// Environment variable that overrides `coreProps.json` discovery.
pub const CORE_PROPS_ENV: &str = "JACK_SONAR_CORE_PROPS";

/// How sonarctl should locate SteelSeries GG.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryOptions {
    /// Explicit `coreProps.json` path (CLI override or environment variable).
    pub core_props: Option<PathBuf>,
}

impl DiscoveryOptions {
    /// CLI override first, then `SONARCTL_CORE_PROPS`, then auto-discovery.
    pub fn resolve(cli_override: Option<PathBuf>) -> Self {
        let core_props = cli_override.or_else(|| {
            std::env::var(CORE_PROPS_ENV)
                .ok()
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        });
        DiscoveryOptions { core_props }
    }
}

/// Tolerantly parsed `coreProps.json`.
#[derive(Debug, Clone, Default)]
pub struct CoreProps {
    /// Encrypted (HTTPS) GG address, e.g. `127.0.0.1:6327`.
    pub gg_encrypted_address: Option<String>,
    /// Plain HTTP GG address, when exposed.
    pub gg_address: Option<String>,
    /// Legacy engine address.
    pub address: Option<String>,
}

impl CoreProps {
    /// Preferred GG base URL (HTTPS when an encrypted address is available).
    pub fn gg_base_url(&self) -> Result<Url> {
        if let Some(address) = self.gg_encrypted_address.as_deref() {
            return parse_local_url("https", address);
        }
        if let Some(address) = self.gg_address.as_deref() {
            return parse_local_url("http", address);
        }
        Err(Error::GgAddressMissing)
    }
}

/// Parse `coreProps.json`, ignoring unknown properties.
pub fn parse_core_props(text: &str) -> std::result::Result<CoreProps, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "coreProps.json is not a JSON object".to_string())?;

    let string_field = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };

    let props = CoreProps {
        gg_encrypted_address: string_field("ggEncryptedAddress")
            .or_else(|| string_field("encryptedAddress")),
        gg_address: string_field("ggAddress"),
        address: string_field("address"),
    };

    if props.gg_encrypted_address.is_none() && props.gg_address.is_none() {
        return Err("coreProps.json does not contain a GG address".to_string());
    }

    Ok(props)
}

/// Locate `coreProps.json` using the documented priority order.
pub fn locate_core_props(options: &DiscoveryOptions) -> Result<PathBuf> {
    if let Some(path) = &options.core_props {
        if path.is_file() {
            return Ok(path.clone());
        }
        return Err(Error::CorePropsNotFound {
            searched: vec![path.clone()],
        });
    }

    let candidates = platform::core_props_candidates();
    for candidate in &candidates {
        if candidate.is_file() {
            tracing::debug!(path = %candidate.display(), "found coreProps.json");
            return Ok(candidate.clone());
        }
    }

    Err(Error::CorePropsNotFound {
        searched: candidates,
    })
}

/// Read and parse `coreProps.json` from disk.
pub fn read_core_props(path: &Path) -> Result<CoreProps> {
    let text = std::fs::read_to_string(path).map_err(|err| Error::CorePropsUnreadable {
        path: path.to_path_buf(),
        detail: err.to_string(),
    })?;
    parse_core_props(&text).map_err(|detail| Error::CorePropsInvalid {
        path: path.to_path_buf(),
        detail,
    })
}

/// Locate, read and parse `coreProps.json`.
pub fn load_core_props(options: &DiscoveryOptions) -> Result<(PathBuf, CoreProps)> {
    let path = locate_core_props(options)?;
    let props = read_core_props(&path)?;
    Ok((path, props))
}

/// State of the Sonar sub-app as reported by GG.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SonarSubApp {
    pub enabled: bool,
    pub running: bool,
    pub ready: bool,
    pub web_server_address: Option<String>,
}

/// Parse the GG `/subApps` payload, tolerating extra fields and wrappers.
pub fn parse_sub_apps(value: &Value) -> Result<SonarSubApp> {
    let sonar = value
        .get("subApps")
        .and_then(|apps| apps.get("sonar"))
        .or_else(|| value.get("sonar"))
        .ok_or_else(|| Error::unexpected("/subApps did not contain a `sonar` entry"))?;

    let flag = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| sonar.get(*key).and_then(Value::as_bool))
            .unwrap_or(false)
    };

    let metadata = sonar.get("metadata");
    let web_server_address = ["webServerAddress", "webServerAdress", "address"]
        .iter()
        .find_map(|key| {
            metadata
                .and_then(|meta| meta.get(*key))
                .or_else(|| sonar.get(*key))
                .and_then(Value::as_str)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    Ok(SonarSubApp {
        enabled: flag(&["isEnabled", "enabled"]),
        running: flag(&["isRunning", "running"]),
        ready: flag(&["isReady", "ready"]),
        web_server_address,
    })
}

/// Validate Sonar's reported state and return its local API base URL.
pub fn sonar_base_url(sub_app: &SonarSubApp) -> Result<Url> {
    if !sub_app.enabled {
        return Err(Error::SonarDisabled);
    }
    if !sub_app.running {
        return Err(Error::SonarNotRunning);
    }
    if !sub_app.ready {
        return Err(Error::SonarNotReady);
    }
    let address = sub_app
        .web_server_address
        .as_deref()
        .ok_or(Error::SonarAddressMissing)?;

    parse_local_url("http", address).map_err(|err| match err {
        Error::NonLocalEndpoint { url } => {
            Error::unexpected(format!("Sonar reported a non-local API endpoint: {url}"))
        }
        other => other,
    })
}

/// Parse an address (`host:port` or full URL) and require it to be local.
pub fn parse_local_url(default_scheme: &str, address: &str) -> Result<Url> {
    let address = address.trim().trim_end_matches('/');
    let candidate = if address.contains("://") {
        address.to_string()
    } else {
        format!("{default_scheme}://{address}")
    };

    let url = Url::parse(&candidate).map_err(|_| Error::NonLocalEndpoint {
        url: address.to_string(),
    })?;
    validate_local_url(&url)?;
    Ok(url)
}

/// Reject anything that is not a loopback SteelSeries endpoint.
pub fn validate_local_url(url: &Url) -> Result<()> {
    let scheme_ok = matches!(url.scheme(), "http" | "https");
    let host_ok = match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip == std::net::Ipv4Addr::LOCALHOST,
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };

    if scheme_ok
        && host_ok
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
    {
        Ok(())
    } else {
        Err(Error::NonLocalEndpoint {
            url: url.to_string(),
        })
    }
}
