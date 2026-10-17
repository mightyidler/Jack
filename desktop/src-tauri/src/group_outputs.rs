//! Physical device selection with independent per-device volume memory.
//! Output linking applies to configured groups; microphones remain independent.
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
            || !state.devices.iter().any(|device| {
                device.id == route.device_id
                    && device.enabled
                    && device.role == DeviceRole::Playback
            })
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
    if let Some(route) = state
        .routes
        .iter()
        .find(|r| r.channel == Channel::Microphone)
        && state
            .devices
            .iter()
            .any(|d| d.id == route.device_id && d.enabled && d.role == DeviceRole::Capture)
        && let Some(volume) = state.volumes.iter().find(|v| {
            v.channel == MixerChannel::Microphone
                && v.volume.is_finite()
                && (0.0..=1.0).contains(&v.volume)
        })
        && !route.device_id.is_empty()
    {
        // Sonar exposes one microphone mixer level, not a level for every
        // inactive input. Pin unvisited devices when first discovered, before
        // editing the active input can change their first-selection fallback.
        for device in state
            .devices
            .iter()
            .filter(|d| d.enabled && d.role == DeviceRole::Capture && !d.id.is_empty())
        {
            settings
                .microphone_device_volumes
                .entry(device.id.clone())
                .or_insert_with(|| {
                    changed = true;
                    volume.volume
                });
        }
        if settings
            .microphone_device_volumes
            .get(&route.device_id)
            .is_none_or(|old| (old - volume.volume).abs() > 0.000001)
        {
            settings
                .microphone_device_volumes
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
    commit: impl FnOnce(&Settings) -> Result<(), String>,
) -> Result<Settings, String> {
    select_with_commit(service, settings, channel, device, expected, commit)
}

/// Enabling the link immediately adopts Gaming's device. Persist the switch
/// together with routing and per-group gain so a failed change stays OFF.
pub fn set_linked(
    service: &mut Service,
    settings: &Settings,
    enabled: bool,
    commit: impl FnOnce(&Settings) -> Result<(), String>,
) -> Result<Settings, String> {
    set_linked_with_commit(service, settings, enabled, commit)
}

fn set_linked_with_commit(
    service: &mut impl OutputService,
    settings: &Settings,
    enabled: bool,
    commit: impl FnOnce(&Settings) -> Result<(), String>,
) -> Result<Settings, String> {
    let mut next = settings.clone();
    next.link_group_outputs = enabled;
    if !enabled || settings.link_group_outputs {
        commit(&next)?;
        return Ok(next);
    }
    let before = service.read();
    if !before.available() {
        return Err("Sonar is not ready".into());
    }
    let device = &before
        .routes
        .iter()
        .find(|route| route.channel == Channel::Game)
        .ok_or("Gaming output is not ready")?
        .device_id;
    select_with_commit(service, &next, Channel::Game, device, device, commit)
}

#[cfg(test)]
fn select_with(
    service: &mut impl OutputService,
    settings: &Settings,
    channel: Channel,
    device: &str,
    expected: &str,
) -> Result<Settings, String> {
    select_with_commit(service, settings, channel, device, expected, |_| Ok(()))
}
fn select_with_commit(
    service: &mut impl OutputService,
    settings: &Settings,
    channel: Channel,
    device: &str,
    expected: &str,
    commit: impl FnOnce(&Settings) -> Result<(), String>,
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
        return Err("Audio device changed externally; refresh before retrying".into());
    }
    // Microphone controls are independent of physical playback linking.
    let microphone = channel == Channel::Microphone;
    let channels: Vec<_> = settings
        .groups
        .iter()
        .filter_map(|g| g.output_channel())
        .map(crate::sonar_routing::channel)
        .collect();
    if !microphone && !channels.contains(&channel) {
        return Err("The selected group no longer exists".into());
    }
    let role = if microphone {
        DeviceRole::Capture
    } else {
        DeviceRole::Playback
    };
    if !before
        .devices
        .iter()
        .any(|d| d.id == device && d.enabled && d.role == role)
    {
        return Err("The audio device is disconnected".into());
    }
    let targets: Vec<_> = if microphone {
        vec![(channel, expected.to_owned())]
    } else {
        channels
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
            .collect::<Result<_, _>>()?
    };
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
                .ok_or("Audio volume is not ready")?
                .volume;
            let saved = if *channel == Channel::Microphone {
                next.microphone_device_volumes.get(device)
            } else {
                next.groups
                    .iter()
                    .find(|g| {
                        g.output_channel().map(crate::sonar_routing::channel) == Some(*channel)
                    })
                    .and_then(|g| g.device_volumes.get(device))
            }
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
                .ok_or("Audio device disappeared")?;
            if route.device_id == device {
                // GG can couple outputs itself; accept the preceding write's result.
                if previous != device {
                    changed.push((*channel, previous.clone()));
                }
                continue;
            }
            if route.device_id != *previous {
                return Err("Audio device changed externally".into());
            }
            // Include uncertain writes in recovery: a failed readback may follow a successful PUT.
            changed.push((*channel, previous.clone()));
            service.route(*channel, device, previous)?;
        }
        for (channel, previous, desired) in &levels {
            if *channel != Channel::Microphone && (previous - desired).abs() <= 0.000001 {
                continue;
            }
            let current = service.read();
            if !current
                .routes
                .iter()
                .any(|r| r.channel == *channel && r.device_id == device)
            {
                return Err("Audio device changed before volume restore".into());
            }
            // GG may restore its own microphone level while routing. Restore
            // Jack's saved level against that confirmed value, not the old device's.
            let expected = if *channel == Channel::Microphone {
                let observed = current
                    .volumes
                    .iter()
                    .find(|v| v.channel == MixerChannel::Microphone)
                    .ok_or("Microphone volume disappeared")?
                    .volume;
                if (observed - desired).abs() <= 0.000001 {
                    continue;
                }
                observed
            } else {
                *previous
            };
            changed_levels.push((*channel, *previous, *desired));
            service.volume(*channel, *desired, expected)?;
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
            return Err("Audio device/volume readback did not match".into());
        }
        remember(&mut next, &after);
        // Persistence is part of the same transaction as routing and gain.
        // A disk error must take the existing rollback path before returning.
        commit(&next)?;
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
            format!("{error}; audio recovery: {}", recovery.join("; "))
        });
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Live Sonar sync enable/volume restore; requires Jack to be closed"]
    fn live_enable_sync_restores_group_levels_and_recovers_original_audio() {
        let proof = std::path::PathBuf::from(
            std::env::var("JACK_OUTPUT_SYNC_PROOF").expect("explicit proof path required"),
        );
        let root = jack_windows_audio::control::current_user_root().unwrap();
        let _lease =
            jack_windows_audio::control::ControlLease::acquire(&root.join("control.lock")).unwrap();
        let mut service = Service::new().unwrap();
        let before = service.read();
        assert!(before.available());
        let game = before
            .routes
            .iter()
            .find(|r| r.channel == Channel::Game)
            .unwrap();
        let other = before
            .devices
            .iter()
            .find(|d| d.enabled && d.role == DeviceRole::Playback && d.id != game.device_id)
            .expect("connected alternative output required");
        let mut settings = Settings {
            link_group_outputs: false,
            ..Settings::default()
        };
        settings.groups.retain(|g| {
            before
                .sources
                .iter()
                .any(|s| g.output_channel().map(crate::sonar_routing::channel) == Some(s.channel))
        });
        if before.sources.iter().any(|s| s.channel == Channel::Aux) {
            settings
                .add_sonar_group("Aux".into(), jack_core::OutputChannel::Aux)
                .unwrap();
        }
        let operation = (|| -> Result<(), String> {
            // Exercise the OFF -> ON transition with genuinely different devices.
            for group in settings
                .groups
                .iter()
                .filter(|g| g.output_channel() != Some(jack_core::OutputChannel::Game))
            {
                let channel = crate::sonar_routing::channel(group.output_channel().unwrap());
                let current = service.read();
                let route = current
                    .routes
                    .iter()
                    .find(|r| r.channel == channel)
                    .ok_or("route missing")?;
                if route.device_id != other.id {
                    service.route(channel, &other.id, &route.device_id)?;
                }
            }
            let split = service.read();
            if !split
                .routes
                .iter()
                .any(|r| r.channel == Channel::Game && r.device_id == game.device_id)
            {
                return Err("GG coupled Gaming while preparing independent outputs".into());
            }
            for (group, level) in settings.groups.iter_mut().zip([0.1, 0.0, 0.27, 0.41]) {
                group.device_volumes.insert(game.device_id.clone(), level);
            }
            let selected = set_linked(&mut service, &settings, true, |_| Ok(()))?;
            let after = service.read();
            if !selected.link_group_outputs
                || !selected
                    .groups
                    .iter()
                    .zip([0.1, 0.0, 0.27, 0.41])
                    .all(|(g, saved)| {
                        let channel = crate::sonar_routing::channel(g.output_channel().unwrap());
                        let level = if channel == Channel::Game {
                            split
                                .volumes
                                .iter()
                                .find(|v| v.channel == MixerChannel::Game)
                                .unwrap()
                                .volume
                        } else {
                            saved
                        };
                        after
                            .routes
                            .iter()
                            .any(|r| r.channel == channel && r.device_id == game.device_id)
                            && after.volumes.iter().any(|v| {
                                v.channel == channel.into() && (v.volume - level).abs() < 0.0001
                            })
                    })
            {
                return Err("sync enable readback failed".into());
            }
            Ok(())
        })();
        // Restore routes first, then gains against their latest confirmed values.
        let mut recovery = Vec::new();
        for route in &before.routes {
            let current = service.read();
            if let Some(now) = current.routes.iter().find(|r| r.channel == route.channel)
                && now.device_id != route.device_id
                && let Err(error) = service.route(route.channel, &route.device_id, &now.device_id)
            {
                recovery.push(error);
            }
        }
        for volume in &before.volumes {
            let current = service.read();
            if let Some(now) = current.volumes.iter().find(|v| v.channel == volume.channel)
                && (now.volume - volume.volume).abs() > 0.0001
                && let Err(error) = service.execute(Command::Volume {
                    channel: volume.channel,
                    level: volume.volume,
                    expected: now.volume,
                })
            {
                recovery.push(error.to_string());
            }
        }
        let restored = service.read();
        let routes_restored = before.routes.iter().all(|r| {
            restored
                .routes
                .iter()
                .any(|v| v.channel == r.channel && v.device_id == r.device_id)
        });
        let volumes_restored = before.volumes == restored.volumes;
        let passed =
            operation.is_ok() && routes_restored && volumes_restored && recovery.is_empty();
        let report = serde_json::json!({"passed":passed,"operation_error":operation.err(),"routes_restored":routes_restored,"volumes_restored":volumes_restored,"recovery_errors":recovery});
        std::fs::write(proof, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        assert!(passed, "{report}");
    }
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
        let request = select_with(
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
                && let Err(error) = select_with(
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
                devices: ["new", "old"]
                    .into_iter()
                    .map(|id| AudioDevice {
                        id: id.into(),
                        name: "Output".into(),
                        role: DeviceRole::Playback,
                        enabled: true,
                        virtual_device: false,
                    })
                    .collect(),
                ..State::default()
            },
            fail: None,
            external: None,
            coupled: false,
            calls: vec![],
            fail_volume: None,
        }
    }
    fn unlinked_fixture() -> (Fake, Settings) {
        let mut f = fixture();
        let mut settings = Settings {
            link_group_outputs: false,
            ..Settings::default()
        };
        settings
            .add_sonar_group("Aux".into(), jack_core::OutputChannel::Aux)
            .unwrap();
        f.state.sources.push(Source {
            channel: Channel::Aux,
            endpoint_id: "aux".into(),
        });
        f.state.routes.push(Route {
            channel: Channel::Aux,
            device_id: "old".into(),
            device_name: None,
        });
        f.state
            .volumes
            .push(jack_sonar::sonar::models::VolumeState {
                channel: MixerChannel::Aux,
                volume: 0.63,
                muted: true,
            });
        f.state.routes[0].device_id = "new".into();
        for (group, level) in settings.groups.iter_mut().zip([0.1, 0.0, 0.21, 0.87]) {
            group.device_volumes.insert("new".into(), level);
        }
        (f, settings)
    }
    #[test]
    fn enabling_sync_adopts_gaming_and_restores_each_device_group_level() {
        let (mut f, settings) = unlinked_fixture();
        let next = set_linked_with_commit(&mut f, &settings, true, |next| {
            assert!(next.link_group_outputs);
            assert_eq!(next.groups[1].device_volumes["new"], 0.0);
            Ok(())
        })
        .unwrap();
        assert!(next.link_group_outputs);
        assert_eq!(f.calls, vec![Channel::Chat, Channel::Media, Channel::Aux]);
        for (group, expected) in next.groups.iter().zip([0.63, 0.0, 0.21, 0.87]) {
            let channel = crate::sonar_routing::channel(group.output_channel().unwrap());
            assert!(
                f.state
                    .routes
                    .iter()
                    .any(|r| r.channel == channel && r.device_id == "new")
            );
            assert!(
                f.state
                    .volumes
                    .iter()
                    .any(|v| v.channel == channel.into() && v.volume == expected)
            );
            assert_eq!(group.device_volumes["new"], expected);
            if channel != Channel::Game {
                assert_eq!(group.device_volumes["old"], 0.63);
            }
        }
        assert_eq!(f.state.routes[3].device_id, "old");
        assert_eq!(f.state.volumes[3].volume, 0.63);
        assert!(f.state.volumes[4].muted);
        assert!(!settings.link_group_outputs);
    }
    #[test]
    fn enabling_sync_keeps_current_levels_for_same_or_unremembered_devices() {
        let (mut f, mut settings) = unlinked_fixture();
        f.state.routes[1].device_id = "new".into();
        settings.groups[2].device_volumes.clear();
        set_linked_with_commit(&mut f, &settings, true, |_| Ok(())).unwrap();
        assert_eq!(f.calls, vec![Channel::Media, Channel::Aux]);
        assert_eq!(f.state.volumes[0].volume, 0.63);
        assert_eq!(f.state.volumes[1].volume, 0.63);
        assert_eq!(f.state.volumes[2].volume, 0.63);
    }
    #[test]
    fn failed_sync_enable_recovers_audio_and_does_not_commit_on() {
        for failure in ["route", "volume", "commit", "disconnected", "missing"] {
            let (mut f, settings) = unlinked_fixture();
            match failure {
                "route" => f.fail = Some(Channel::Media),
                "volume" => f.fail_volume = Some(Channel::Media),
                "disconnected" => f.state.connection = "disconnected".into(),
                "missing" => f.state.routes.retain(|r| r.channel != Channel::Game),
                _ => {}
            }
            let before = f.state.clone();
            let mut commits = 0;
            assert!(
                set_linked_with_commit(&mut f, &settings, true, |_| {
                    commits += 1;
                    Err("disk full".into())
                })
                .is_err(),
                "{failure}"
            );
            assert_eq!(commits, usize::from(failure == "commit"), "{failure}");
            assert_eq!(f.state.routes, before.routes, "{failure}");
            assert_eq!(f.state.volumes, before.volumes, "{failure}");
            assert!(!settings.link_group_outputs);
        }
    }
    #[test]
    fn disabling_or_repeating_sync_does_not_route_even_when_disconnected() {
        for (previous, enabled) in [(true, false), (false, false), (true, true)] {
            let (mut f, mut settings) = unlinked_fixture();
            settings.link_group_outputs = previous;
            f.state.connection = "disconnected".into();
            let before = f.state.clone();
            let next = set_linked_with_commit(&mut f, &settings, enabled, |_| Ok(())).unwrap();
            assert_eq!(next.link_group_outputs, enabled);
            assert!(f.calls.is_empty());
            assert_eq!(f.state.routes, before.routes);
            assert_eq!(f.state.volumes, before.volumes);
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
    #[test]
    fn disconnected_physical_output_does_not_overwrite_remembered_levels() {
        let mut f = fixture();
        let mut settings = Settings::default();
        assert!(remember(&mut settings, &f.state));
        let before = settings.clone();
        f.state
            .devices
            .iter_mut()
            .find(|device| device.id == "old")
            .unwrap()
            .enabled = false;
        f.state
            .volumes
            .iter_mut()
            .for_each(|volume| volume.volume = 0.0);
        assert!(!remember(&mut settings, &f.state));
        assert_eq!(settings, before);
    }
    #[test]
    fn microphone_memory_pins_unvisited_inputs_and_restores_zero_independently() {
        let mut f = fixture();
        f.state.routes[3].device_id = "mic-old".into();
        f.state
            .devices
            .extend(["mic-old", "mic-new"].into_iter().map(|id| AudioDevice {
                id: id.into(),
                name: "Input".into(),
                role: DeviceRole::Capture,
                enabled: true,
                virtual_device: false,
            }));
        let mut settings = Settings::default();
        assert!(remember(&mut settings, &f.state));
        f.state.volumes[3].volume = 0.0;
        assert!(remember(&mut settings, &f.state));
        assert_eq!(settings.microphone_device_volumes["mic-new"], 0.63);
        let next =
            select_with(&mut f, &settings, Channel::Microphone, "mic-new", "mic-old").unwrap();
        assert_eq!(f.state.volumes[3].volume, 0.63);
        assert_eq!(next.microphone_device_volumes["mic-old"], 0.0);
        let restored: Settings =
            serde_json::from_str(&serde_json::to_string(&next).unwrap()).unwrap();
        select_with(&mut f, &restored, Channel::Microphone, "mic-old", "mic-new").unwrap();
        assert_eq!(f.state.volumes[3].volume, 0.0);
        assert!(
            f.state.routes[..3]
                .iter()
                .all(|route| route.device_id == "old")
        );
        assert!(
            f.state.volumes[..3]
                .iter()
                .all(|volume| volume.volume == 0.63)
        );
    }
    #[test]
    fn settings_commit_failure_recovers_routes_and_levels_and_retains_original_memory() {
        let mut f = fixture();
        let mut settings = Settings::default();
        for group in &mut settings.groups {
            group.device_volumes.insert("new".into(), 0.2);
        }
        let original = settings.clone();
        let mut commits = 0;
        let error = select_with_commit(&mut f, &settings, Channel::Game, "new", "old", |next| {
            commits += 1;
            assert!(
                next.groups
                    .iter()
                    .all(|group| group.device_volumes["old"] == 0.63)
            );
            Err("disk full".into())
        })
        .unwrap_err();
        assert_eq!(error, "disk full");
        assert_eq!(commits, 1);
        assert!(f.state.routes.iter().all(|route| route.device_id == "old"));
        assert!(f.state.volumes.iter().all(|volume| volume.volume == 0.63));
        assert_eq!(settings, original);
    }
}
