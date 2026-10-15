pub mod routing;
pub mod runtime;
pub mod settings;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub const GAMING: Uuid = Uuid::from_u128(1);
pub const CHAT: Uuid = Uuid::from_u128(2);
pub const MEDIA: Uuid = Uuid::from_u128(3);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioGroup {
    pub id: Uuid,
    pub display_name: String,
    pub icon_asset_id: Option<String>,
    pub virtual_endpoint_id: Option<String>,
    /// None follows the Windows default. Missing IDs are resolved at runtime.
    pub physical_output_id: Option<String>,
    pub volume: f32,
    /// Last confirmed group level on each physical output (full Sonar device ID).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub device_volumes: BTreeMap<String, f64>,
    /// Product group binding; Sonar still owns its fixed virtual channels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sonar_channel: Option<OutputChannel>,
}
impl AudioGroup {
    pub fn output_channel(&self) -> Option<OutputChannel> {
        self.sonar_channel.or(match self.id {
            GAMING => Some(OutputChannel::Game),
            CHAT => Some(OutputChannel::Chat),
            MEDIA => Some(OutputChannel::Media),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum OutputChannel {
    Game,
    Chat,
    Media,
    Aux,
}
impl OutputChannel {
    pub const ALL: [Self; 4] = [Self::Game, Self::Chat, Self::Media, Self::Aux];
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TrayShortcut {
    pub enabled: bool,
    pub key: Option<String>,
    pub modifiers: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default)]
    pub tray_shortcut: TrayShortcut,
    #[serde(default)]
    pub tray_volume_numbers_always_visible: bool,
    pub schema_version: u32,
    /// Persisted preference only. Runtime activation still requires a healthy engine.
    pub channel_separation_enabled: bool,
    /// Link physical output selections without linking channel volumes.
    #[serde(default = "enabled_by_default")]
    pub link_group_outputs: bool,
    /// Last confirmed microphone level on each physical input (full Sonar device ID).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub microphone_device_volumes: BTreeMap<String, f64>,
    pub launch_at_login: bool,
    #[serde(default)]
    pub startup_preference_initialized: bool,
    pub theme: Theme,
    pub language: String,
    pub groups: Vec<AudioGroup>,
    pub user_app_overrides: BTreeMap<String, Uuid>,
    /// Sonar app rules use a verified executable path or AUMID, never a PID.
    #[serde(default)]
    pub sonar_app_overrides: BTreeMap<String, OutputChannel>,
    #[serde(default)]
    pub sonar_auto_exclusions: BTreeSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tray_shortcut: TrayShortcut::default(),
            tray_volume_numbers_always_visible: false,
            channel_separation_enabled: false,
            link_group_outputs: true,
            microphone_device_volumes: BTreeMap::new(),
            launch_at_login: true,
            startup_preference_initialized: false,
            theme: Theme::System,
            language: "ko".into(),
            groups: [(GAMING, "게이밍"), (CHAT, "채팅"), (MEDIA, "미디어")]
                .into_iter()
                .map(|(id, name)| AudioGroup {
                    id,
                    display_name: name.into(),
                    icon_asset_id: None,
                    virtual_endpoint_id: None,
                    physical_output_id: None,
                    volume: 1.0,
                    device_volumes: BTreeMap::new(),
                    sonar_channel: None,
                })
                .collect(),
            user_app_overrides: BTreeMap::new(),
            sonar_app_overrides: BTreeMap::new(),
            sonar_auto_exclusions: BTreeSet::new(),
        }
    }
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("unsupported settings schema {0}")]
    Schema(u32),
    #[error("invalid or duplicate group")]
    Group,
    #[error("volume must be finite and within 0..=1")]
    Volume,
    #[error("app override references an unknown group")]
    Override,
    #[error("channel separation requires a healthy engine and a unique endpoint for each group")]
    EngineUnavailable,
}

impl Settings {
    pub fn validate(&self) -> Result<(), ModelError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ModelError::Schema(self.schema_version));
        }
        if self
            .microphone_device_volumes
            .iter()
            .any(|(device, level)| {
                device.is_empty() || !level.is_finite() || !(0.0..=1.0).contains(level)
            })
        {
            return Err(ModelError::Volume);
        }
        let mut ids = BTreeSet::new();
        let mut channels = BTreeSet::new();
        for group in &self.groups {
            if group.id.is_nil() || group.display_name.trim().is_empty() || !ids.insert(group.id) {
                return Err(ModelError::Group);
            }
            if !group.volume.is_finite() || !(0.0..=1.0).contains(&group.volume) {
                return Err(ModelError::Volume);
            }
            if group.device_volumes.iter().any(|(device, level)| {
                device.is_empty() || !level.is_finite() || !(0.0..=1.0).contains(level)
            }) {
                return Err(ModelError::Volume);
            }
            if let Some(channel) = group.output_channel()
                && !channels.insert(channel)
            {
                return Err(ModelError::Group);
            }
        }
        if self
            .user_app_overrides
            .iter()
            .any(|(app, id)| app.is_empty() || !ids.contains(id))
        {
            return Err(ModelError::Override);
        }
        if self
            .sonar_app_overrides
            .keys()
            .chain(self.sonar_auto_exclusions.iter())
            .any(|key| {
                !(key.starts_with("path:")
                    || key.starts_with("aumid:")
                    || key.starts_with("publisher:"))
                    || key.len() <= 5
            })
        {
            return Err(ModelError::Override);
        }
        Ok(())
    }

    pub fn assign_app(&mut self, stable_app_id: String, group_id: Uuid) -> Result<(), ModelError> {
        if stable_app_id.is_empty() || !self.groups.iter().any(|g| g.id == group_id) {
            return Err(ModelError::Override);
        }
        self.user_app_overrides.insert(stable_app_id, group_id);
        Ok(())
    }

    /// Removes overrides to the deleted group: affected apps follow Windows again.
    /// The live controller must restore Jack-owned routes BEFORE committing this model.
    pub fn remove_group(&mut self, group_id: Uuid) {
        self.groups.retain(|g| g.id != group_id);
        self.user_app_overrides.retain(|_, id| *id != group_id);
    }
    pub fn add_sonar_group(
        &mut self,
        name: String,
        channel: OutputChannel,
    ) -> Result<Uuid, ModelError> {
        if name.trim().is_empty()
            || self.groups.len() >= 4
            || self
                .groups
                .iter()
                .any(|group| group.output_channel() == Some(channel))
        {
            return Err(ModelError::Group);
        }
        let id = Uuid::new_v4();
        self.groups.push(AudioGroup {
            id,
            display_name: name.trim().into(),
            icon_asset_id: Some("Frame-1".into()),
            virtual_endpoint_id: None,
            physical_output_id: None,
            volume: 1.0,
            device_volumes: BTreeMap::new(),
            sonar_channel: Some(channel),
        });
        Ok(id)
    }
    pub fn remove_sonar_group(&mut self, group_id: Uuid) -> Result<OutputChannel, ModelError> {
        let channel = self
            .groups
            .iter()
            .find(|group| group.id == group_id)
            .and_then(AudioGroup::output_channel)
            .ok_or(ModelError::Group)?;
        let affected: Vec<_> = self
            .sonar_app_overrides
            .iter()
            .filter(|(_, output)| **output == channel)
            .map(|(app, _)| app.clone())
            .collect();
        self.remove_group(group_id);
        for app in affected {
            self.sonar_app_overrides.remove(&app);
            self.sonar_auto_exclusions.insert(app);
        }
        Ok(channel)
    }

    pub fn can_activate(&self, engine_healthy: bool) -> Result<(), ModelError> {
        let endpoints: BTreeSet<_> = self
            .groups
            .iter()
            .filter_map(|g| g.virtual_endpoint_id.as_deref())
            .filter(|id| !id.is_empty())
            .collect();
        if !engine_healthy || self.groups.is_empty() || endpoints.len() != self.groups.len() {
            return Err(ModelError::EngineUnavailable);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppIdentity {
    pub stable_id: String,
    pub executable_name: Option<String>,
    /// Must be verified from the executable's signature, never inferred from its name.
    pub verified_publisher: Option<String>,
    pub app_user_model_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Classification {
    User(Uuid),
    BuiltIn(Uuid),
    Unclassified,
}

/// Conservative rules: exact executable AND verified publisher, never a substring/PID.
pub fn classify(app: &AppIdentity, settings: &Settings) -> Classification {
    if let Some(id) = settings.user_app_overrides.get(&app.stable_id)
        && settings.groups.iter().any(|g| g.id == *id)
    {
        return Classification::User(*id);
    }
    let Some(exe) = app.executable_name.as_deref() else {
        return Classification::Unclassified;
    };
    let Some(publisher) = app.verified_publisher.as_deref() else {
        return Classification::Unclassified;
    };
    let rule = match (exe.to_ascii_lowercase().as_str(), publisher) {
        ("valorant-win64-shipping.exe", "Riot Games, Inc.") => Some(GAMING),
        ("discord.exe", "Discord Inc.") => Some(CHAT),
        ("spotify.exe", "Spotify AB") => Some(MEDIA),
        _ => None,
    };
    match rule.filter(|id| settings.groups.iter().any(|g| g.id == *id)) {
        Some(id) => Classification::BuiltIn(id),
        None => Classification::Unclassified,
    }
}

/// Restore only if the current policy is still the value Jack applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnedRoute {
    pub key: String,
    pub previous: Option<String>,
    pub applied: String,
}
impl OwnedRoute {
    pub fn restore_value(&self, current: Option<&str>) -> Option<Option<&str>> {
        (current == Some(self.applied.as_str())).then_some(self.previous.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn discord() -> AppIdentity {
        AppIdentity {
            stable_id: "path:c:/discord/discord.exe".into(),
            executable_name: Some("Discord.exe".into()),
            verified_publisher: Some("Discord Inc.".into()),
            app_user_model_id: None,
        }
    }
    #[test]
    fn starts_off_with_only_three_groups() {
        let s = Settings::default();
        assert!(!s.channel_separation_enabled);
        assert_eq!(s.groups.len(), 3);
        s.validate().unwrap();
    }
    #[test]
    fn sonar_preferences_load_legacy_settings_and_reject_pid_rules() {
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("sonar_app_overrides");
        value.as_object_mut().unwrap().remove("link_group_outputs");
        value
            .as_object_mut()
            .unwrap()
            .remove("sonar_auto_exclusions");
        let mut settings: Settings = serde_json::from_value(value).unwrap();
        assert!(settings.sonar_app_overrides.is_empty());
        assert!(settings.link_group_outputs);
        settings
            .sonar_app_overrides
            .insert("12345".into(), OutputChannel::Chat);
        assert!(settings.validate().is_err());
        settings.sonar_app_overrides.clear();
        settings.sonar_app_overrides.insert(
            "publisher:Discord Inc.:discord.exe".into(),
            OutputChannel::Media,
        );
        settings.validate().unwrap();
    }
    #[test]
    fn user_assignment_wins_and_clear_restores_automatic() {
        let mut s = Settings::default();
        let app = discord();
        s.assign_app(app.stable_id.clone(), MEDIA).unwrap();
        assert_eq!(classify(&app, &s), Classification::User(MEDIA));
        s.user_app_overrides.clear();
        assert_eq!(classify(&app, &s), Classification::BuiltIn(CHAT));
    }
    #[test]
    fn unverified_and_similar_names_stay_unclassified() {
        let s = Settings::default();
        let mut app = discord();
        app.verified_publisher = None;
        assert_eq!(classify(&app, &s), Classification::Unclassified);
        app.verified_publisher = Some("Discord Inc.".into());
        app.executable_name = Some("fake-discord.exe".into());
        assert_eq!(classify(&app, &s), Classification::Unclassified);
    }
    #[test]
    fn deleting_group_drops_its_override() {
        let mut s = Settings::default();
        s.assign_app(discord().stable_id, CHAT).unwrap();
        s.remove_group(CHAT);
        assert!(s.user_app_overrides.is_empty());
        assert_eq!(classify(&discord(), &s), Classification::Unclassified);
    }
    #[test]
    fn activation_rejects_missing_shared_or_unhealthy_endpoints() {
        let mut s = Settings::default();
        assert!(s.can_activate(true).is_err());
        for g in &mut s.groups {
            g.virtual_endpoint_id = Some(g.id.to_string());
        }
        assert!(s.can_activate(true).is_ok());
        assert!(s.can_activate(false).is_err());
        s.groups[1].virtual_endpoint_id = s.groups[0].virtual_endpoint_id.clone();
        assert!(s.can_activate(true).is_err());
    }
    #[test]
    fn invalid_volumes_and_dangling_assignments_rejected() {
        let mut s = Settings::default();
        s.groups[0].volume = f32::NAN;
        assert!(s.validate().is_err());
        s.groups[0].volume = 1.1;
        assert!(s.validate().is_err());
        s.groups[0].volume = 1.0;
        s.user_app_overrides.insert("app".into(), Uuid::new_v4());
        assert!(s.validate().is_err());
    }
    #[test]
    fn sonar_groups_reuse_channels_without_exceeding_capacity() {
        let mut settings = Settings::default();
        let id = settings
            .add_sonar_group("새 그룹".into(), OutputChannel::Aux)
            .unwrap();
        assert_ne!(id, GAMING);
        assert!(
            settings
                .add_sonar_group("추가".into(), OutputChannel::Aux)
                .is_err()
        );
        assert!(
            settings
                .add_sonar_group("추가".into(), OutputChannel::Game)
                .is_err()
        );
        settings.validate().unwrap();
        settings.remove_sonar_group(CHAT).unwrap();
        let replacement = settings
            .add_sonar_group("대화".into(), OutputChannel::Chat)
            .unwrap();
        assert_ne!(replacement, CHAT);
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        assert_eq!(restored, settings);
        assert_eq!(
            classify(&discord(), &restored),
            Classification::Unclassified
        );
    }
    #[test]
    fn sonar_group_deletion_clears_only_its_app_intent_and_blocks_reassignment() {
        let mut settings = Settings::default();
        settings
            .sonar_app_overrides
            .insert("path:chat".into(), OutputChannel::Chat);
        settings
            .sonar_app_overrides
            .insert("path:music".into(), OutputChannel::Media);
        settings
            .assign_app(discord().stable_id.clone(), CHAT)
            .unwrap();
        assert_eq!(
            settings.remove_sonar_group(CHAT).unwrap(),
            OutputChannel::Chat
        );
        assert!(!settings.sonar_app_overrides.contains_key("path:chat"));
        assert!(settings.sonar_auto_exclusions.contains("path:chat"));
        assert_eq!(
            settings.sonar_app_overrides["path:music"],
            OutputChannel::Media
        );
        assert!(settings.user_app_overrides.is_empty());
        assert_eq!(
            classify(&discord(), &settings),
            Classification::Unclassified
        );
    }
    #[test]
    fn old_groups_keep_original_bindings_and_duplicate_bindings_are_rejected() {
        let mut settings = Settings::default();
        let serialized = serde_json::to_value(&settings).unwrap();
        assert!(serialized["groups"][0].get("sonar_channel").is_none());
        assert_eq!(
            settings.groups[0].output_channel(),
            Some(OutputChannel::Game)
        );
        settings.groups[1].sonar_channel = Some(OutputChannel::Game);
        assert!(settings.validate().is_err());
    }
    #[test]
    fn restore_preserves_external_changes() {
        let r = OwnedRoute {
            key: "render:console".into(),
            previous: Some("speaker".into()),
            applied: "jack".into(),
        };
        assert_eq!(r.restore_value(Some("jack")), Some(Some("speaker")));
        assert_eq!(r.restore_value(Some("user-choice")), None);
        assert_eq!(r.restore_value(None), None);
    }
}
