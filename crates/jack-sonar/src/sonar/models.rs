//! Stable, application-level Sonar models.
//!
//! Nothing in this module exposes SteelSeries' internal identifiers; the
//! translation lives in [`crate::sonar::routing`] and the parsing helpers below.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

/// A Sonar virtual channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Game,
    Chat,
    Media,
    Aux,
    Microphone,
}

impl Channel {
    /// All channels in display order.
    pub const ALL: [Channel; 5] = [
        Channel::Game,
        Channel::Chat,
        Channel::Media,
        Channel::Aux,
        Channel::Microphone,
    ];

    /// Stable lowercase name used by sonarctl (CLI + JSON output).
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Game => "game",
            Channel::Chat => "chat",
            Channel::Media => "media",
            Channel::Aux => "aux",
            Channel::Microphone => "microphone",
        }
    }

    /// Human-friendly name used in tables and the TUI.
    pub fn display_name(self) -> &'static str {
        match self {
            Channel::Game => "Game",
            Channel::Chat => "Chat",
            Channel::Media => "Media",
            Channel::Aux => "Aux",
            Channel::Microphone => "Microphone",
        }
    }

    /// Parse a user-supplied channel name, accepting documented aliases.
    pub fn parse(input: &str) -> Option<Channel> {
        match input.trim().to_ascii_lowercase().as_str() {
            "game" | "gaming" => Some(Channel::Game),
            "chat" => Some(Channel::Chat),
            "media" | "music" => Some(Channel::Media),
            "aux" => Some(Channel::Aux),
            "mic" | "microphone" => Some(Channel::Microphone),
            _ => None,
        }
    }

    /// Device role that can be routed to this channel.
    pub fn role(self) -> DeviceRole {
        match self {
            Channel::Microphone => DeviceRole::Capture,
            _ => DeviceRole::Playback,
        }
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

impl std::str::FromStr for Channel {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Channel::parse(s).ok_or_else(|| Error::UnknownChannel {
            channel: s.to_string(),
        })
    }
}

/// A Sonar mixer channel, including the master bus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MixerChannel {
    Master,
    Game,
    Chat,
    Media,
    Aux,
    Microphone,
}

impl MixerChannel {
    pub const ALL: [MixerChannel; 6] = [
        MixerChannel::Master,
        MixerChannel::Game,
        MixerChannel::Chat,
        MixerChannel::Media,
        MixerChannel::Aux,
        MixerChannel::Microphone,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MixerChannel::Master => "master",
            MixerChannel::Game => "game",
            MixerChannel::Chat => "chat",
            MixerChannel::Media => "media",
            MixerChannel::Aux => "aux",
            MixerChannel::Microphone => "microphone",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            MixerChannel::Master => "Master",
            MixerChannel::Game => "Game",
            MixerChannel::Chat => "Chat",
            MixerChannel::Media => "Media",
            MixerChannel::Aux => "Aux",
            MixerChannel::Microphone => "Microphone",
        }
    }

    pub fn api_id(self) -> &'static str {
        match self {
            MixerChannel::Master => "master",
            MixerChannel::Game => "game",
            MixerChannel::Chat => "chatRender",
            MixerChannel::Media => "media",
            MixerChannel::Aux => "aux",
            MixerChannel::Microphone => "chatCapture",
        }
    }

    pub fn parse(input: &str) -> Option<MixerChannel> {
        if input.trim().eq_ignore_ascii_case("master") {
            return Some(MixerChannel::Master);
        }
        Channel::parse(input).map(Into::into)
    }
}

impl From<Channel> for MixerChannel {
    fn from(channel: Channel) -> Self {
        match channel {
            Channel::Game => MixerChannel::Game,
            Channel::Chat => MixerChannel::Chat,
            Channel::Media => MixerChannel::Media,
            Channel::Aux => MixerChannel::Aux,
            Channel::Microphone => MixerChannel::Microphone,
        }
    }
}

impl std::fmt::Display for MixerChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

impl std::str::FromStr for MixerChannel {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        MixerChannel::parse(s).ok_or_else(|| Error::UnknownChannel {
            channel: s.to_string(),
        })
    }
}

/// Classic-mode volume and mute state for one mixer channel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct VolumeState {
    pub channel: MixerChannel,
    /// Sonar's native normalized value (`0.0..=1.0`).
    pub volume: f64,
    pub muted: bool,
}

impl VolumeState {
    pub fn percent(self) -> f64 {
        self.volume * 100.0
    }
}

/// Effective Sonar route for a Windows application process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplicationRoute {
    Game,
    Chat,
    Media,
    Aux,
    Unassigned,
    Multiple,
}

impl ApplicationRoute {
    pub fn from_channel(channel: Channel) -> Option<Self> {
        match channel {
            Channel::Game => Some(ApplicationRoute::Game),
            Channel::Chat => Some(ApplicationRoute::Chat),
            Channel::Media => Some(ApplicationRoute::Media),
            Channel::Aux => Some(ApplicationRoute::Aux),
            Channel::Microphone => None,
        }
    }

    pub fn channel(self) -> Option<Channel> {
        match self {
            ApplicationRoute::Game => Some(Channel::Game),
            ApplicationRoute::Chat => Some(Channel::Chat),
            ApplicationRoute::Media => Some(Channel::Media),
            ApplicationRoute::Aux => Some(Channel::Aux),
            ApplicationRoute::Unassigned | ApplicationRoute::Multiple => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ApplicationRoute::Game => "game",
            ApplicationRoute::Chat => "chat",
            ApplicationRoute::Media => "media",
            ApplicationRoute::Aux => "aux",
            ApplicationRoute::Unassigned => "unassigned",
            ApplicationRoute::Multiple => "multiple",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ApplicationRoute::Game => "Game",
            ApplicationRoute::Chat => "Chat",
            ApplicationRoute::Media => "Media",
            ApplicationRoute::Aux => "Aux",
            ApplicationRoute::Unassigned => "Unassigned",
            ApplicationRoute::Multiple => "Multiple",
        }
    }
}

/// Whether a Windows audio session is currently producing sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplicationActivity {
    Active,
    Inactive,
    Unknown,
}

impl ApplicationActivity {
    pub fn as_str(self) -> &'static str {
        match self {
            ApplicationActivity::Active => "active",
            ApplicationActivity::Inactive => "inactive",
            ApplicationActivity::Unknown => "unknown",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ApplicationActivity::Active => "Active",
            ApplicationActivity::Inactive => "Idle",
            ApplicationActivity::Unknown => "Unknown",
        }
    }
}

/// One running Windows process with a non-expired render audio session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApplicationSession {
    pub process_id: u32,
    pub process_name: String,
    pub display_name: String,
    pub route: ApplicationRoute,
    pub activity: ApplicationActivity,
    pub routing_error: bool,
}

impl ApplicationSession {
    pub fn label(&self) -> &str {
        if self.display_name.is_empty() {
            &self.process_name
        } else {
            &self.display_name
        }
    }
}

/// Direction of an audio endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
    Playback,
    Capture,
    Unknown,
}

impl DeviceRole {
    /// Column label used by `sonarctl devices`.
    pub fn label(self) -> &'static str {
        match self {
            DeviceRole::Playback => "Playback",
            DeviceRole::Capture => "Capture",
            DeviceRole::Unknown => "Unknown",
        }
    }

    /// Tolerant mapping of Sonar's `dataFlow` field.
    pub fn from_data_flow(value: &str) -> DeviceRole {
        match value.trim().to_ascii_lowercase().as_str() {
            "render" | "playback" | "output" => DeviceRole::Playback,
            "capture" | "record" | "input" => DeviceRole::Capture,
            _ => DeviceRole::Unknown,
        }
    }

    /// Whether a device with role `self` may be routed where `wanted` is expected.
    /// Devices with an unknown role are never silently accepted.
    pub fn accepts(self, wanted: DeviceRole) -> bool {
        self == wanted
    }
}

impl std::fmt::Display for DeviceRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A physical (or virtual) audio endpoint known to Sonar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub role: DeviceRole,
    pub enabled: bool,
    /// Sonar's own virtual endpoints (`isVad`). Never listed as physical devices.
    #[serde(skip)]
    pub virtual_device: bool,
}

impl AudioDevice {
    pub fn is_physical(&self) -> bool {
        !self.virtual_device
    }
}

/// A resolved channel → device routing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Route {
    pub channel: Channel,
    pub device_id: String,
    pub device_name: Option<String>,
}

impl Route {
    /// Device name if known, otherwise a readable placeholder.
    pub fn display_device(&self) -> String {
        match &self.device_name {
            Some(name) => name.clone(),
            None if self.device_id.is_empty() => "(none)".to_string(),
            None => format!("(unknown device {})", self.device_id),
        }
    }
}

/// Tolerant representation of one `/audioDevices` entry.
#[derive(Debug, Clone, Deserialize)]
struct RawAudioDevice {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "friendlyName")]
    friendly_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "dataFlow")]
    data_flow: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default, rename = "isVad")]
    is_vad: Option<bool>,
}

/// Parse Sonar's `/audioDevices` payload.
///
/// The payload is normally a JSON array, but object wrappers are tolerated.
/// Unknown fields are ignored and malformed entries are skipped unless none
/// of a non-empty payload remain usable.
pub fn parse_devices(value: &Value) -> Result<Vec<AudioDevice>> {
    let items = as_array(value, &["devices", "audioDevices", "items"])
        .ok_or_else(|| Error::unexpected("/audioDevices did not return a list of devices"))?;

    let mut devices = Vec::new();
    for item in items {
        let raw: RawAudioDevice = match serde_json::from_value(item.clone()) {
            Ok(raw) => raw,
            Err(err) => {
                tracing::debug!(error = %err, "skipping unparsable audio device entry");
                continue;
            }
        };
        let Some(id) = raw.id.filter(|id| !id.is_empty()) else {
            tracing::debug!("skipping audio device entry without id");
            continue;
        };
        let name = raw
            .friendly_name
            .or(raw.name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| id.clone());
        let role = raw
            .data_flow
            .as_deref()
            .map(DeviceRole::from_data_flow)
            .unwrap_or(DeviceRole::Unknown);
        let enabled = raw
            .state
            .as_deref()
            .map(|state| state.eq_ignore_ascii_case("active"))
            .unwrap_or(true);

        devices.push(AudioDevice {
            id,
            name,
            role,
            enabled,
            virtual_device: raw.is_vad.unwrap_or(false),
        });
    }

    if !items.is_empty() && devices.is_empty() {
        return Err(Error::unexpected(
            "/audioDevices did not contain any device with a usable id",
        ));
    }

    Ok(devices)
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct RawVolumeState {
    volume: f64,
    muted: bool,
}

#[derive(Debug, Deserialize)]
struct RawVolumeContainer {
    classic: RawVolumeState,
}

#[derive(Debug, Deserialize)]
struct RawDeviceVolumes {
    game: RawVolumeContainer,
    #[serde(rename = "chatRender")]
    chat: RawVolumeContainer,
    media: RawVolumeContainer,
    aux: RawVolumeContainer,
    #[serde(rename = "chatCapture")]
    microphone: RawVolumeContainer,
}

#[derive(Debug, Deserialize)]
struct RawClassicVolumes {
    masters: RawVolumeContainer,
    devices: RawDeviceVolumes,
}

/// Parse the strict fields required from `GET /volumeSettings/classic`.
pub fn parse_classic_volumes(value: &Value) -> Result<Vec<VolumeState>> {
    let raw: RawClassicVolumes = serde_json::from_value(value.clone()).map_err(|err| {
        Error::unexpected(format!(
            "/volumeSettings/classic returned an invalid volume payload: {err}"
        ))
    })?;
    let values = [
        (MixerChannel::Master, raw.masters.classic),
        (MixerChannel::Game, raw.devices.game.classic),
        (MixerChannel::Chat, raw.devices.chat.classic),
        (MixerChannel::Media, raw.devices.media.classic),
        (MixerChannel::Aux, raw.devices.aux.classic),
        (MixerChannel::Microphone, raw.devices.microphone.classic),
    ];

    values
        .into_iter()
        .map(|(channel, state)| {
            if !state.volume.is_finite() || !(0.0..=1.0).contains(&state.volume) {
                return Err(Error::unexpected(format!(
                    "/volumeSettings/classic reported an invalid {} volume: {}",
                    channel.as_str(),
                    state.volume
                )));
            }
            Ok(VolumeState {
                channel,
                volume: state.volume,
                muted: state.muted,
            })
        })
        .collect()
}

/// Extract an array from a value that may be an array or an object wrapper.
pub(crate) fn as_array<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Vec<Value>> {
    if let Some(array) = value.as_array() {
        return Some(array);
    }
    let object = value.as_object()?;
    for key in keys {
        if let Some(array) = object.get(*key).and_then(Value::as_array) {
            return Some(array);
        }
    }
    None
}
