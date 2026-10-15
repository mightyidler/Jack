//! Fixed-size custom tray menu; all placement uses physical monitor work areas.
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize};

#[derive(Default)]
pub struct State(AtomicU64);

pub fn show(
    app: &tauri::AppHandle,
    point: PhysicalPosition<f64>,
    _rect: tauri::Rect,
) -> Result<(), String> {
    crate::popup::right_click(app);
    let window = app
        .get_webview_window("tray-menu")
        .ok_or("tray menu unavailable")?;
    let monitor = app
        .monitor_from_point(point.x, point.y)
        .map_err(|e| e.to_string())?
        .ok_or("tray monitor unavailable")?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    // Context menu follows the actual cursor, not the audio panel's tray anchor.
    // The 8px transparent gutter ends exactly at the cursor's vertical position.
    let width = (176.0 * scale).round().min(area.size.width as f64);
    let height = (88.0 * scale).round().min(area.size.height as f64);
    let left = area.position.x as f64;
    let top = area.position.y as f64;
    // The visible panel's lower LEFT corner, excluding its shadow gutter,
    // meets the cursor. Clamp only when the screen boundary leaves no room.
    let x = (point.x - 8.0 * scale).clamp(left, left + area.size.width as f64 - width);
    let y = (if point.y - height >= top {
        point.y - height + 8.0 * scale
    } else {
        point.y - 8.0 * scale
    })
    .clamp(top, top + area.size.height as f64 - height);
    app.state::<State>().0.fetch_add(1, Ordering::Relaxed);
    window
        .set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
        .map_err(|e| e.to_string())?;
    // Apply the physical size after changing monitors and their DPI.
    window
        .set_size(PhysicalSize::new(
            width.round() as u32,
            height.round() as u32,
        ))
        .map_err(|e| e.to_string())?;
    crate::idle_webview::resume(&window);
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    window
        .emit("jack-tray-menu-visible", true)
        .map_err(|e| e.to_string())
}

pub fn hide(app: &tauri::AppHandle) {
    let generation = app.state::<State>().0.fetch_add(1, Ordering::Relaxed) + 1;
    if let Some(window) = app.get_webview_window("tray-menu") {
        let _ = window.emit("jack-tray-menu-visible", false);
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if handle.state::<State>().0.load(Ordering::Relaxed) != generation {
                return;
            }
            if let Some(window) = handle.get_webview_window("tray-menu") {
                let _ = window.hide();
                crate::idle_webview::suspend_later(&window);
            }
        });
    });
}

#[tauri::command]
pub async fn tray_menu_action(app: tauri::AppHandle, action: String) -> Result<(), String> {
    hide(&app);
    match action.as_str() {
        "open" => {
            crate::show(&app);
            Ok(())
        }
        "quit" => crate::quit(app).await,
        "hide" => Ok(()),
        _ => Err("unknown tray menu action".into()),
    }
}
