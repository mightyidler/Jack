//! Two separate diagnostic processes, simultaneously mixed through Sonar.
use crate::audio::{Action, Broker};
use jack_sonar::{
    Command,
    sonar::models::{Channel, MixerChannel},
};
use serde_json::{Value, json};
use std::process::{Child, Stdio};

struct Signal(Child);
impl Drop for Signal {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn run() -> Result<Value, String> {
    let broker = Broker::start()?;
    let result = (|| {
        let before = broker.call(Action::Read)?;
        if !before.sonar.available() {
            return Err("Sonar Classic required".into());
        }
        let output = before
            .sonar
            .devices
            .iter()
            .find(|d| {
                d.enabled
                    && before
                        .audio
                        .endpoints
                        .iter()
                        .any(|e| e.id == d.id && e.form_factor == Some(9))
            })
            .ok_or("separate digital output required")?;
        let pairs = [
            (Channel::Game, MixerChannel::Game, 431.0),
            (Channel::Chat, MixerChannel::Chat, 673.0),
        ];
        let mut changes = vec![];
        let mut signals = vec![];
        let observed = (|| {
            for (channel, mixer, frequency) in pairs {
                let source = before
                    .sonar
                    .sources
                    .iter()
                    .find(|s| s.channel == channel)
                    .ok_or("source unavailable")?;
                let route = before
                    .sonar
                    .routes
                    .iter()
                    .find(|r| r.channel == channel)
                    .ok_or("route unavailable")?;
                let volume = before
                    .sonar
                    .volumes
                    .iter()
                    .find(|v| v.channel == mixer)
                    .ok_or("volume unavailable")?;
                broker.call(Action::Sonar {
                    command: Command::Route {
                        channel,
                        device_id: output.id.clone(),
                        expected: route.device_id.clone(),
                    },
                })?;
                changes.push(Command::Route {
                    channel,
                    device_id: route.device_id.clone(),
                    expected: output.id.clone(),
                });
                broker.call(Action::Sonar {
                    command: Command::Volume {
                        channel: mixer,
                        level: 0.2,
                        expected: volume.volume,
                    },
                })?;
                changes.push(Command::Volume {
                    channel: mixer,
                    level: volume.volume,
                    expected: 0.2,
                });
                broker.call(Action::Sonar {
                    command: Command::Mute {
                        channel: mixer,
                        muted: false,
                        expected: volume.muted,
                    },
                })?;
                changes.push(Command::Mute {
                    channel: mixer,
                    muted: volume.muted,
                    expected: false,
                });
                signals.push(Signal(
                    std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
                        .args(["--sonar-tone", &source.endpoint_id, &frequency.to_string()])
                        .stdout(Stdio::null())
                        .spawn()
                        .map_err(|e| e.to_string())?,
                ));
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let state = loop {
                let state = broker.call(Action::Read)?;
                if signals.iter().zip(pairs).all(|(signal, (channel, _, _))| {
                    state.sonar.applications.iter().any(|app| {
                        app.process_id == signal.0.id() && app.route.channel() == Some(channel)
                    })
                }) || std::time::Instant::now() >= deadline
                {
                    break state;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            };
            let processes: Vec<_> = signals.iter().map(|s| {
                let pid = s.0.id();
                json!({"pid":pid,"session":state.sonar.applications.iter().find(|a|a.process_id==pid)})
            }).collect();
            let measure = || -> Result<Value, String> {
                Ok(
                    json!({"game":jack_windows_audio::sonar_probe::measure(&output.id,431.0).map_err(|e|e.to_string())?,
                    "chat":jack_windows_audio::sonar_probe::measure(&output.id,673.0).map_err(|e|e.to_string())?}),
                )
            };
            let both = measure()?;
            broker.call(Action::Sonar {
                command: Command::Mute {
                    channel: MixerChannel::Game,
                    muted: true,
                    expected: false,
                },
            })?;
            // Update ownership before measurement so all error paths restore the current mute.
            for command in &mut changes {
                if let Command::Mute {
                    channel: MixerChannel::Game,
                    expected,
                    ..
                } = command
                {
                    *expected = true;
                }
            }
            let game_muted = measure()?;
            broker.call(Action::Sonar {
                command: Command::Mute {
                    channel: MixerChannel::Game,
                    muted: false,
                    expected: true,
                },
            })?;
            for command in &mut changes {
                if let Command::Mute {
                    channel: MixerChannel::Game,
                    expected,
                    ..
                } = command
                {
                    *expected = false;
                }
            }
            broker.call(Action::Sonar {
                command: Command::Mute {
                    channel: MixerChannel::Chat,
                    muted: true,
                    expected: false,
                },
            })?;
            for command in &mut changes {
                if let Command::Mute {
                    channel: MixerChannel::Chat,
                    expected,
                    ..
                } = command
                {
                    *expected = true;
                }
            }
            let chat_muted = measure()?;
            let amplitude = |sample: &Value, channel: &str| {
                sample[channel]["tone_amplitude"].as_f64().unwrap_or(0.0)
            };
            let game = amplitude(&both, "game");
            let chat = amplitude(&both, "chat");
            let unchanged =
                |value: f64, reference: f64| value > reference * 0.8 && value < reference * 1.2;
            let passed = processes
                .iter()
                .all(|process| !process["session"].is_null())
                && game > 1e-7
                && chat > 1e-7
                && amplitude(&game_muted, "game") < game * 0.05
                && unchanged(amplitude(&game_muted, "chat"), chat)
                && amplitude(&chat_muted, "chat") < chat * 0.05
                && unchanged(amplitude(&chat_muted, "game"), game);
            let group_policy = verify_group_policy(
                &mut signals,
                &before
                    .sonar
                    .sources
                    .iter()
                    .find(|source| source.channel == Channel::Game)
                    .ok_or("game source unavailable")?
                    .endpoint_id,
            )?;
            Ok::<_, String>(
                json!({"passed":passed && group_policy["passed"]==true,"processes":processes,"both":both,"game_muted":game_muted,"chat_muted":chat_muted,"group_policy":group_policy}),
            )
        })();
        drop(signals);
        let mut errors = vec![];
        for command in changes.into_iter().rev() {
            if let Err(error) = broker.call(Action::Sonar { command }) {
                errors.push(error);
            }
        }
        let after = broker.call(Action::Read)?;
        let restored = serde_json::to_value(&before.sonar.volumes).unwrap()
            == serde_json::to_value(&after.sonar.volumes).unwrap()
            && serde_json::to_value(&before.sonar.routes).unwrap()
                == serde_json::to_value(&after.sonar.routes).unwrap();
        Ok(
            json!({"passed":observed.as_ref().is_ok_and(|o|o["passed"]==true) && restored && errors.is_empty(),"sonar_restored":restored,
            "signals":"Two native diagnostic processes, 431Hz and 673Hz, amplitude 0.002; no microphone or saved PCM", "observed":observed.as_ref().ok(),"operation_error":observed.err(),"restore_errors":errors}),
        )
    })();
    broker.call(Action::Shutdown)?;
    result
}

fn verify_group_policy(signals: &mut [Signal], target: &str) -> Result<Value, String> {
    use jack_core::routing::{AudioPolicy, FileJournal, JournalStore, RouteController};
    use jack_windows_audio::policy::WindowsAudioPolicy;
    let mut policy = WindowsAudioPolicy::new().map_err(|error| error.to_string())?;
    let identity = policy
        .bind_process(signals[0].0.id())
        .map_err(|error| error.to_string())?;
    let same_app = policy
        .bind_process(signals[1].0.id())
        .map_err(|error| error.to_string())?
        == identity;
    let keys = crate::sonar_routing::app_keys(&identity);
    let read = |policy: &mut WindowsAudioPolicy| {
        keys.iter()
            .map(|key| policy.read(key))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    };
    let original = read(&mut policy)?;
    let store = FileJournal {
        path: std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join("artifacts/sonar-process-group-journal.json"),
    };
    if !store
        .load()
        .map_err(|error| error.to_string())?
        .changes
        .is_empty()
    {
        return Err("group diagnostic journal requires recovery".into());
    }
    let mut controller = RouteController { policy, store };
    let mut applied = vec![];
    let mut survivor = vec![];
    let operation = (|| {
        controller
            .apply_batch(
                keys.iter()
                    .cloned()
                    .map(|key| (key, target.into()))
                    .collect(),
            )
            .map_err(|error| error.to_string())?;
        applied = read(&mut controller.policy)?;
        signals[0].0.kill().map_err(|error| error.to_string())?;
        signals[0].0.wait().map_err(|error| error.to_string())?;
        survivor = read(&mut controller.policy)?;
        Ok::<_, String>(())
    })();
    let recovery = controller
        .restore_best_effort()
        .map_err(|error| error.to_string())?;
    let after = read(&mut controller.policy)?;
    let passed = same_app
        && operation.is_ok()
        && recovery.complete
        && original == after
        && applied.iter().all(|value| value.as_deref() == Some(target))
        && survivor == applied;
    Ok(
        json!({"passed":passed,"same_durable_app":same_app,"original":original,"applied_all_live_members":applied,"after_first_member_exit":survivor,"restored":after,"recovery":recovery,"operation_error":operation.err()}),
    )
}
