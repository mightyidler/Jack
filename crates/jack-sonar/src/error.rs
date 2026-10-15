//! Error types, exit codes and user-facing hints.

use std::path::PathBuf;

use thiserror::Error;

/// Convenience result alias used across the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Documented process exit codes.
pub mod exit_code {
    pub const SUCCESS: i32 = 0;
    pub const FAILURE: i32 = 1;
    pub const INVALID_ARGUMENTS: i32 = 2;
    pub const GG_UNAVAILABLE: i32 = 3;
    pub const SONAR_UNAVAILABLE: i32 = 4;
    pub const DEVICE_NOT_FOUND: i32 = 5;
    pub const AMBIGUOUS_DEVICE: i32 = 6;
    pub const UNEXPECTED_API: i32 = 7;
    pub const CONFIGURATION: i32 = 8;
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    InvalidArguments(String),

    #[error("Could not find SteelSeries GG coreProps.json.")]
    CorePropsNotFound { searched: Vec<PathBuf> },

    #[error("Could not read {path}.")]
    CorePropsUnreadable { path: PathBuf, detail: String },

    #[error("SteelSeries GG coreProps.json is not valid JSON.")]
    CorePropsInvalid { path: PathBuf, detail: String },

    #[error("SteelSeries GG coreProps.json does not contain a usable GG address.")]
    GgAddressMissing,

    #[error("Refusing to contact non-local SteelSeries endpoint {url}.")]
    NonLocalEndpoint { url: String },

    #[error("SteelSeries GG is not running.")]
    GgUnreachable { url: String, detail: String },

    #[error("SteelSeries GG is running, but Sonar is disabled.")]
    SonarDisabled,

    #[error("SteelSeries GG is running, but Sonar is not running.")]
    SonarNotRunning,

    #[error("Sonar is enabled but not ready.")]
    SonarNotReady,

    #[error("Sonar is running, but did not report an API address.")]
    SonarAddressMissing,

    #[error("Could not reach the Sonar API at {url}.")]
    SonarUnreachable { url: String, detail: String },

    #[error("SteelSeries Sonar returned an unexpected API response.")]
    UnexpectedApi { detail: String },

    #[error("Sonar does not expose a channel named {channel}.")]
    UnknownChannel { channel: String },

    #[error("No {role} device matches \"{query}\".")]
    DeviceNotFound { query: String, role: String },

    #[error("multiple devices match \"{query}\"")]
    AmbiguousDevice { query: String, matches: Vec<String> },

    #[error("No active application matches \"{query}\".")]
    ApplicationNotFound { query: String },

    #[error("multiple applications match \"{query}\"")]
    AmbiguousApplication { query: String, matches: Vec<String> },

    #[error("Application process {process_id} is no longer available.")]
    ApplicationSessionStale { process_id: u32 },

    #[error("Sonar did not apply the application route change for process {process_id}.")]
    ApplicationRouteVerificationFailed {
        process_id: u32,
        expected: String,
        actual: Option<String>,
    },

    #[error("Sonar did not apply the route change for {channel}.")]
    RouteVerificationFailed {
        channel: String,
        expected: String,
        actual: Option<String>,
    },

    #[error("Configuration error in {path}.")]
    Config { path: PathBuf, detail: String },

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Documented exit code for this error.
    pub fn exit_code(&self) -> i32 {
        use Error::*;
        match self {
            InvalidArguments(_) | UnknownChannel { .. } => exit_code::INVALID_ARGUMENTS,
            CorePropsNotFound { .. }
            | CorePropsUnreadable { .. }
            | CorePropsInvalid { .. }
            | GgAddressMissing
            | NonLocalEndpoint { .. }
            | GgUnreachable { .. } => exit_code::GG_UNAVAILABLE,
            SonarDisabled
            | SonarNotRunning
            | SonarNotReady
            | SonarAddressMissing
            | SonarUnreachable { .. } => exit_code::SONAR_UNAVAILABLE,
            UnexpectedApi { .. }
            | RouteVerificationFailed { .. }
            | ApplicationSessionStale { .. }
            | ApplicationRouteVerificationFailed { .. } => exit_code::UNEXPECTED_API,
            DeviceNotFound { .. } | ApplicationNotFound { .. } => exit_code::DEVICE_NOT_FOUND,
            AmbiguousDevice { .. } | AmbiguousApplication { .. } => exit_code::AMBIGUOUS_DEVICE,
            Config { .. } => exit_code::CONFIGURATION,
            Other(_) => exit_code::FAILURE,
        }
    }

    /// Extra actionable text shown below the error message.
    pub fn hint(&self) -> Option<String> {
        use Error::*;
        match self {
            CorePropsNotFound { searched } => {
                let mut hint = String::from(
                    "Make sure SteelSeries GG is installed and running, or point sonarctl at the \
                     file:\n\n  sonarctl --core-props \"C:\\path\\to\\coreProps.json\" status\n",
                );
                if !searched.is_empty() {
                    hint.push_str("\nSearched:\n");
                    for path in searched {
                        hint.push_str(&format!("  {}\n", path.display()));
                    }
                }
                Some(hint.trim_end().to_string())
            }
            GgUnreachable { .. } => {
                Some("Start SteelSeries GG and run:\n\n  sonarctl doctor".to_string())
            }
            SonarDisabled => {
                Some("Enable Sonar in SteelSeries GG and run:\n\n  sonarctl doctor".to_string())
            }
            SonarNotRunning | SonarNotReady => Some(
                "Open SteelSeries GG, wait for Sonar to finish starting, then run:\n\n  \
                 sonarctl doctor"
                    .to_string(),
            ),
            SonarUnreachable { .. } => Some(
                "Sonar may have restarted. Run `sonarctl doctor` to re-check the connection."
                    .to_string(),
            ),
            UnexpectedApi { .. }
            | RouteVerificationFailed { .. }
            | ApplicationRouteVerificationFailed { .. } => {
                Some("Run `sonarctl doctor -v` for details.".to_string())
            }
            AmbiguousDevice { matches, .. } | AmbiguousApplication { matches, .. } => {
                let mut hint = String::new();
                for name in matches {
                    hint.push_str(&format!("  {name}\n"));
                }
                hint.push_str("\nUse a more specific name.");
                Some(hint)
            }
            DeviceNotFound { .. } => {
                Some("Run `sonarctl devices` to list available devices.".to_string())
            }
            ApplicationNotFound { .. } | ApplicationSessionStale { .. } => {
                Some("Run `sonarctl apps` to list current application sessions.".to_string())
            }
            Config { .. } => Some("Fix the configuration file and try again.".to_string()),
            _ => None,
        }
    }

    /// Low-level detail, only shown in verbose mode.
    pub fn detail(&self) -> Option<String> {
        use Error::*;
        match self {
            CorePropsUnreadable { detail, .. }
            | CorePropsInvalid { detail, .. }
            | GgUnreachable { detail, .. }
            | SonarUnreachable { detail, .. }
            | UnexpectedApi { detail }
            | Config { detail, .. } => Some(detail.clone()),
            RouteVerificationFailed {
                expected, actual, ..
            } => Some(format!(
                "expected device {expected}, Sonar reports {}",
                actual.as_deref().unwrap_or("nothing")
            )),
            ApplicationRouteVerificationFailed {
                expected, actual, ..
            } => Some(format!(
                "expected channel {expected}, Sonar reports {}",
                actual.as_deref().unwrap_or("nothing")
            )),
            _ => None,
        }
    }

    /// Whether the failure suggests the Sonar endpoint moved or disappeared,
    /// which should trigger rediscovery and a single retry.
    pub fn is_stale_connection(&self) -> bool {
        matches!(self, Error::SonarUnreachable { .. })
    }

    /// Build an [`Error::UnexpectedApi`] from any displayable detail.
    pub fn unexpected(detail: impl std::fmt::Display) -> Self {
        Error::UnexpectedApi {
            detail: detail.to_string(),
        }
    }
}
