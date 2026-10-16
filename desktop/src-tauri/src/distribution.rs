//! Dependency setup and signed Jack updates; no arbitrary executable or URL IPC.
use crate::audio::{Action, Broker};
use serde::Serialize;
use std::{
    future::Future,
    os::windows::process::CommandExt,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct State {
    pending: Mutex<Option<Update>>,
    operation: Mutex<()>,
    update_operation: tauri::async_runtime::Mutex<()>,
    updating: AtomicBool,
    startup_checked: AtomicBool,
}
impl State {
    fn ensure_setup_allowed(&self) -> Result<(), String> {
        if self.updating.load(Ordering::Acquire) {
            return Err("Jack is preparing an update".into());
        }
        Ok(())
    }
    fn begin_update(&self) -> Result<(), String> {
        let _setup = self
            .operation
            .try_lock()
            .map_err(|_| "Another setup operation is running")?;
        self.updating
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| "An update is already being applied".into())
    }
}

struct UpdateGuard(tauri::AppHandle);
impl Drop for UpdateGuard {
    fn drop(&mut self) {
        self.0
            .state::<State>()
            .updating
            .store(false, Ordering::Release);
    }
}

pub fn exit_for_install(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let handle = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            handle.state::<Broker>().call(Action::Shutdown)
        })
        .await;
        match result {
            Ok(Ok(_)) => app.exit(0),
            _ => {
                let _ = app.emit("jack-error", "Could not restore audio routes. Quit Jack from settings and retry installation.");
            }
        }
    });
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GgOperation {
    Inspect,
    Latest,
    Install,
    Open,
    Start,
}

fn run_gg(operation: GgOperation, consent: bool) -> Result<serde_json::Value, String> {
    if matches!(operation, GgOperation::Install) && !consent {
        return Err("Explicit consent to install or update SteelSeries GG is required".into());
    }
    // Per-call private directory; embedded script cannot depend on the checkout path.
    let folder = std::env::temp_dir().join(format!("Jack-setup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&folder).map_err(|e| e.to_string())?;
    let path = folder.join("steelseries-gg.ps1");
    let script = include_str!("../windows/steelseries-gg.ps1").trim_start_matches('\u{feff}');
    let result = (|| {
        // Windows PowerShell 5.1 needs a BOM for the Korean installer text.
        std::fs::write(&path, format!("\u{feff}{script}")).map_err(|e| e.to_string())?;
        let powershell = std::path::PathBuf::from(
            std::env::var_os("SystemRoot").ok_or("Windows directory is unavailable")?,
        )
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let mut command = Command::new(powershell);
        command
            .creation_flags(0x08000000) // CREATE_NO_WINDOW; GG's official installer has its own UI.
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&path)
            .arg("-Mode")
            .arg(match operation {
                GgOperation::Inspect => "Inspect",
                GgOperation::Latest => "Latest",
                GgOperation::Install => "Install",
                GgOperation::Open => "Open",
                GgOperation::Start => "Start",
            });
        if consent {
            command.arg("-Consent");
        }
        let output = command.output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_dir(folder);
    result
}

/// Only an explicit Retry starts an existing GG installation. Background reads
/// keep reconnecting without launching programs or reinstalling dependencies.
pub async fn start_gg_for_retry(app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let _guard = state
            .operation
            .try_lock()
            .map_err(|_| "Another setup operation is running")?;
        state.ensure_setup_allowed()?;
        run_gg(GgOperation::Start, false).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn gg_action(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    operation: GgOperation,
    consent: bool,
) -> Result<serde_json::Value, String> {
    if window.label() != "main" {
        return Err("Dependency setup is available in the main window".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<State>();
        let _guard = state
            .operation
            .try_lock()
            .map_err(|_| "Another setup operation is running")?;
        state.ensure_setup_allowed()?;
        run_gg(operation, consent)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct UpdateInfo {
    current: String,
    version: Option<String>,
    notes: Option<String>,
}

#[tauri::command]
pub async fn check_jack_update(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<UpdateInfo, String> {
    if window.label() != "main" {
        return Err("Updates are available in the main window".into());
    }
    let state = app.state::<State>();
    let _operation = state
        .update_operation
        .try_lock()
        .map_err(|_| "Another update operation is running")?;
    if state.updating.load(Ordering::Acquire) {
        return Err("An update is already being applied".into());
    }
    check_update(&app).await
}

async fn check_update(app: &tauri::AppHandle) -> Result<UpdateInfo, String> {
    let mut update = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(update) = &mut update {
        update.timeout = Some(std::time::Duration::from_secs(300));
    }
    let info = UpdateInfo {
        current: app.package_info().version.to_string(),
        version: update.as_ref().map(|u| u.version.clone()),
        notes: update.as_ref().and_then(|u| u.body.clone()),
    };
    *app.state::<State>()
        .pending
        .lock()
        .map_err(|e| e.to_string())? = update;
    Ok(info)
}

#[tauri::command]
pub async fn install_jack_update(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Updates are available in the main window".into());
    }
    let state = app.state::<State>();
    let _operation = state
        .update_operation
        .try_lock()
        .map_err(|_| "Another update operation is running")?;
    apply_update(&app).await
}

async fn apply_update(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<State>();
    state.begin_update()?;
    let _guard = UpdateGuard(app.clone());
    let update = app
        .state::<State>()
        .pending
        .lock()
        .map_err(|e| e.to_string())?
        .take()
        .ok_or("Check for a Jack update first")?;
    let progress_app = app.clone();
    // download() verifies the updater signature before audio routing is paused.
    let bytes = update
        .download(
            move |chunk, total| {
                let _ = progress_app.emit(
                    "jack-update-progress",
                    serde_json::json!({"chunk": chunk,"total":total}),
                );
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        handle
            .state::<Broker>()
            .call(Action::UpdatePause { paused: true })
    })
    .await
    .map_err(|e| e.to_string())??;
    // On Windows the updater launches the installer and exits the current process.
    if let Err(error) = update.install(bytes) {
        let handle = app.clone();
        let resumed = tauri::async_runtime::spawn_blocking(move || {
            handle
                .state::<Broker>()
                .call(Action::UpdatePause { paused: false })
        })
        .await
        .map_err(|e| e.to_string())?;
        resumed?;
        return Err(error.to_string());
    }
    app.restart();
}

// The process owns this task, so hidden/suspended WebViews cannot skip it.
// Failed checks are not retried until the next launch. Manual checks remain available.
async fn run_startup_update(
    state: &State,
    check: impl Future<Output = Result<bool, String>>,
    install: impl Future<Output = Result<(), String>>,
) -> Result<(), String> {
    if state.startup_checked.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let _operation = state.update_operation.lock().await;
    if check.await? {
        install.await?;
    }
    Ok(())
}

pub fn start_update_check(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let result = run_startup_update(
            &app.state::<State>(),
            async { check_update(&app).await.map(|info| info.version.is_some()) },
            apply_update(&app),
        )
        .await;
        match result {
            Ok(()) => eprintln!("Jack startup update check completed"),
            Err(error) => eprintln!("Jack startup update failed: {error}"),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_checks_once_and_installs_only_when_a_new_version_exists() {
        for available in [false, true] {
            let state = State::default();
            let checks = std::sync::atomic::AtomicUsize::new(0);
            let installs = std::sync::atomic::AtomicUsize::new(0);
            for _ in 0..2 {
                tauri::async_runtime::block_on(run_startup_update(
                    &state,
                    async {
                        assert!(state.update_operation.try_lock().is_err());
                        checks.fetch_add(1, Ordering::SeqCst);
                        Ok(available)
                    },
                    async {
                        assert!(state.update_operation.try_lock().is_err());
                        installs.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                ))
                .unwrap();
            }
            assert_eq!(checks.load(Ordering::SeqCst), 1);
            assert_eq!(installs.load(Ordering::SeqCst), usize::from(available));
        }
    }

    #[test]
    fn failed_startup_check_or_install_does_not_retry_or_block_manual_updates() {
        for check_fails in [false, true] {
            let state = State::default();
            let result = tauri::async_runtime::block_on(run_startup_update(
                &state,
                async {
                    if check_fails {
                        Err("offline".into())
                    } else {
                        Ok(true)
                    }
                },
                async {
                    assert!(!check_fails);
                    Err("installation failed".into())
                },
            ));
            assert!(result.is_err());
            assert!(state.update_operation.try_lock().is_ok());
            assert!(state.ensure_setup_allowed().is_ok());
            tauri::async_runtime::block_on(run_startup_update(
                &state,
                async { panic!("startup check must not retry") },
                async { panic!("installation must not retry") },
            ))
            .unwrap();
        }
    }
    #[test]
    fn setup_and_update_exclude_each_other_and_concurrent_checks_are_rejected() {
        let state = State::default();
        let setup = state.operation.lock().unwrap();
        assert!(state.begin_update().is_err());
        assert!(!state.updating.load(Ordering::Acquire));
        drop(setup);
        state.begin_update().unwrap();
        assert!(state.ensure_setup_allowed().is_err());
        assert!(state.begin_update().is_err());
        state.updating.store(false, Ordering::Release);
        assert!(state.ensure_setup_allowed().is_ok());
        let check = state.update_operation.try_lock().unwrap();
        assert!(state.update_operation.try_lock().is_err());
        drop(check);
        assert!(state.update_operation.try_lock().is_ok());
    }
}
