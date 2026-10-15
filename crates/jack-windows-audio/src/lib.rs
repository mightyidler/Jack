#[cfg(windows)]
mod platform;
#[cfg(windows)]
pub use platform::*;

#[cfg(windows)]
mod bridge;
#[cfg(windows)]
pub use bridge::{BridgeStats, bridge_lab};

#[cfg(windows)]
mod monitor;
#[cfg(windows)]
pub use monitor::{AudioMonitor, MonitorDiagnostics};

#[cfg(windows)]
mod fixture;
#[cfg(windows)]
pub use fixture::{EventVerification, verify_session_events};

#[cfg(windows)]
pub mod policy;

#[cfg(windows)]
pub mod volume;

#[cfg(windows)]
pub mod pcm_proof;
#[cfg(windows)]
pub mod sonar_probe;

#[cfg(windows)]
pub mod control;

#[cfg(windows)]
pub mod transport;

#[cfg(windows)]
pub mod virtual_devices;

#[cfg(windows)]
pub mod sources;

#[cfg(windows)]
pub mod engine;
#[cfg(windows)]
pub mod identity;
