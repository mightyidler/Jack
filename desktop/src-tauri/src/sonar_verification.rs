//! Opt-in live verification of the same control broker used by Jack's UI.
use crate::audio::{Action, Broker};
use jack_sonar::{
    Command,
    sonar::models::{Channel, MixerChannel},
};
use serde_json::{Value, json};

pub fn run_pcm() -> Result<Value, String> {
    let broker = Broker::start()?;
    let result = (|| {
        let before = broker.call(Action::Read)?;
        if !before.sonar.available() {
            return Err("Sonar Classic mode unavailable".into());
        }
        let mut steps = vec![];
        for (channel, mixer) in [
            (Channel::Game, MixerChannel::Game),
            (Channel::Chat, MixerChannel::Chat),
            (Channel::Media, MixerChannel::Media),
            (Channel::Aux, MixerChannel::Aux),
        ] {
            let original = before
                .sonar
                .volumes
                .iter()
                .find(|v| v.channel == mixer)
                .ok_or("missing volume")?;
            let route = before
                .sonar
                .routes
                .iter()
                .find(|r| r.channel == channel)
                .ok_or("missing route")?;
            let source = before.sonar.sources.iter().find(|s| s.channel == channel);
            let Some(source) = source else {
                steps.push(json!({"channel":channel,"skipped":"Sonar does not expose this virtual output on this PC","passed":false}));
                continue;
            };
            let set_volume = |level, expected| {
                broker.call(Action::Sonar {
                    command: Command::Volume {
                        channel: mixer,
                        level,
                        expected,
                    },
                })
            };
            let set_mute = |muted, expected| {
                broker.call(Action::Sonar {
                    command: Command::Mute {
                        channel: mixer,
                        muted,
                        expected,
                    },
                })
            };
            set_volume(0.1, original.volume)?;
            if let Err(error) = set_mute(false, original.muted) {
                set_volume(original.volume, 0.1)?;
                return Err(error);
            }
            let mut owned_level = 0.1;
            let mut owned_mute = false;
            // A separate physical output avoids unrelated audio contaminating
            // the measurement and proves actual output-device routing.
            let output = before
                .sonar
                .devices
                .iter()
                .find(|d| d.enabled && d.role == channel.role() && d.id != route.device_id)
                .map(|d| d.id.as_str())
                .unwrap_or(&route.device_id);
            let mut route_changed = false;
            let observation = (|| {
                if output != route.device_id {
                    broker.call(Action::Sonar {
                        command: Command::Route {
                            channel,
                            device_id: output.into(),
                            expected: route.device_id.clone(),
                        },
                    })?;
                    route_changed = true;
                }
                let low = jack_windows_audio::sonar_probe::observe(&source.endpoint_id, output)
                    .map_err(|e| e.to_string())?;
                set_volume(0.2, owned_level)?;
                owned_level = 0.2;
                let high = jack_windows_audio::sonar_probe::observe(&source.endpoint_id, output)
                    .map_err(|e| e.to_string())?;
                set_mute(true, false)?;
                owned_mute = true;
                let muted = jack_windows_audio::sonar_probe::observe(&source.endpoint_id, output)
                    .map_err(|e| e.to_string())?;
                let passed = high.tone_amplitude > 1e-7
                    && low.tone_amplitude < high.tone_amplitude * 0.8
                    && muted.tone_amplitude < high.tone_amplitude * 0.05;
                Ok::<_, String>(
                    json!({"channel":channel,"low":low,"high":high,"muted":muted,"passed":passed}),
                )
            })();
            let volume_restore = set_volume(original.volume, owned_level);
            let mute_restore = set_mute(original.muted, owned_mute);
            let route_restore = if route_changed {
                broker
                    .call(Action::Sonar {
                        command: Command::Route {
                            channel,
                            device_id: route.device_id.clone(),
                            expected: output.into(),
                        },
                    })
                    .map(|_| ())
            } else {
                Ok(())
            };
            volume_restore?;
            mute_restore?;
            route_restore?;
            steps.push(observation?);
        }
        let after = broker.call(Action::Read)?;
        let restored = serde_json::to_value(&before.sonar.volumes).unwrap()
            == serde_json::to_value(&after.sonar.volumes).unwrap()
            && serde_json::to_value(&before.sonar.routes).unwrap()
                == serde_json::to_value(&after.sonar.routes).unwrap();
        Ok(
            json!({"passed":restored && steps.iter().all(|s| s["passed"] == true),"physical_loopback":true,"listening_verified":false,"sonar_restored":restored,"steps":steps}),
        )
    })();
    broker.call(Action::Shutdown)?;
    result
}

pub fn run() -> Result<Value, String> {
    let broker = Broker::start()?;
    let result = (|| {
        let before = broker.call(Action::Read)?;
        if !before.sonar.available() {
            return Err(format!("Sonar unavailable: {:?}", before.sonar.message));
        }
        let mut steps = vec![];
        for channel in [
            MixerChannel::Game,
            MixerChannel::Chat,
            MixerChannel::Media,
            MixerChannel::Aux,
        ] {
            let original = before
                .sonar
                .volumes
                .iter()
                .find(|v| v.channel == channel)
                .ok_or("missing channel")?;
            let target = if original.volume >= 0.02 {
                original.volume - 0.01
            } else {
                original.volume + 0.01
            };
            let changed = broker.call(Action::Sonar {
                command: Command::Volume {
                    channel,
                    level: target,
                    expected: original.volume,
                },
            });
            let restored = broker.call(Action::Sonar {
                command: Command::Volume {
                    channel,
                    level: original.volume,
                    expected: target,
                },
            });
            let restored_ok = restored.is_ok();
            steps.push(json!({"kind":"volume","channel":channel,"applied":changed.is_ok(),"restored":restored_ok,"error":changed.err(),"restore_error":restored.err()}));
            if !restored_ok {
                return Err(format!("{channel:?} volume restoration failed"));
            }
            let muted = broker.call(Action::Sonar {
                command: Command::Mute {
                    channel,
                    muted: !original.muted,
                    expected: original.muted,
                },
            });
            let unmuted = broker.call(Action::Sonar {
                command: Command::Mute {
                    channel,
                    muted: original.muted,
                    expected: !original.muted,
                },
            });
            let restored_ok = unmuted.is_ok();
            steps.push(json!({"kind":"mute","channel":channel,"applied":muted.is_ok(),"restored":restored_ok,"error":muted.err(),"restore_error":unmuted.err()}));
            if !restored_ok {
                return Err(format!("{channel:?} mute restoration failed"));
            }
        }
        let original = before
            .sonar
            .routes
            .iter()
            .find(|r| r.channel == Channel::Game)
            .ok_or("missing game route")?;
        if let Some(target) = before
            .sonar
            .devices
            .iter()
            .find(|d| d.enabled && d.role == Channel::Game.role() && d.id != original.device_id)
        {
            let changed = broker.call(Action::Sonar {
                command: Command::Route {
                    channel: Channel::Game,
                    device_id: target.id.clone(),
                    expected: original.device_id.clone(),
                },
            });
            let restored = broker.call(Action::Sonar {
                command: Command::Route {
                    channel: Channel::Game,
                    device_id: original.device_id.clone(),
                    expected: target.id.clone(),
                },
            });
            steps.push(json!({"kind":"route","applied":changed.is_ok(),"restored":restored.is_ok(),"error":changed.err(),"restore_error":restored.err()}));
        }
        let after = broker.call(Action::Read)?;
        let unchanged = serde_json::to_value(&before.sonar.volumes).unwrap()
            == serde_json::to_value(&after.sonar.volumes).unwrap()
            && serde_json::to_value(&before.sonar.routes).unwrap()
                == serde_json::to_value(&after.sonar.routes).unwrap();
        let windows_unchanged = serde_json::to_value(&before.audio.endpoints).unwrap()
            == serde_json::to_value(&after.audio.endpoints).unwrap();
        Ok(
            json!({"passed":unchanged && windows_unchanged && steps.iter().all(|s| s["applied"] == true && s["restored"] == true),"steps":steps,"sonar_restored":unchanged,"windows_controls_unchanged":windows_unchanged,"physical_audio":"not evaluated by this API roundtrip"}),
        )
    })();
    let shutdown = broker.call(Action::Shutdown);
    shutdown?;
    result
}
