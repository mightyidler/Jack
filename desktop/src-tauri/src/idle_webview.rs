//! Sleep hidden page renderers; the native audio owner remains active.
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{Manager, WebviewWindow};
use webview2_com::{Microsoft::Web::WebView2::Win32::ICoreWebView2_3, TrySuspendCompletedHandler};
use windows::core::{BOOL, Interface};

#[derive(Default)]
pub struct State {
    results: Mutex<BTreeMap<String, bool>>,
}

pub fn suspend_later(window: &WebviewWindow) {
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        // Let close/drag completion IPC settle before pausing page scripts.
        settle_delay().await;
        let check = window.clone();
        let app = window.app_handle().clone();
        let label = window.label().to_string();
        let _ = window.with_webview(move |webview| {
            if check.is_visible().unwrap_or(true) && !check.is_minimized().unwrap_or(false) {
                return;
            }
            let callback_app = app.clone();
            let callback_label = label.clone();
            let result = (|| -> windows::core::Result<()> {
                unsafe {
                    let controller = webview.controller();
                    controller.SetIsVisible(false)?;
                    let core = controller.CoreWebView2()?.cast::<ICoreWebView2_3>()?;
                    let handler =
                        TrySuspendCompletedHandler::create(Box::new(move |error, success| {
                            if let Some(state) = callback_app.try_state::<State>()
                                && let Ok(mut records) = state.results.lock()
                            {
                                records.insert(callback_label, error.is_ok() && success);
                            }
                            Ok(())
                        }));
                    core.TrySuspend(&handler)
                }
            })();
            if result.is_err()
                && let Some(state) = app.try_state::<State>()
                && let Ok(mut records) = state.results.lock()
            {
                records.insert(label, false);
            }
        });
    });
}
async fn settle_delay() {
    // Runtime dependency is provided by Tauri; a sleeping worker does not poll audio.
    tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_secs(1)))
        .await
        .ok();
}
pub fn resume(window: &WebviewWindow) {
    let _ = window.with_webview(|webview| unsafe {
        let controller = webview.controller();
        if let Ok(core) = controller
            .CoreWebView2()
            .and_then(|c| c.cast::<ICoreWebView2_3>())
        {
            let _ = core.Resume();
        }
        let _ = controller.SetIsVisible(true);
    });
}

/// Opt-in resource diagnostics only. No page content, GUI action or audio mutation.
pub fn report(app: &tauri::AppHandle, path: PathBuf) {
    let records = Arc::new(Mutex::new(BTreeMap::<String, serde_json::Value>::new()));
    let windows: Vec<_> = ["main", "quick"]
        .into_iter()
        .filter_map(|label| app.get_webview_window(label).map(|window| (label, window)))
        .collect();
    let expected = windows.len();
    for (label, window) in windows {
        let output = records.clone();
        let path = path.clone();
        let app = app.clone();
        let visible = window.is_visible().unwrap_or(false);
        let minimized = window.is_minimized().unwrap_or(false);
        let _ = window.with_webview(move |webview| {
            let suspended = (|| -> windows::core::Result<bool> {
                unsafe {
                    let core = webview
                        .controller()
                        .CoreWebView2()?
                        .cast::<ICoreWebView2_3>()?;
                    let mut value = BOOL(0);
                    core.IsSuspended(&mut value)?;
                    Ok(value.as_bool())
                }
            })();
            let last = app.try_state::<State>().and_then(|state| {
                state
                    .results
                    .lock()
                    .ok()
                    .and_then(|r| r.get(label).copied())
            });
            if let Ok(mut output) = output.lock() {
                output.insert(
                    label.into(),
                    serde_json::json!({
                        "visible": visible,
                        "minimized": minimized,
                        "suspended": suspended.ok(),
                        "last_suspend_succeeded": last
                    }),
                );
                if output.len() == expected
                    && let Ok(mut file) = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                {
                    use std::io::Write;
                    if let Ok(bytes) = serde_json::to_vec_pretty(&serde_json::json!({
                        "process_id": std::process::id(), "read_only": true, "windows": *output
                    })) {
                        let _ = file.write_all(&bytes);
                    }
                }
            }
        });
    }
}
