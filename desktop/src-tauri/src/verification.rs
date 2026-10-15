//! Opt-in current-PC verification of the exact broker used by the GUI.
//! Each temporary change is read back and restored while still owned.
use crate::audio::{Action, Broker, ViewState};
use jack_core::routing::Flow;
use serde_json::{Value, json};

fn controls(state: &ViewState) -> Value {
    json!(
        state
            .audio
            .endpoints
            .iter()
            .map(|e| json!({"id":e.id,"defaults":e.defaults,"volume":e.volume}))
            .collect::<Vec<_>>()
    )
}
pub fn run() -> Result<Value, String> {
    let broker = Broker::start()?;
    let before = broker.call(Action::Read)?;
    let mut steps = vec![];
    for (flow, flow_name, target_name) in [
        (Flow::Render, "render", "LG TV"),
        (Flow::Capture, "capture", "Razer"),
    ] {
        let original = before
            .audio
            .endpoints
            .iter()
            .find(|e| e.flow == flow_name && e.defaults.contains(&"console"));
        let target = before
            .audio
            .endpoints
            .iter()
            .find(|e| e.flow == flow_name && e.name.contains(target_name));
        if let (Some(original), Some(target)) = (original, target) {
            if !original.defaults.contains(&"multimedia") || original.id == target.id {
                steps.push(json!({"kind":"selection","flow":flow_name,"skipped":"original roles differ or target already selected"}));
                continue;
            }
            let result = broker.call(Action::Select {
                flow,
                endpoint: target.id.clone(),
                expected: Some(original.id.clone()),
            });
            let applied_result = broker.call(Action::Read);
            let applied = applied_result.as_ref().ok();
            let current = applied.and_then(|s| {
                s.audio
                    .endpoints
                    .iter()
                    .find(|e| e.flow == flow_name && e.defaults.contains(&"console"))
            });
            let selected =
                current.is_some_and(|e| e.id == target.id && e.defaults.contains(&"multimedia"));
            let communications = |s: &ViewState| {
                s.audio
                    .endpoints
                    .iter()
                    .find(|e| e.flow == flow_name && e.defaults.contains(&"communications"))
                    .map(|e| e.id.clone())
            };
            let communications_unchanged =
                applied.is_some_and(|s| communications(&before) == communications(s));
            let restoration = broker.call(Action::Select {
                flow,
                endpoint: original.id.clone(),
                expected: Some(target.id.clone()),
            });
            let after = broker.call(Action::Read);
            let restored = after.as_ref().is_ok_and(|s| {
                s.audio.endpoints.iter().any(|e| {
                    e.id == original.id
                        && e.defaults.contains(&"console")
                        && e.defaults.contains(&"multimedia")
                })
            });
            steps.push(json!({"kind":"selection","flow":flow_name,"target":target.id,"operation_error":result.err(),"selected":selected,"communications_unchanged":communications_unchanged,"restore_error":restoration.err(),"restored":restored}));
        } else {
            steps.push(json!({"kind":"selection","flow":flow_name,"skipped":"known physical target unavailable"}));
        }
    }
    if let Some(target) = before
        .audio
        .endpoints
        .iter()
        .find(|e| e.flow == "render" && e.name.contains("LG TV"))
        && let Some(volume) = &target.volume
    {
        let level = if volume.level >= 0.04 {
            volume.level - 0.02
        } else {
            volume.level + 0.02
        };
        let result = broker.call(Action::Volume {
            flow: Flow::Render,
            endpoint: target.id.clone(),
            level,
            expected: volume.level,
            expected_muted: volume.muted,
        });
        let applied = broker.call(Action::Read);
        let current = applied.as_ref().ok().and_then(|s| {
            s.audio
                .endpoints
                .iter()
                .find(|e| e.id == target.id)
                .and_then(|e| e.volume.as_ref())
        });
        let matched =
            current.is_some_and(|v| (v.level - level).abs() < 0.0001 && v.muted == volume.muted);
        let restore = broker.call(Action::Volume {
            flow: Flow::Render,
            endpoint: target.id.clone(),
            level: volume.level,
            expected: level,
            expected_muted: volume.muted,
        });
        steps.push(json!({"kind":"volume","operation_error":result.err(),"readback_matched":matched,"restore_error":restore.err()}));
    }
    let saved = &before.settings;
    let result = broker.call(Action::Preferences {
        theme: jack_core::Theme::Dark,
        language: "en".into(),
        launch_at_login: saved.launch_at_login,
    });
    let applied = broker.call(Action::Read);
    let restore = broker.call(Action::Preferences {
        theme: saved.theme,
        language: saved.language.clone(),
        launch_at_login: saved.launch_at_login,
    });
    steps.push(json!({"kind":"preferences","operation_error":result.err(),"readback_matched":applied.as_ref().is_ok_and(|s| s.settings.theme == jack_core::Theme::Dark && s.settings.language == "en"),"restore_error":restore.err()}));
    let enabled = broker.call(Action::Preferences {
        theme: saved.theme,
        language: saved.language.clone(),
        launch_at_login: true,
    });
    let enabled_readback = broker
        .call(Action::Read)
        .is_ok_and(|s| s.settings.launch_at_login);
    let restore = broker.call(Action::Preferences {
        theme: saved.theme,
        language: saved.language.clone(),
        launch_at_login: saved.launch_at_login,
    });
    steps.push(json!({"kind":"startup","operation_error":enabled.err(),"readback_matched":enabled_readback,"restore_error":restore.err()}));
    let after = broker.call(Action::Read)?;
    let unchanged = controls(&before) == controls(&after);
    let preferences_restored = saved.theme == after.settings.theme
        && saved.language == after.settings.language
        && saved.launch_at_login == after.settings.launch_at_login;
    broker.call(Action::Shutdown)?;
    let passed = unchanged
        && preferences_restored
        && steps.iter().all(|s| {
            s.get("skipped").is_none()
                && s["operation_error"].is_null()
                && s["restore_error"].is_null()
                && s.get("selected").is_none_or(|v| v == true)
                && s.get("readback_matched").is_none_or(|v| v == true)
                && s.get("communications_unchanged").is_none_or(|v| v == true)
                && s.get("restored").is_none_or(|v| v == true)
        });
    Ok(
        json!({"passed":passed,"steps":steps,"audio_controls_restored":unchanged,"preferences_restored":preferences_restored,"before":controls(&before),"after":controls(&after),"driver_installed":false,"separation_available":after.separation_available}),
    )
}
