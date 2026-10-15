//! Parsing and URL construction for Sonar application audio-session routing.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::sonar::models::{ApplicationActivity, ApplicationRoute, ApplicationSession, Channel};
use crate::sonar::routing::encode_path_segment;

pub const APPLICATION_ROUTING_PATH: &str = "AudioDeviceRouting";

#[derive(Debug, Deserialize)]
struct RawRoutingDevice {
    #[serde(rename = "deviceId")]
    device_id: Option<String>,
    role: Option<String>,
    #[serde(rename = "dataFlow")]
    data_flow: Option<String>,
    #[serde(rename = "audioSessions")]
    audio_sessions: Option<Vec<RawAudioSession>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawAudioSession {
    #[serde(rename = "processName")]
    process_name: Option<String>,
    #[serde(rename = "processId")]
    process_id: Option<u32>,
    #[serde(rename = "isSystemSound")]
    is_system_sound: Option<bool>,
    state: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "routingErrorDetected")]
    routing_error_detected: Option<bool>,
}

#[derive(Debug, Clone)]
struct Candidate {
    session: RawAudioSession,
    channel: Option<Channel>,
    activity: ApplicationActivity,
}

/// Parsed process inventory plus Sonar's internal target device identifiers.
#[derive(Debug, Clone)]
pub struct ApplicationRouting {
    pub sessions: Vec<ApplicationSession>,
    targets: BTreeMap<Channel, String>,
}

impl ApplicationRouting {
    pub fn target_device(&self, channel: Channel) -> Result<&str> {
        if channel == Channel::Microphone {
            return Err(Error::InvalidArguments(
                "Applications can only be routed to output channels.".to_string(),
            ));
        }
        self.targets
            .get(&channel)
            .map(String::as_str)
            .ok_or_else(|| {
                Error::unexpected(format!(
                    "/AudioDeviceRouting did not report the {} virtual output",
                    channel.display_name()
                ))
            })
    }

    pub fn contains_process(&self, process_id: u32) -> bool {
        self.sessions
            .iter()
            .any(|session| session.process_id == process_id)
    }

    pub fn process_is_on(&self, process_id: u32, channel: Channel) -> bool {
        self.sessions.iter().any(|session| {
            session.process_id == process_id && session.route.channel() == Some(channel)
        })
    }
}

/// Parse `GET /AudioDeviceRouting` into one effective record per process.
pub fn parse_application_routing(value: &Value) -> Result<ApplicationRouting> {
    let devices: Vec<RawRoutingDevice> = serde_json::from_value(value.clone()).map_err(|err| {
        Error::unexpected(format!(
            "/AudioDeviceRouting returned an invalid payload: {err}"
        ))
    })?;

    let mut targets = BTreeMap::new();
    let mut candidates: HashMap<u32, Vec<Candidate>> = HashMap::new();

    for device in devices {
        if !device
            .data_flow
            .as_deref()
            .is_some_and(|flow| flow.eq_ignore_ascii_case("render"))
        {
            continue;
        }
        let channel = device.role.as_deref().and_then(output_channel_from_role);
        if let Some(channel) = channel
            && let Some(device_id) = device.device_id.clone()
            && targets.insert(channel, device_id).is_some()
        {
            return Err(Error::unexpected(format!(
                "/AudioDeviceRouting reported multiple {} virtual outputs",
                channel.display_name()
            )));
        }

        for session in device.audio_sessions.unwrap_or_default() {
            let Some(process_id) = session.process_id.filter(|process_id| *process_id != 0) else {
                continue;
            };
            if session.is_system_sound.unwrap_or(false)
                || session
                    .state
                    .as_deref()
                    .is_some_and(|state| state.eq_ignore_ascii_case("expired"))
            {
                continue;
            }
            let activity = activity_from_state(session.state.as_deref().unwrap_or_default());
            candidates.entry(process_id).or_default().push(Candidate {
                session,
                channel,
                activity,
            });
        }
    }

    let mut sessions = candidates
        .into_values()
        .filter_map(collapse_process)
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        activity_order(left.activity)
            .cmp(&activity_order(right.activity))
            .then_with(|| {
                left.label()
                    .to_ascii_lowercase()
                    .cmp(&right.label().to_ascii_lowercase())
            })
            .then_with(|| left.process_id.cmp(&right.process_id))
    });

    Ok(ApplicationRouting { sessions, targets })
}

fn collapse_process(candidates: Vec<Candidate>) -> Option<ApplicationSession> {
    let best_rank = candidates
        .iter()
        .map(|candidate| activity_rank(candidate.activity))
        .max()?;
    let current = candidates
        .iter()
        .filter(|candidate| activity_rank(candidate.activity) == best_rank)
        .collect::<Vec<_>>();
    let representative = current.first()?;
    let channels = current
        .iter()
        .filter_map(|candidate| candidate.channel)
        .collect::<BTreeSet<_>>();
    let has_unassigned = current.iter().any(|candidate| candidate.channel.is_none());
    let route = if channels.len() == 1 && !has_unassigned {
        ApplicationRoute::from_channel(*channels.first()?)?
    } else if channels.is_empty() {
        ApplicationRoute::Unassigned
    } else {
        ApplicationRoute::Multiple
    };

    Some(ApplicationSession {
        process_id: representative.session.process_id?,
        process_name: representative
            .session
            .process_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "Unknown".to_string()),
        display_name: representative
            .session
            .display_name
            .clone()
            .unwrap_or_default(),
        route,
        activity: representative.activity,
        routing_error: current
            .iter()
            .any(|candidate| candidate.session.routing_error_detected.unwrap_or(false)),
    })
}

fn output_channel_from_role(role: &str) -> Option<Channel> {
    match role.trim().to_ascii_lowercase().as_str() {
        "game" | "gaming" => Some(Channel::Game),
        "chat" | "chatrender" => Some(Channel::Chat),
        "media" | "music" => Some(Channel::Media),
        "aux" => Some(Channel::Aux),
        _ => None,
    }
}

fn activity_from_state(state: &str) -> ApplicationActivity {
    match state.trim().to_ascii_lowercase().as_str() {
        "active" => ApplicationActivity::Active,
        "inactive" => ApplicationActivity::Inactive,
        _ => ApplicationActivity::Unknown,
    }
}

fn activity_rank(activity: ApplicationActivity) -> u8 {
    match activity {
        ApplicationActivity::Active => 2,
        ApplicationActivity::Inactive | ApplicationActivity::Unknown => 1,
    }
}

fn activity_order(activity: ApplicationActivity) -> u8 {
    match activity {
        ApplicationActivity::Active => 0,
        ApplicationActivity::Inactive => 1,
        ApplicationActivity::Unknown => 2,
    }
}

pub fn set_application_route_path(channel_device_id: &str, process_id: u32) -> String {
    format!(
        "{APPLICATION_ROUTING_PATH}/render/{}/{process_id}",
        encode_path_segment(channel_device_id)
    )
}
