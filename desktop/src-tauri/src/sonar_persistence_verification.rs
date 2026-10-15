//! Opt-in app-rule lifecycle proof. Caller backs up/restores settings bytes.
use crate::audio::{Action, Broker};
use jack_core::{
    OutputChannel,
    routing::{AudioPolicy, Flow, Role, RouteKey},
};
use jack_sonar::sonar::models::Channel;
use jack_windows_audio::policy::WindowsAudioPolicy;
use serde_json::{Value, json};

pub fn run(pid: u32) -> Result<Value, String> {
    let broker = Broker::start()?;
    let before = broker.call(Action::Read)?;
    let app = before
        .sonar
        .applications
        .iter()
        .find(|a| a.process_id == pid)
        .ok_or("Live app session required")?;
    let name = app.process_name.clone();
    let identity = before
        .app_routing
        .iter()
        .find(|a| a.process_id == pid)
        .and_then(|a| a.stable_id.clone())
        .ok_or("Stable app identity required")?;
    let original = app.route.channel();
    let target = if original == Some(Channel::Chat) {
        Channel::Media
    } else {
        Channel::Chat
    };
    let mut policy = WindowsAudioPolicy::new().map_err(|e| e.to_string())?;
    let binding = policy.bind_process(pid).map_err(|e| e.to_string())?;
    let keys = [Role::Console, Role::Multimedia, Role::Communications].map(|role| RouteKey::App {
        stable_app_id: binding.clone(),
        flow: Flow::Render,
        role,
    });
    let initial = keys
        .iter()
        .map(|k| policy.read(k))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let operation = (|| {
        broker.call(Action::Separation { enabled: true })?;
        let applied = broker.call(Action::AssignApplication {
            process_id: pid,
            process_name: name.clone(),
            channel: Some(target),
            automatic: false,
        })?;
        let saved = applied.settings.sonar_app_overrides.get(&identity)
            == Some(&if target == Channel::Chat {
                OutputChannel::Chat
            } else {
                OutputChannel::Media
            });
        let effective = applied
            .sonar
            .applications
            .iter()
            .any(|a| a.process_id == pid && a.route.channel() == Some(target));
        broker.call(Action::Separation { enabled: false })?;
        let off = keys
            .iter()
            .map(|k| policy.read(k))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            == initial;
        let enabled = broker.call(Action::Separation { enabled: true })?;
        let reapplied = enabled
            .sonar
            .applications
            .iter()
            .any(|a| a.process_id == pid && a.route.channel() == Some(target));
        Ok::<_, String>(
            json!({"saved":saved,"effective":effective,"off_restored":off,"on_reapplied":reapplied}),
        )
    })();
    broker.call(Action::Shutdown)?;
    drop(broker);
    let shutdown_restored = keys
        .iter()
        .map(|k| policy.read(k))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?
        == initial;
    let broker = Broker::start()?;
    let after_restart = broker.call(Action::Read)?;
    let restart_reapplied = after_restart
        .sonar
        .applications
        .iter()
        .any(|a| a.process_id == pid && a.route.channel() == Some(target));
    let cleanup = broker.call(Action::AssignApplication {
        process_id: pid,
        process_name: name,
        channel: None,
        automatic: true,
    });
    let shutdown = broker.call(Action::Shutdown);
    drop(broker);
    cleanup?;
    shutdown?;
    let final_policy = keys
        .iter()
        .map(|k| policy.read(k))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let steps = operation?;
    Ok(
        json!({"passed":steps["saved"]==true&&steps["effective"]==true&&steps["off_restored"]==true&&steps["on_reapplied"]==true&&shutdown_restored&&restart_reapplied&&final_policy==initial,"stable_id":identity,"steps":steps,"shutdown_restored":shutdown_restored,"restart_reapplied":restart_reapplied,"policy_restored":final_policy==initial}),
    )
}
