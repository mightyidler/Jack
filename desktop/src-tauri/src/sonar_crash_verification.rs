//! Live durable-policy recovery across a killed control process. Never kill the
//! user's audio app. The caller snapshots/restores settings with the normal lease.
use crate::audio::{Action, Broker};
use jack_core::routing::AudioPolicy;
use jack_sonar::sonar::models::Channel;
use jack_windows_audio::{control::current_user_root, policy::WindowsAudioPolicy};
use serde_json::{Value, json};
use std::os::windows::process::CommandExt;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct ReadyFile(std::path::PathBuf);
impl Drop for ReadyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn prepare(pid: u32, ready: &Path) -> Result<(), String> {
    let broker = Broker::start()?;
    let operation = (|| {
        let before = broker.call(Action::Read)?;
        let app = before
            .sonar
            .applications
            .iter()
            .find(|app| app.process_id == pid)
            .ok_or("Live audio app required")?;
        let target = if app.route.channel() == Some(Channel::Chat) {
            Channel::Media
        } else {
            Channel::Chat
        };
        broker.call(Action::Separation { enabled: true })?;
        let assigned = broker.call(Action::AssignApplication {
            process_id: pid,
            process_name: app.process_name.clone(),
            channel: Some(target),
            automatic: false,
        })?;
        let effective = assigned
            .sonar
            .applications
            .iter()
            .any(|app| app.process_id == pid && app.route.channel() == Some(target));
        let value = json!({"target":target,"effective":effective,"saved":assigned.app_routing.iter().any(|app|app.process_id==pid && app.origin=="user")});
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(ready)
            .map_err(|error| error.to_string())?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec(&value).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        // A supervisor kills only this diagnostic worker after observing durable
        // state. If supervision fails, normal shutdown bounds the mutation.
        std::thread::sleep(Duration::from_secs(30));
        Ok::<_, String>(())
    })();
    let shutdown = broker.call(Action::Shutdown);
    operation?;
    shutdown?;
    Ok(())
}

pub fn run(pid: u32) -> Result<Value, String> {
    let mut policy = WindowsAudioPolicy::new().map_err(|error| error.to_string())?;
    let binding = policy
        .bind_process(pid)
        .map_err(|error| error.to_string())?;
    let keys = crate::sonar_routing::app_keys(&binding);
    let read = |policy: &mut WindowsAudioPolicy| {
        keys.iter()
            .map(|key| policy.read(key))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    };
    let initial = read(&mut policy)?;
    let before_audio = jack_windows_audio::snapshot().map_err(|error| error.to_string())?;
    let temp = std::env::temp_dir().join("JackSonarCrashProof");
    std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
    let ready = ReadyFile(temp.join(format!("{}.json", uuid::Uuid::new_v4())));
    let mut child = Worker(
        Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
            .arg("--prepare-sonar-crash")
            .arg(pid.to_string())
            .arg(&ready.0)
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .spawn()
            .map_err(|error| error.to_string())?,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let prepared = loop {
        if let Ok(bytes) = std::fs::read(&ready.0)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            break value;
        }
        if let Some(status) = child.0.try_wait().map_err(|error| error.to_string())? {
            return Err(format!("crash worker exited before ready: {status}"));
        }
        if Instant::now() >= deadline {
            return Err("crash worker did not report ready".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let after_assignment = read(&mut policy)?;
    child.0.kill().map_err(|error| error.to_string())?;
    child.0.wait().map_err(|error| error.to_string())?;
    drop(child);
    let after_kill = read(&mut policy)?;
    let journal_before: Value = serde_json::from_slice(
        &std::fs::read(
            current_user_root()
                .map_err(|error| error.to_string())?
                .join("routes.json"),
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let broker = Broker::start()?;
    let mut external_report = Value::Null;
    let operation = (|| {
        // Broker initialization recovers before the first view reapplies saved ON.
        let recovered_before_read = read(&mut policy)?;
        let reapplied = broker.call(Action::Read)?;
        let target: Channel = serde_json::from_value(prepared["target"].clone())
            .map_err(|error| error.to_string())?;
        let reapply_effective = reapplied
            .sonar
            .applications
            .iter()
            .any(|app| app.process_id == pid && app.route.channel() == Some(target));
        let reset = broker.call(Action::Reset)?;
        let reset_restored = read(&mut policy)? == initial;
        let after_audio = jack_windows_audio::snapshot().map_err(|error| error.to_string())?;
        let defaults_and_volumes_unchanged = serde_json::to_value(&before_audio.endpoints).unwrap()
            == serde_json::to_value(&after_audio.endpoints).unwrap();
        external_report = verify_external_choice(&broker, pid, &initial)?;
        Ok::<_, String>(
            json!({"recovered_before_read":recovered_before_read==initial,"saved_on_reapplied":reapply_effective,
            "reset_restored":reset_restored,"reset_rules_empty":reset.settings.sonar_app_overrides.is_empty() && reset.settings.sonar_auto_exclusions.is_empty() && !reset.settings.channel_separation_enabled,
            "defaults_and_volumes_unchanged":defaults_and_volumes_unchanged,"external_choice_preserved":external_report["passed"]==true}),
        )
    })();
    let shutdown = broker.call(Action::Shutdown);
    drop(broker);
    shutdown?;
    let after = read(&mut policy)?;
    let journal_after: Value = serde_json::from_slice(
        &std::fs::read(
            current_user_root()
                .map_err(|error| error.to_string())?
                .join("routes.json"),
        )
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let steps = operation?;
    let durable_before = journal_before["changes"]
        .as_array()
        .is_some_and(|changes| !changes.is_empty());
    let durable_after = journal_after["changes"]
        .as_array()
        .is_some_and(|changes| changes.is_empty());
    Ok(
        json!({"passed":prepared["effective"]==true && prepared["saved"]==true && after_assignment!=initial && after_kill==after_assignment && durable_before && durable_after && steps.as_object().is_some_and(|steps|steps.values().all(|value|value==true)) && after==initial,
        "audio_app_pid":pid,"only_diagnostic_worker_killed":true,"prepared":prepared,"initial_policy":initial,"after_assignment":after_assignment,"after_worker_kill":after_kill,
        "durable_journal_survived":durable_before,"journal_empty_after":durable_after,"steps":steps,"external_choice":external_report,"policy_restored":after==initial}),
    )
}

fn verify_external_choice(
    broker: &Broker,
    pid: u32,
    original: &[Option<String>],
) -> Result<Value, String> {
    use jack_core::routing::{FileJournal, JournalStore, RouteController};
    let before = broker.call(Action::Read)?;
    let name = before
        .sonar
        .applications
        .iter()
        .find(|app| app.process_id == pid)
        .ok_or("Audio app ended")?
        .process_name
        .clone();
    let output = before
        .audio
        .endpoints
        .iter()
        .find(|endpoint| endpoint.flow == "render" && endpoint.form_factor == Some(9))
        .ok_or("Digital external output required")?
        .id
        .clone();
    let mut policy = WindowsAudioPolicy::new().map_err(|error| error.to_string())?;
    let binding = policy
        .bind_process(pid)
        .map_err(|error| error.to_string())?;
    let keys = crate::sonar_routing::app_keys(&binding);
    let read = |policy: &mut WindowsAudioPolicy| {
        keys.iter()
            .map(|key| policy.read(key))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())
    };
    broker.call(Action::Separation { enabled: true })?;
    broker.call(Action::AssignApplication {
        process_id: pid,
        process_name: name.clone(),
        channel: Some(Channel::Chat),
        automatic: false,
    })?;
    // The independent test actor retains the real pre-test undo records. Jack
    // must preserve its changes as foreign while the test can still undo safely.
    let mut undo = FileJournal {
        path: current_user_root()
            .map_err(|error| error.to_string())?
            .join("routes.json"),
    }
    .load()
    .map_err(|error| error.to_string())?;
    undo.changes.retain(|change| keys.contains(&change.key));
    let mut store = FileJournal {
        path: std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join("artifacts/sonar-crash/external-journal.json"),
    };
    if !store
        .load()
        .map_err(|error| error.to_string())?
        .changes
        .is_empty()
    {
        return Err("external test journal has pending recovery".into());
    }
    store.save(&undo).map_err(|error| error.to_string())?;
    let mut external = RouteController { policy, store };
    let operation = (|| {
        external
            .apply_batch(
                keys[..2]
                    .iter()
                    .cloned()
                    .map(|key| (key, output.clone()))
                    .collect(),
            )
            .map_err(|error| error.to_string())?;
        let foreign = read(&mut external.policy)?;
        let synced = broker.call(Action::Read)?;
        let preserved = read(&mut external.policy)? == foreign;
        let shown_as_external = synced
            .app_routing
            .iter()
            .any(|app| app.process_id == pid && app.origin == "external" && app.message.is_some());
        // Explicit reassignment resumes Jack control, then OFF returns to the
        // external preference, not the older pre-test Windows default.
        broker.call(Action::AssignApplication {
            process_id: pid,
            process_name: name,
            channel: Some(Channel::Chat),
            automatic: false,
        })?;
        let resumed = read(&mut external.policy)? != foreign;
        broker.call(Action::Separation { enabled: false })?;
        let returned_to_external = read(&mut external.policy)? == foreign;
        broker.call(Action::Reset)?;
        let reset_preserved = read(&mut external.policy)? == foreign;
        Ok::<_, String>(
            json!({"passed":preserved && shown_as_external && resumed && returned_to_external && reset_preserved,
            "foreign_policy":foreign,"read_preserved":preserved,"shown_as_external":shown_as_external,"explicit_assignment_resumed":resumed,"off_returned_to_external":returned_to_external,"reset_preserved_external":reset_preserved}),
        )
    })();
    // Stop Jack from reconciling while the independent actor restores its own
    // undo record. This cleanup executes even after a failed step.
    let off = broker.call(Action::Separation { enabled: false });
    let recovery = external
        .restore_best_effort()
        .map_err(|error| error.to_string())?;
    let after = read(&mut external.policy)?;
    off?;
    let details = operation?;
    Ok(
        json!({"passed":details["passed"]==true && recovery.complete && after==original,"details":details,"recovery":recovery,"policy_restored":after==original}),
    )
}
