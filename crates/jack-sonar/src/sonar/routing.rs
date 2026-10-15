//! Translation between application channels and Sonar's `classicRedirections`
//! identifiers, plus route parsing and URL construction.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::sonar::models::{AudioDevice, Channel, Route, as_array};

/// Sonar's identifier for a channel. Kept in one place so a GG update only
/// requires editing this mapping.
pub fn channel_api_id(channel: Channel) -> &'static str {
    match channel {
        Channel::Game => "game",
        Channel::Chat => "chat",
        Channel::Media => "media",
        Channel::Aux => "aux",
        Channel::Microphone => "mic",
    }
}

/// Tolerant reverse mapping of a Sonar redirection id.
pub fn channel_from_api_id(id: &str) -> Option<Channel> {
    match id.trim().to_ascii_lowercase().as_str() {
        "game" | "gaming" => Some(Channel::Game),
        "chat" => Some(Channel::Chat),
        "media" => Some(Channel::Media),
        "aux" => Some(Channel::Aux),
        "mic" | "microphone" => Some(Channel::Microphone),
        _ => None,
    }
}

/// Parse Sonar's `/classicRedirections` payload.
pub fn parse_routes(value: &Value) -> Result<Vec<Route>> {
    let items = as_array(value, &["classicRedirections", "redirections", "items"])
        .ok_or_else(|| Error::unexpected("/classicRedirections did not return a list"))?;

    let mut routes = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(id) = object.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(channel) = channel_from_api_id(id) else {
            tracing::debug!(id, "ignoring unknown Sonar redirection id");
            continue;
        };
        let Some(device_id) = object.get("deviceId").and_then(Value::as_str) else {
            return Err(Error::unexpected(format!(
                "/classicRedirections route `{id}` did not contain a string deviceId"
            )));
        };

        routes.push(Route {
            channel,
            device_id: device_id.to_string(),
            device_name: None,
        });
    }

    for channel in Channel::ALL {
        let count = routes
            .iter()
            .filter(|route| route.channel == channel)
            .count();
        if count != 1 {
            return Err(Error::unexpected(format!(
                "/classicRedirections must contain exactly one `{}` route (found {count})",
                channel.as_str()
            )));
        }
    }

    routes.sort_by_key(|route| {
        Channel::ALL
            .iter()
            .position(|channel| *channel == route.channel)
            .unwrap_or(usize::MAX)
    });

    Ok(routes)
}

/// Return the exact identifier Sonar reported for an application channel.
pub fn route_api_id(value: &Value, channel: Channel) -> Result<String> {
    parse_routes(value)?;
    let items = as_array(value, &["classicRedirections", "redirections", "items"])
        .ok_or_else(|| Error::unexpected("/classicRedirections did not return a list"))?;

    items
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|object| object.get("id").and_then(Value::as_str))
        .find(|id| channel_from_api_id(id) == Some(channel))
        .map(str::to_string)
        .ok_or_else(|| {
            Error::unexpected(format!(
                "/classicRedirections did not contain a `{}` route",
                channel.as_str()
            ))
        })
}

/// Fill in device display names from the known device list.
pub fn resolve_route_names(routes: &mut [Route], devices: &[AudioDevice]) {
    for route in routes.iter_mut() {
        route.device_name = devices
            .iter()
            .find(|device| device.id == route.device_id)
            .map(|device| device.name.clone());
    }
}

/// Percent-encode a single URL path segment, keeping only unreserved characters.
pub fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Relative path used to read the current redirections.
pub const ROUTES_PATH: &str = "classicRedirections";

/// Relative path used to change one channel's device.
pub fn set_route_path(channel: Channel, device_id: &str) -> String {
    set_route_path_with_id(channel_api_id(channel), device_id)
}

/// Relative mutation path using the identifier observed in Sonar's response.
pub fn set_route_path_with_id(api_id: &str, device_id: &str) -> String {
    format!(
        "{ROUTES_PATH}/{}/deviceId/{}",
        encode_path_segment(api_id),
        encode_path_segment(device_id)
    )
}
