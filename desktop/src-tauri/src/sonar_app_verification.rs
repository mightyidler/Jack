//! Live browser session assignment, with exact original per-app policy restore.
use crate::audio::{Action, Broker};
use jack_core::routing::{AudioPolicy, Flow, Role, RouteKey, WriteOutcome};
use jack_sonar::{
    Command,
    sonar::models::{Channel, MixerChannel},
};
use jack_windows_audio::policy::WindowsAudioPolicy;
use serde_json::{Value, json};

pub fn run(pid: u32) -> Result<Value, String> {
    let broker = Broker::start()?;
    let result = (|| {
        let before = broker.call(Action::Read)?;
        let app = before
            .sonar
            .applications
            .iter()
            .find(|a| a.process_id == pid)
            .ok_or("no live audio session for this PID")?;
        let original_channel = app
            .route
            .channel()
            .ok_or("session must start on one Sonar channel for this diagnostic")?;
        let target = if original_channel == Channel::Chat {
            Channel::Media
        } else {
            Channel::Chat
        };
        let endpoint = before
            .sonar
            .sources
            .iter()
            .find(|s| s.channel == target)
            .ok_or("target unavailable")?;
        let mut policy = WindowsAudioPolicy::new().map_err(|e| e.to_string())?;
        let identity = policy.bind_process(pid).map_err(|e| e.to_string())?;
        let keys =
            [Role::Console, Role::Multimedia, Role::Communications].map(|role| RouteKey::App {
                stable_app_id: identity.clone(),
                flow: Flow::Render,
                role,
            });
        let originals = keys
            .iter()
            .map(|k| policy.read(k))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let old_output = &before
            .sonar
            .routes
            .iter()
            .find(|r| r.channel == original_channel)
            .ok_or("initial output missing")?
            .device_id;
        let route = before
            .sonar
            .routes
            .iter()
            .find(|r| r.channel == target)
            .ok_or("target route missing")?;
        let physical = before
            .sonar
            .devices
            .iter()
            .find(|d| {
                d.enabled
                    && d.role == target.role()
                    && d.id != *old_output
                    && before
                        .audio
                        .endpoints
                        .iter()
                        .any(|e| e.id == d.id && e.form_factor == Some(9))
            })
            .ok_or("a separate digital output is required for physical proof")?;
        let mixer = if target == Channel::Chat {
            MixerChannel::Chat
        } else {
            MixerChannel::Media
        };
        let volume = before
            .sonar
            .volumes
            .iter()
            .find(|v| v.channel == mixer)
            .ok_or("target volume missing")?;
        if volume.muted {
            return Err("Unmute the target channel before this diagnostic".into());
        }
        let mut routed = false;
        let mut leveled = false;
        let mut physical_report = Value::Null;
        let applied = (|| {
            broker.call(Action::Sonar {
                command: Command::Route {
                    channel: target,
                    device_id: physical.id.clone(),
                    expected: route.device_id.clone(),
                },
            })?;
            routed = true;
            broker.call(Action::Sonar {
                command: Command::Volume {
                    channel: mixer,
                    level: 0.2,
                    expected: volume.volume,
                },
            })?;
            leveled = true;
            let initial = jack_windows_audio::sonar_probe::measure(old_output, 431.0)
                .map_err(|e| e.to_string())?;
            let destination_before = jack_windows_audio::sonar_probe::measure(&physical.id, 431.0)
                .map_err(|e| e.to_string())?;
            broker.call(Action::Sonar {
                command: Command::Application {
                    process_id: pid,
                    process_name: app.process_name.clone(),
                    channel: target,
                },
            })?;
            let destination_after = jack_windows_audio::sonar_probe::measure(&physical.id, 431.0)
                .map_err(|e| e.to_string())?;
            let previous_after = jack_windows_audio::sonar_probe::measure(old_output, 431.0)
                .map_err(|e| e.to_string())?;
            let passed = initial.tone_amplitude > 1e-7
                && destination_after.tone_amplitude > 1e-7
                && destination_before.tone_amplitude < destination_after.tone_amplitude * 0.05
                && previous_after.tone_amplitude < initial.tone_amplitude * 0.05;
            physical_report = json!({"passed":passed,"before":initial,"destination_before":destination_before,"destination_after":destination_after,"previous_output_after":previous_after});
            Ok::<_, String>(())
        })();
        let after_apply = broker.call(Action::Read);
        let mut restorations = vec![];
        for (key, original) in keys.iter().zip(&originals) {
            let current = policy.read(key).map_err(|e| e.to_string())?;
            if current == *original {
                continue;
            }
            if current.as_deref() != Some(&endpoint.endpoint_id) {
                restorations.push("external route change preserved".to_string());
                continue;
            }
            if policy
                .compare_and_set(key, Some(&endpoint.endpoint_id), original.as_deref())
                .map_err(|e| e.to_string())?
                != WriteOutcome::Applied
            {
                restorations.push("restore conflict".into());
            }
        }
        if leveled {
            broker.call(Action::Sonar {
                command: Command::Volume {
                    channel: mixer,
                    level: volume.volume,
                    expected: 0.2,
                },
            })?;
        }
        if routed {
            broker.call(Action::Sonar {
                command: Command::Route {
                    channel: target,
                    device_id: route.device_id.clone(),
                    expected: physical.id.clone(),
                },
            })?;
        }
        let after_restore = keys
            .iter()
            .map(|k| policy.read(k))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let effective = after_apply
            .as_ref()
            .ok()
            .and_then(|s| s.sonar.applications.iter().find(|a| a.process_id == pid))
            .map(|a| a.route);
        Ok(
            json!({"passed":applied.is_ok() && physical_report["passed"]==true && effective.and_then(|r|r.channel()) == Some(target) && originals == after_restore && restorations.is_empty(),"pid":pid,"app":app.process_name,"initial_channel":original_channel,"target":target,"operation_error":applied.err(),"observed_route":effective,"policy_restored":originals == after_restore,"restore_errors":restorations,"physical_audio":physical_report}),
        )
    })();
    broker.call(Action::Shutdown)?;
    result
}
