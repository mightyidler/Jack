#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod audio;
mod bald_program_icons;
mod distribution;
mod group_outputs;
mod icon_cache;
mod idle_webview;
mod own_window_frame;
mod popup;
mod program_icons;
mod sonar_app_verification;
mod sonar_crash_verification;
mod sonar_independence_verification;
mod sonar_persistence_verification;
mod sonar_routing;
mod sonar_verification;
mod tray_menu;
mod tray_shortcut;
mod verification;
use audio::{Action, Broker, ViewState};
#[tauri::command]
async fn audio_volume(
    app: tauri::AppHandle,
    target: audio::LiveVolumeTarget,
    level: f64,
    expected: f64,
) -> Result<f64, String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        handle
            .state::<Broker>()
            .live_volume(target, level, expected)
    })
    .await
    .map_err(|e| e.to_string())?
}
use tauri::{
    Emitter, Manager,
    menu::MenuItem,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

struct UiProofPath {
    path: Option<std::path::PathBuf>,
    views: std::sync::Mutex<std::collections::BTreeMap<String, serde_json::Value>>,
}
struct TrayLabels {
    open: tauri::menu::MenuItem<tauri::Wry>,
    quit: tauri::menu::MenuItem<tauri::Wry>,
}
impl TrayLabels {
    fn update(&self, language: &str) -> Result<(), String> {
        let english = language == "en";
        self.open
            .set_text(if english { "Open Jack" } else { "Jack 열기" })
            .map_err(|e| e.to_string())?;
        self.quit
            .set_text(if english { "Quit Jack" } else { "Jack 종료" })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
#[tauri::command]
fn ui_ready(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    report: serde_json::Value,
) -> Result<(), String> {
    let state = app.state::<UiProofPath>();
    if let Some(path) = &state.path {
        let mut views = state.views.lock().map_err(|e| e.to_string())?;
        views.insert(window.label().into(), report.clone());
        let proof = serde_json::json!({"webview":window.label(),"native_ipc_received":true,"report":report,"views":*views});
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&proof).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    idle_webview::suspend_later(&window);
    Ok(())
}

#[tauri::command]
async fn audio_action(app: tauri::AppHandle, action: Action) -> Result<ViewState, String> {
    if matches!(action, Action::SonarRetry) {
        distribution::start_gg_for_retry(app.clone()).await?;
    }
    let preference_change = matches!(
        action,
        Action::Preferences { .. } | Action::TrayShortcut { .. } | Action::Reset
    );
    let next_shortcut = match &action {
        Action::TrayShortcut {
            enabled,
            key,
            modifiers,
        } => Some(jack_core::TrayShortcut {
            enabled: *enabled,
            key: key.clone(),
            modifiers: *modifiers,
        }),
        Action::Reset => Some(jack_core::TrayShortcut::default()),
        _ => None,
    };
    let previous_shortcut = tray_shortcut::current(&app);
    if let Some(config) = next_shortcut.as_ref() {
        tray_shortcut::configure(&app, config.clone()).await?;
    }
    let handle = app.clone();
    let outcome =
        tauri::async_runtime::spawn_blocking(move || handle.state::<Broker>().call(action))
            .await
            .map_err(|e| e.to_string())
            .and_then(|value| value);
    let state = match outcome {
        Ok(state) => state,
        Err(error) => {
            if next_shortcut.is_some() {
                let _ = tray_shortcut::configure(&app, previous_shortcut).await;
            }
            return Err(error);
        }
    };
    if preference_change {
        app.state::<TrayLabels>().update(&state.settings.language)?;
        // Hidden tray WebViews must receive the palette before their next open.
        app.emit("jack-preferences", &state.settings)
            .map_err(|e| e.to_string())?;
    }
    Ok(state)
}
#[tauri::command]
async fn quit(app: tauri::AppHandle) -> Result<(), String> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || handle.state::<Broker>().call(Action::Shutdown))
        .await
        .map_err(|e| e.to_string())??;
    app.exit(0);
    Ok(())
}
fn show(app: &tauri::AppHandle) {
    tray_menu::hide(app);
    popup::hide(app);
    if let Some(window) = app.get_webview_window("main") {
        idle_webview::resume(&window);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("jack-visibility", true);
    }
}
#[tauri::command]
fn resize_quick(app: tauri::AppHandle, height: f64) -> Result<popup::Layout, String> {
    popup::resize(&app, height)
}
#[tauri::command]
async fn begin_window_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the main window can be dragged".into());
    }
    let (done, completion) = std::sync::mpsc::sync_channel(1);
    let dragging_window = window.clone();
    window
        .run_on_main_thread(move || {
            let result = dragging_window
                .hwnd()
                .map_err(|e| e.to_string())
                .and_then(|hwnd| own_window_frame::drag(windows::Win32::Foundation::HWND(hwnd.0)));
            let _ = done.send(result);
        })
        .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || completion.recv().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
fn hide_to_tray(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() == "quick" {
        popup::hide(window.app_handle());
        return Ok(());
    }
    window.hide().map_err(|e| e.to_string())?;
    let _ = window.emit("jack-visibility", false);
    idle_webview::suspend_later(&window);
    Ok(())
}
#[tauri::command]
fn open_devtools(window: tauri::WebviewWindow) {
    if window.label() == "quick" {
        popup::inspect(window.app_handle());
    }
    // WebView2 supports opening the inspector, but Wry's Windows close/status
    // APIs are no-ops. Close the inspector with its own window controls.
    window.open_devtools();
}
#[tauri::command]
fn minimize_window(window: tauri::WebviewWindow) -> Result<(), String> {
    window.minimize().map_err(|e| e.to_string())?;
    let _ = window.emit("jack-visibility", false);
    idle_webview::suspend_later(&window);
    Ok(())
}
#[tauri::command]
fn window_visible(window: tauri::WebviewWindow) -> Result<bool, String> {
    Ok(window.is_visible().map_err(|e| e.to_string())?
        && !window.is_minimized().map_err(|e| e.to_string())?
        && (window.label() != "quick" || !popup::is_closing(window.app_handle())))
}
#[tauri::command]
fn window_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let mut windows = serde_json::Map::new();
    for label in ["main", "quick"] {
        if let Some(window) = app.get_webview_window(label) {
            windows.insert(
                label.into(),
                serde_json::json!({
                    "visible":window.is_visible().map_err(|e| e.to_string())?,
                "focused":window.is_focused().map_err(|e| e.to_string())?,
                "hwnd":window.hwnd().map_err(|e| e.to_string())?.0 as usize,
                "innerPosition":window.inner_position().map_err(|e| e.to_string())?,
                    "minimized":window.is_minimized().map_err(|e| e.to_string())?,
                    "position":window.outer_position().map_err(|e| e.to_string())?,
                    "size":window.outer_size().map_err(|e| e.to_string())?,
                    "scale":window.scale_factor().map_err(|e| e.to_string())?
                }),
            );
        }
    }
    let labels = app.state::<TrayLabels>();
    Ok(
        serde_json::json!({"windows":windows,"monitors":app.available_monitors().map_err(|e| e.to_string())?,"trayLabels":[labels.open.text().map_err(|e| e.to_string())?,labels.quit.text().map_err(|e| e.to_string())?]}),
    )
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--inspect-program-icons") {
        let result = args
            .get(index + 1)
            .ok_or("report path required")
            .and_then(|path| {
                program_icons::verify(std::path::Path::new(path))
                    .map_err(|_| "program icon inspection failed")
            });
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--prepare-sonar-crash") {
        let result = (|| {
            let pid = args
                .get(index + 1)
                .ok_or("PID required")?
                .parse()
                .map_err(|_| "invalid PID")?;
            let path = args.get(index + 2).ok_or("ready file required")?;
            sonar_crash_verification::prepare(pid, std::path::Path::new(path))
        })();
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--verify-sonar-crash") {
        let result = args
            .get(index + 1)
            .ok_or("PID required")
            .and_then(|value| value.parse().map_err(|_| "invalid PID"))
            .map_err(String::from)
            .and_then(sonar_crash_verification::run);
        match result {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize")
                );
                if report["passed"] != true {
                    std::process::exit(2);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--sonar-tone") {
        let result = (|| {
            let source = args.get(index + 1).ok_or("source required")?;
            let frequency = args.get(index + 2).ok_or("frequency required")?.parse()?;
            jack_windows_audio::sonar_probe::play_tone(source, frequency, 30)
        })();
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args.iter().any(|arg| arg == "--verify-sonar-independence") {
        match sonar_independence_verification::run() {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize")
                );
                if report["passed"] != true {
                    std::process::exit(2);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(index) = args
        .iter()
        .position(|arg| arg == "--verify-sonar-persistence")
    {
        let report = args
            .get(index + 1)
            .ok_or("PID required")
            .and_then(|s| s.parse::<u32>().map_err(|_| "invalid PID"))
            .map_err(String::from)
            .and_then(sonar_persistence_verification::run);
        match report {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize")
                );
                if report["passed"] != true {
                    std::process::exit(2);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if let Some(index) = args.iter().position(|arg| arg == "--verify-sonar-app") {
        let report = args
            .get(index + 1)
            .ok_or("PID required")
            .and_then(|pid| pid.parse::<u32>().map_err(|_| "invalid PID"))
            .map_err(String::from)
            .and_then(sonar_app_verification::run);
        match report {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize")
                );
                if report["passed"] != true {
                    std::process::exit(2);
                }
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--verify-sonar-pcm") {
        match sonar_verification::run_pcm() {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize")
                );
                if report["passed"] != true {
                    std::process::exit(2);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--verify-sonar") {
        match sonar_verification::run() {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize")
                );
                if report["passed"] != true {
                    std::process::exit(2);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--verify-controls") {
        match verification::run() {
            Ok(report) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).expect("serialize report")
                );
                if report["passed"] != true {
                    std::process::exit(2);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--inspect") {
        let result = (|| {
            let broker = Broker::start()?;
            let state = broker.call(Action::Read)?;
            let json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
            broker.call(Action::Shutdown)?;
            Ok::<_, String>(json)
        })();
        match result {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    let background = std::env::args().any(|arg| arg == "--background");
    let quick_preview = std::env::args().any(|arg| arg == "--quick-preview");
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|arg| arg == "--exit-for-install") {
                distribution::exit_for_install(app.clone());
                return;
            }
            if let Some(index) = args.iter().position(|a| a == "--resource-status") {
                if let Some(path) = args.get(index + 1) {
                    idle_webview::report(app, std::path::PathBuf::from(path));
                }
                return;
            }
            if args.iter().any(|a| a == "--quick-preview") {
                if let Err(error) = popup::preview(app) {
                    let _ = app.emit("jack-error", error);
                }
            } else if !args.iter().any(|a| a == "--background") {
                show(app);
            }
        }))
        .invoke_handler(tauri::generate_handler![
            audio_action,
            audio_volume,
            quit,
            hide_to_tray,
            minimize_window,
            open_devtools,
            window_visible,
            ui_ready,
            resize_quick,
            begin_window_drag,
            window_status,
            tray_menu::tray_menu_action,
            distribution::gg_action,
            distribution::check_jack_update,
            distribution::install_jack_update,
            tray_shortcut::shortcut_recording
        ])
        .setup(move |app| {
            if std::env::args().any(|arg| arg == "--exit-for-install") {
                app.handle().exit(0);
                return Ok(());
            }
            app.manage(distribution::State::default());
            app.manage(idle_webview::State::default());
            if let Some(window) = app.get_webview_window("main") {
                own_window_frame::install(windows::Win32::Foundation::HWND(window.hwnd()?.0))
                    .map_err(std::io::Error::other)?;
            }
            app.manage(UiProofPath {
                path: std::env::var_os("JACK_UI_PROOF_PATH").map(std::path::PathBuf::from),
                views: Default::default(),
            });
            app.manage(popup::PopupState::default());
            app.manage(tray_menu::State::default());
            app.manage(tray_shortcut::State::default());
            app.manage(Broker::start().map_err(std::io::Error::other)?);
            // Production startup registration is separate from audio diagnostics.
            if let Err(error) = app.state::<Broker>().call(Action::InitializeStartup) {
                eprintln!("Could not register Jack startup: {error}");
            }
            let open = MenuItem::with_id(app, "open", "Jack 열기", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Jack 종료", true, None::<&str>)?;
            app.manage(TrayLabels {
                open,
                quit: quit_item,
            });
            let initial = app
                .state::<Broker>()
                .call(Action::Read)
                .map_err(std::io::Error::other)?;
            app.state::<TrayLabels>()
                .update(&initial.settings.language)
                .map_err(std::io::Error::other)?;
            if let Err(error) =
                tray_shortcut::install(app.handle(), initial.settings.tray_shortcut.clone())
            {
                let _ = app.emit("jack-error", error);
            }
            let skip_double_click_release = std::sync::atomic::AtomicBool::new(false);
            let last_click =
                std::sync::Mutex::new(None::<(std::time::Instant, tauri::PhysicalPosition<f64>)>);
            TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/32x32.png"
                ))?)
                .tooltip("Jack · Just Audio Control Knob")
                .show_menu_on_left_click(false)
                .on_tray_icon_event(move |tray, event| match event {
                    TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } => {
                        // Windows sends the second release after DoubleClick.
                        // Do not reopen the popup over the newly opened app.
                        if skip_double_click_release
                            .swap(true, std::sync::atomic::Ordering::Relaxed)
                        {
                            return;
                        }
                        if let Ok(mut last) = last_click.lock() {
                            *last = None;
                        }
                        popup::hide_now(tray.app_handle());
                        show(tray.app_handle());
                    }
                    TrayIconEvent::Click {
                        position,
                        rect,
                        button: MouseButton::Left,
                        button_state,
                        ..
                    } => {
                        // Immediate single click. Also recognize a second press ourselves:
                        // moving focus into the popup can disrupt Windows' DoubleClick event.
                        let recent = last_click.lock().is_ok_and(|last| {
                            last.as_ref().is_some_and(|(at, point)| {
                                use windows::Win32::UI::{
                                    Input::KeyboardAndMouse::GetDoubleClickTime,
                                    WindowsAndMessaging::{
                                        GetSystemMetrics, SM_CXDOUBLECLK, SM_CYDOUBLECLK,
                                    },
                                };
                                unsafe {
                                    at.elapsed().as_millis() <= GetDoubleClickTime() as u128
                                        && (point.x - position.x).abs()
                                            <= GetSystemMetrics(SM_CXDOUBLECLK) as f64 / 2.0
                                        && (point.y - position.y).abs()
                                            <= GetSystemMetrics(SM_CYDOUBLECLK) as f64 / 2.0
                                }
                            })
                        });
                        if button_state == MouseButtonState::Down {
                            if recent {
                                if let Ok(mut last) = last_click.lock() {
                                    *last = None;
                                }
                                skip_double_click_release
                                    .store(true, std::sync::atomic::Ordering::Relaxed);
                                popup::hide_now(tray.app_handle());
                                show(tray.app_handle());
                            }
                            return;
                        }
                        if skip_double_click_release
                            .swap(false, std::sync::atomic::Ordering::Relaxed)
                        {
                            return;
                        }
                        if let Ok(mut last) = last_click.lock() {
                            *last = if recent {
                                None
                            } else {
                                Some((std::time::Instant::now(), position))
                            };
                        }
                        if recent {
                            popup::hide_now(tray.app_handle());
                            show(tray.app_handle());
                            return;
                        }
                        tray_menu::hide(tray.app_handle());
                        if let Err(error) = popup::toggle(tray.app_handle(), position, rect) {
                            let _ = tray.app_handle().emit("jack-error", error);
                        }
                    }
                    TrayIconEvent::Click {
                        position,
                        rect,
                        button: MouseButton::Right,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        if let Ok(mut last) = last_click.lock() {
                            *last = None;
                        }
                        skip_double_click_release
                            .store(false, std::sync::atomic::Ordering::Relaxed);
                        if let Err(error) = tray_menu::show(tray.app_handle(), position, rect) {
                            let _ = tray.app_handle().emit("jack-error", error);
                        }
                    }
                    _ => {}
                })
                .build(app)?;
            if !background {
                show(app.handle());
            }
            if quick_preview {
                popup::preview(app.handle()).map_err(std::io::Error::other)?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "tray-menu" && matches!(event, tauri::WindowEvent::Focused(false))
            {
                tray_menu::hide(window.app_handle());
            }
            if matches!(event, tauri::WindowEvent::Focused(true))
                && let Some(webview) = window.app_handle().get_webview_window(window.label())
            {
                idle_webview::resume(&webview);
            }
            if matches!(event, tauri::WindowEvent::Resized(_))
                && window.is_minimized().unwrap_or(false)
                && let Some(webview) = window.app_handle().get_webview_window(window.label())
            {
                idle_webview::suspend_later(&webview);
            }
            if window.label() == "quick" && matches!(event, tauri::WindowEvent::Focused(false)) {
                popup::lost_focus(window.app_handle());
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if window.label() == "tray-menu" {
                    tray_menu::hide(window.app_handle());
                    return;
                }
                if window.label() == "quick" {
                    popup::hide(window.app_handle());
                    return;
                }
                let _ = window.hide();
                let _ = window.emit("jack-visibility", false);
                if let Some(webview) = window.app_handle().get_webview_window(window.label()) {
                    idle_webview::suspend_later(&webview);
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("Jack desktop initialization failed");
}
