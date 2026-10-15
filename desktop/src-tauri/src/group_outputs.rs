//! A physical output selection changes configured output groups only.
use jack_core::Settings;
use jack_sonar::{
    Command, Service, State,
    sonar::models::{Channel, DeviceRole, MixerChannel},
};

trait OutputService {
    fn read(&mut self) -> State;
    fn route(&mut self, channel: Channel, device: &str, expected: &str) -> Result<(), String>;
    fn volume(&mut self, channel: Channel, level: f64, expected: f64) -> Result<(), String>;
}
impl OutputService for Service {
    fn read(&mut self) -> State {
        self.refresh(false)
    }
    fn route(&mut self, channel: Channel, device: &str, expected: &str) -> Result<(), String> {
        self.execute(Command::Route {
            channel,
            device_id: device.into(),
            expected: expected.into(),
        })
        .map_err(|e| e.to_string())
    }
    fn volume(&mut self, channel: Channel, level: f64, expected: f64) -> Result<(), String> {
        self.execute(Command::Volume {
            channel: channel.into(),
            level,
            expected,
        })
        .map_err(|e| e.to_string())
    }
}

/// Capture confirmed state, including adjustments made outside Jack. Do not
/// overwrite remembered levels with a disconnected/incomplete Sonar snapshot.
pub fn remember(settings: &mut Settings, state: &State) -> bool {
    if !state.available() {
        return false;
    }
    let mut changed = false;
    for group in &mut settings.groups {
        let Some(channel) = group.output_channel().map(crate::sonar_routing::channel) else {
            continue;
        };
        let Some(route) = state.routes.iter().find(|r| r.channel == channel) else {
            continue;
        };
        let Some(volume) = state
            .volumes
            .iter()
            .find(|v| v.channel == MixerChannel::from(channel))
        else {
            continue;
        };
        if route.device_id.is_empty()
            || !volume.volume.is_finite()
            || !(0.0..=1.0).contains(&volume.volume)
        {
            continue;
        }
        if group
            .device_volumes
            .get(&route.device_id)
            .is_none_or(|old| (old - volume.volume).abs() > 0.000001)
        {
            group
                .device_volumes
                .insert(route.device_id.clone(), volume.volume);
            changed = true;
        }
    }
    changed
}
pub fn select(
    service: &mut Service,
    settings: &Settings,
    channel: Channel,
    device: &str,
    expected: &str,
) -> Result<Settings, String> {
    select_with(service, settings, channel, device, expected)
}
fn select_with(
    service: &mut impl OutputService,
    settings: &Settings,
    channel: Channel,
    device: &str,
    expected: &str,
) -> Result<Settings, String> {
    let before = service.read();
    if !before.available() {
        return Err("Sonar is not ready".into());
    }
    if !before
        .routes
        .iter()
        .any(|r| r.channel == channel && r.device_id == expected)
    {
        return Err("Output changed externally; refresh before retrying".into());
    }
    // Microphone controls are independent of physical playback linking.
    if channel == Channel::Microphone {
        service.route(channel, device, expected)?;
        return Ok(settings.clone());
    }
    let channels: Vec<_> = settings
        .groups
        .iter()
        .filter_map(|g| g.output_channel())
        .map(crate::sonar_routing::channel)
        .collect();
    if !channels.contains(&channel) {
        return Err("The selected group no longer exists".into());
    }
    if !before
        .devices
        .iter()
        .any(|d| d.id == device && d.enabled && d.role == DeviceRole::Playback)
    {
        return Err("The output device is disconnected".into());
    }
    let targets: Vec<_> = channels
        .into_iter()
        .filter(|c| settings.link_group_outputs || *c == channel)
        .map(|c| {
            before
                .routes
                .iter()
                .find(|r| r.channel == c)
                .filter(|_| before.sources.iter().any(|s| s.channel == c))
                .map(|r| (c, r.device_id.clone()))
                .ok_or_else(|| "A group output is not ready".to_string())
        })
        .collect::<Result<_, _>>()?;
    // Save outgoing levels before any route writes (GG may couple routes).
    let mut next = settings.clone();
    remember(&mut next, &before);
    let levels: Vec<_> = targets
        .iter()
        .map(|(channel, previous)| {
            let current = before
                .volumes
                .iter()
                .find(|v| v.channel == MixerChannel::from(*channel))
                .ok_or("A group volume is not ready")?
                .volume;
            let saved = next
                .groups
                .iter()
                .find(|g| g.output_channel().map(crate::sonar_routing::channel) == Some(*channel))
                .and_then(|g| g.device_volumes.get(device))
                .copied()
                .unwrap_or(current);
            Ok((
                *channel,
                current,
                if previous == device { current } else { saved },
            ))
        })
        .collect::<Result<_, String>>()?;
    let mut changed = Vec::new();
    let mut changed_levels = Vec::new();
    let result = (|| {
        for (channel, previous) in &targets {
            let current = service.read();
            let route = current
                .routes
                .iter()
                .find(|r| r.channel == *channel)
                .ok_or("A group output disappeared")?;
            if route.device_id == device {
                // GG can couple outputs itself; accept the preceding write's result.
                if previous != device {
                    changed.push((*channel, previous.clone()));
                }
                continue;
            }
            if route.device_id != *previous {
                return Err("A group output changed externally".into());
            }
            // Include uncertain writes in recovery: a failed readback may follow a successful PUT.
            changed.push((*channel, previous.clone()));
            service.route(*channel, device, previous)?;
        }
        for (channel, previous, desired) in &levels {
            if (previous - desired).abs() <= 0.000001 {
                continue;
            }
            let current = service.read();
            if !current
                .routes
                .iter()
                .any(|r| r.channel == *channel && r.device_id == device)
            {
                return Err("A group output changed before volume restore".into());
            }
            changed_levels.push((*channel, *previous, *desired));
            service.volume(*channel, *desired, *previous)?;
        }
        let after = service.read();
        if !after.available()
            || !targets.iter().all(|(c, _)| {
                after
                    .routes
                    .iter()
                    .any(|r| r.channel == *c && r.device_id == device)
            })
            || !levels.iter().all(|(c, _, desired)| {
                after.volumes.iter().any(|v| {
                    v.channel == MixerChannel::from(*c) && (v.volume - desired).abs() < 0.0001
                })
            })
        {
            return Err("Group output readback did not match".into());
        }
        remember(&mut next, &after);
        Ok(())
    })();
    if let Err(error) = result {
        let mut recovery = Vec::new();
        for (channel, previous, desired) in changed_levels.into_iter().rev() {
            let current = service.read();
            if current
                .routes
                .iter()
                .any(|r| r.channel == channel && r.device_id == device)
                && current.volumes.iter().any(|v| {
                    v.channel == MixerChannel::from(channel) && (v.volume - desired).abs() < 0.0001
                })
                && let Err(e) = service.volume(channel, previous, desired)
            {
                recovery.push(e);
            }
        }
        for (channel, previous) in changed.into_iter().rev() {
            // Respect any intervening GG change instead of overwriting it.
            if service
                .read()
                .routes
                .iter()
                .any(|r| r.channel == channel && r.device_id == device)
                && let Err(e) = service.route(channel, &previous, device)
            {
                recovery.push(e);
            }
        }
        return Err(if recovery.is_empty() {
            error
        } else {
            format!("{error}; output recovery: {}", recovery.join("; "))
        });
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Live Sonar group output switch/readback/restore; requires Jack to be closed"]
    fn live_group_outputs_change_together_and_restore() {
        let proof = std::path::PathBuf::from(
            std::env::var("JACK_OUTPUT_SYNC_PROOF").expect("explicit proof path required"),
        );
        let root = jack_windows_audio::control::current_user_root().unwrap();
        let _lease =
            jack_windows_audio::control::ControlLease::acquire(&root.join("control.lock")).unwrap();
        let mut service = Service::new().unwrap();
        let before = service.refresh(false);
        assert!(before.available());
        let mut settings = Settings::default();
        settings.groups.retain(|g| {
            g.output_channel().is_some_and(|c| {
                before
                    .sources
                    .iter()
                    .any(|s| s.channel == crate::sonar_routing::channel(c))
            })
        });
        if before.sources.iter().any(|s| s.channel == Channel::Aux) {
            settings
                .add_sonar_group("Aux".into(), jack_core::OutputChannel::Aux)
                .unwrap();
        }
        let game = before
            .routes
            .iter()
            .find(|r| r.channel == Channel::Game)
            .unwrap();
        let device = before
            .devices
            .iter()
            .find(|d| d.enabled && d.role == DeviceRole::Playback && d.id != game.device_id)
            .expect("connected alternative output required");
        let request = select(
            &mut service,
            &settings,
            Channel::Game,
            &device.id,
            &game.device_id,
        );
        let selected = service.refresh(false);
        let confirmed = settings
            .groups
            .iter()
            .filter_map(|g| g.output_channel())
            .all(|c| {
                selected.routes.iter().any(|r| {
                    r.channel == crate::sonar_routing::channel(c) && r.device_id == device.id
                })
            });
        settings.link_group_outputs = false;
        let mut recovery = Vec::new();
        for group in &settings.groups {
            let channel = crate::sonar_routing::channel(group.output_channel().unwrap());
            let previous = before.routes.iter().find(|r| r.channel == channel).unwrap();
            let current = service.refresh(false);
            if current
                .routes
                .iter()
                .any(|r| r.channel == channel && r.device_id == device.id)
                && previous.device_id != device.id
                && let Err(error) = select(
                    &mut service,
                    &settings,
                    channel,
                    &previous.device_id,
                    &device.id,
                )
            {
                recovery.push(error);
            }
        }
        let after = service.refresh(false);
        let restored = before.routes.iter().all(|a| {
            after
                .routes
                .iter()
                .any(|b| a.channel == b.channel && a.device_id == b.device_id)
        });
        let passed = request.is_ok()
            && confirmed
            && restored
            && before.volumes == after.volumes
            && recovery.is_empty();
        let report = serde_json::json!({"passed":passed,"scope":"Actual shared group output command and Sonar full-ID readback/restore; does not prove GUI device selection or human listening","group_count":settings.groups.len(),"target_device":device,"selected_routes":selected.routes,"before_routes":before.routes,"after_routes":after.routes,"unchanged_volumes":before.volumes==after.volumes,"request_error":request.err(),"recovery_errors":recovery});
        std::fs::write(proof, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        assert!(passed, "{report}");
    }
    use jack_sonar::{
        Source,
        sonar::models::{AudioDevice, Route},
    };
    struct Fake {
        state: State,
        fail: Option<Channel>,
        external: Option<Channel>,
        coupled: bool,
        calls: Vec<Channel>,
        fail_volume: Option<Channel>,
    }
    impl OutputService for Fake {
        fn read(&mut self) -> State {
            self.state.clone()
        }
        fn volume(&mut self, channel: Channel, level: f64, expected: f64) -> Result<(), String> {
            let volume = self
                .state
                .volumes
                .iter_mut()
                .find(|v| v.channel == MixerChannel::from(channel))
                .unwrap();
            if (volume.volume - expected).abs() > 0.0001 {
                return Err("volume changed externally".into());
            }
            if self.fail_volume == Some(channel) {
                return Err("volume write failed".into());
            }
            volume.volume = level;
            Ok(())
        }
        fn route(&mut self, channel: Channel, device: &str, expected: &str) -> Result<(), String> {
            self.calls.push(channel);
            let route = self
                .state
                .routes
                .iter_mut()
                .find(|r| r.channel == channel)
                .unwrap();
            assert_eq!(route.device_id, expected);
            if self.fail == Some(channel) {
                if let Some(c) = self.external {
                    self.state
                        .routes
                        .iter_mut()
                        .find(|r| r.channel == c)
                        .unwrap()
                        .device_id = "external".into();
                }
                return Err("write failed".into());
            }
            route.device_id = device.into();
            if self.coupled {
                for route in &mut self.state.routes {
                    if route.channel != Channel::Microphone {
                        route.device_id = device.into();
                    }
                }
            }
            Ok(())
        }
    }
    fn fixture() -> Fake {
        Fake {
            state: State {
                connection: "connected".into(),
                mode: Some("classic".into()),
                volumes: [
                    Channel::Game,
                    Channel::Chat,
                    Channel::Media,
                    Channel::Microphone,
                ]
                .into_iter()
                .map(|channel| jack_sonar::sonar::models::VolumeState {
                    channel: channel.into(),
                    volume: 0.63,
                    muted: false,
                })
                .collect(),
                routes: [
                    Channel::Game,
                    Channel::Chat,
                    Channel::Media,
                    Channel::Microphone,
                ]
                .into_iter()
                .map(|channel| Route {
                    channel,
                    device_id: "old".into(),
                    device_name: None,
                })
                .collect(),
                sources: [Channel::Game, Channel::Chat, Channel::Media]
                    .into_iter()
                    .map(|channel| Source {
                        channel,
                        endpoint_id: format!("{channel:?}"),
                    })
                    .collect(),
                devices: vec![AudioDevice {
                    id: "new".into(),
                    name: "Output".into(),
                    role: DeviceRole::Playback,
                    enabled: true,
                    virtual_device: false,
                }],
                ..State::default()
            },
            fail: None,
            external: None,
            coupled: false,
            calls: vec![],
            fail_volume: None,
        }
    }
    #[test]
    fn gg_coupled_outputs_do_not_replay_the_write() {
        let mut f = fixture();
        f.coupled = true;
        select_with(&mut f, &Settings::default(), Channel::Chat, "new", "old").unwrap();
        assert_eq!(f.calls, vec![Channel::Game]);
    }
    #[test]
    fn linked_selection_changes_three_outputs_without_microphone() {
        let mut f = fixture();
        select_with(&mut f, &Settings::default(), Channel::Chat, "new", "old").unwrap();
        assert_eq!(f.calls, vec![Channel::Game, Channel::Chat, Channel::Media]);
        assert_eq!(f.state.routes[3].device_id, "old");
    }
    #[test]
    fn unlinked_selection_changes_only_selected_group() {
        let mut f = fixture();
        let settings = Settings {
            link_group_outputs: false,
            ..Settings::default()
        };
        select_with(&mut f, &settings, Channel::Chat, "new", "old").unwrap();
        assert_eq!(f.calls, vec![Channel::Chat]);
    }
    #[test]
    fn failure_recovers_changed_outputs_and_preserves_external_choice() {
        let mut f = fixture();
        f.fail = Some(Channel::Media);
        assert!(select_with(&mut f, &Settings::default(), Channel::Chat, "new", "old").is_err());
        assert!(f.state.routes.iter().all(|r| r.device_id == "old"));
        let mut f = fixture();
        f.fail = Some(Channel::Media);
        f.external = Some(Channel::Game);
        assert!(select_with(&mut f, &Settings::default(), Channel::Chat, "new", "old").is_err());
        assert_eq!(f.state.routes[0].device_id, "external");
        assert_eq!(f.state.routes[1].device_id, "old");
    }
    #[test]
    fn linked_devices_restore_each_groups_own_level_including_zero() {
        let mut f = fixture();
        let mut settings = Settings::default();
        for (group, level) in settings.groups.iter_mut().zip([0.0, 0.21, 0.87]) {
            group.device_volumes.insert("new".into(), level);
        }
        let next = select_with(&mut f, &settings, Channel::Chat, "new", "old").unwrap();
        for ((group, volume), expected) in next
            .groups
            .iter()
            .zip(&f.state.volumes)
            .zip([0.0, 0.21, 0.87])
        {
            assert_eq!(group.device_volumes["old"], 0.63);
            assert_eq!(group.device_volumes["new"], expected);
            assert_eq!(volume.volume, expected);
        }
        assert_eq!(f.state.volumes[3].volume, 0.63);
        assert_eq!(f.state.routes[3].device_id, "old");
        // The same data survives an app restart via settings serialization.
        let mut restored: Settings =
            serde_json::from_str(&serde_json::to_string(&next).unwrap()).unwrap();
        restored.link_group_outputs = false;
        f.state.devices[0].id = "old".into();
        let back = select_with(&mut f, &restored, Channel::Chat, "old", "new").unwrap();
        assert_eq!(f.state.volumes[1].volume, 0.63);
        assert_eq!(f.state.volumes[0].volume, 0.0);
        assert_eq!(back.groups[1].device_volumes["new"], 0.21);
    }
    #[test]
    fn first_visit_keeps_current_level_and_current_device_is_never_reset() {
        let mut f = fixture();
        let settings =
            select_with(&mut f, &Settings::default(), Channel::Game, "new", "old").unwrap();
        assert!(f.state.volumes.iter().all(|v| v.volume == 0.63));
        f.state.volumes[0].volume = 0.34; // external adjustment
        let next = select_with(&mut f, &settings, Channel::Game, "new", "new").unwrap();
        assert_eq!(f.state.volumes[0].volume, 0.34);
        assert_eq!(next.groups[0].device_volumes["new"], 0.34);
    }
    #[test]
    fn volume_failure_recovers_both_levels_and_routes_without_committing_memory() {
        let mut f = fixture();
        f.fail_volume = Some(Channel::Chat);
        let mut settings = Settings::default();
        for group in &mut settings.groups {
            group.device_volumes.insert("new".into(), 0.2);
        }
        assert!(select_with(&mut f, &settings, Channel::Game, "new", "old").is_err());
        assert!(f.state.routes.iter().all(|r| r.device_id == "old"));
        assert!(f.state.volumes.iter().all(|v| v.volume == 0.63));
        assert!(
            settings
                .groups
                .iter()
                .all(|g| !g.device_volumes.contains_key("old"))
        );
    }
    #[test]
    fn disconnected_snapshot_keeps_saved_memory() {
        let mut f = fixture();
        let mut settings = Settings::default();
        assert!(remember(&mut settings, &f.state));
        let before = settings.clone();
        f.state.connection = "disconnected".into();
        f.state.volumes[0].volume = 0.0;
        assert!(!remember(&mut settings, &f.state));
        assert_eq!(settings, before);
    }
}
