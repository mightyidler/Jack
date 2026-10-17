//! Stable IPC codes keep technical details out of translated product copy.
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    Connection,
    ClassicRequired,
    ExternalChange,
    DeviceUnavailable,
    SettingsSave,
    Recovery,
    Startup,
    Shortcut,
    Busy,
    SessionChanged,
    Verification,
    Internal,
}

#[derive(Debug, Serialize)]
pub struct UserError {
    pub code: Code,
    pub detail: String,
}

impl From<String> for UserError {
    fn from(detail: String) -> Self {
        let text = detail.to_ascii_lowercase();
        let code = if text.contains("recovery") || text.contains("restore audio routes") {
            Code::Recovery
        } else if text.contains("settings save failed") {
            Code::SettingsSave
        } else if text.contains("startup") {
            Code::Startup
        } else if text.contains("shortcut") || text.contains("hotkey") {
            Code::Shortcut
        } else if text.contains("externally")
            || text.contains("changed before")
            || text.contains("selection is stale")
        {
            Code::ExternalChange
        } else if text.contains("classic") || text.contains("streamer") {
            Code::ClassicRequired
        } else if text.contains("device")
            && (text.contains("disconnected")
                || text.contains("disappeared")
                || text.contains("not found"))
        {
            Code::DeviceUnavailable
        } else if text.contains("session changed")
            || text.contains("process") && text.contains("no longer")
            || text.contains("group no longer")
        {
            Code::SessionChanged
        } else if text.contains("preparing an update")
            || text.contains("already being applied")
            || text.contains("operation is running")
        {
            Code::Busy
        } else if text.contains("readback")
            || text.contains("did not apply")
            || text.contains("could not be confirmed")
        {
            Code::Verification
        } else if text.contains("sonar")
            || text.contains("steelseries")
            || text.contains("not ready")
            || text.contains("timed out")
            || text.contains("coreprops")
        {
            Code::Connection
        } else {
            Code::Internal
        };
        Self { code, detail }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_preserve_cause_and_recovery_takes_precedence() {
        for (detail, expected) in [
            ("Settings save failed: Access denied", Code::SettingsSave),
            (
                "Volume changed externally; refresh before retrying",
                Code::ExternalChange,
            ),
            ("The audio device is disconnected", Code::DeviceUnavailable),
            ("Sonar is not ready", Code::Connection),
            ("Classic mode required", Code::ClassicRequired),
            (
                "Audio device/volume readback did not match",
                Code::Verification,
            ),
            (
                "Settings save failed; audio recovery: Sonar is not ready",
                Code::Recovery,
            ),
            (
                "Windows startup setting could not be confirmed",
                Code::Startup,
            ),
            (
                "Audio session changed; refresh before assigning",
                Code::SessionChanged,
            ),
        ] {
            let error = UserError::from(detail.to_string());
            assert_eq!(error.code, expected);
            assert_eq!(error.detail, detail);
            assert!(serde_json::to_value(error).unwrap()["code"].is_string());
        }
    }
}
