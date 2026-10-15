//! Tray geometry uses physical work areas, including monitors with negative origins.
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize};
use windows::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::{CreateRectRgn, DeleteObject, HGDIOBJ, SetWindowRgn},
    UI::WindowsAndMessaging::{GetCursorPos, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos},
};

const WIDTH: f64 = 360.0; // Latest JSON: 302px inner + 18px panel padding/border + 40px gutter
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Bounds {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }
    fn valid(&self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }
}
#[derive(Clone, Copy)]
struct Anchor {
    rect: Bounds,
    x: f64,
    y: f64,
}
#[derive(Default)]
pub struct PopupState {
    inner: Mutex<PopupInner>,
}
#[derive(Default)]
struct PopupInner {
    anchor: Option<Anchor>,
    dismissed_at_tray: Option<Instant>,
    height: f64,
    transition: u64,
    closing: bool,
    inspecting: bool,
    host: Option<Bounds>,
}

#[derive(Clone, serde::Serialize)]
pub struct Layout {
    anchor: f64,
    origin: f64,
}

fn dock_edge(work: Bounds, anchor: Bounds) -> usize {
    let cx = anchor.x + anchor.width / 2.0;
    let cy = anchor.y + anchor.height / 2.0;
    let distances = [
        (cx - work.x).abs(),
        (cx - work.x - work.width).abs(),
        (cy - work.y).abs(),
        (cy - work.y - work.height).abs(),
    ];
    (0..4)
        .min_by(|a, b| distances[*a].total_cmp(&distances[*b]))
        .expect("four edges")
}

pub fn placement(work: Bounds, anchor: Bounds, scale: f64, height: f64) -> Result<Bounds, String> {
    placement_for_width(work, anchor, scale, WIDTH, height)
}
pub fn placement_for_width(
    work: Bounds,
    anchor: Bounds,
    scale: f64,
    logical_width: f64,
    height: f64,
) -> Result<Bounds, String> {
    if !work.valid()
        || !anchor.valid()
        || !scale.is_finite()
        || scale <= 0.0
        || !height.is_finite()
        || height <= 0.0
        || !logical_width.is_finite()
        || logical_width <= 0.0
    {
        return Err("invalid popup geometry".into());
    }
    let width = (logical_width * scale).round().min(work.width);
    let height = (height * scale).round().clamp(1.0, work.height);
    let gap = 8.0 * scale;
    // Choose the closest work-area edge to the tray, then place away from it.
    let cx = anchor.x + anchor.width / 2.0;
    let cy = anchor.y + anchor.height / 2.0;
    let edge = dock_edge(work, anchor);
    let (x, y) = match edge {
        0 => (anchor.x + anchor.width + gap, cy - height / 2.0),
        1 => (anchor.x - width - gap, cy - height / 2.0),
        2 => (cx - width / 2.0, anchor.y + anchor.height + gap),
        _ => (cx - width / 2.0, anchor.y - height - gap),
    };
    Ok(Bounds {
        x: x.clamp(work.x, work.x + work.width - width),
        y: y.clamp(work.y, work.y + work.height - height),
        width,
        height,
    })
}
fn work(monitor: &tauri::Monitor) -> Bounds {
    let area = monitor.work_area();
    Bounds {
        x: area.position.x as f64,
        y: area.position.y as f64,
        width: area.size.width as f64,
        height: area.size.height as f64,
    }
}
pub fn resize(app: &tauri::AppHandle, height: f64) -> Result<Layout, String> {
    if !height.is_finite() || !(64.0..=20000.0).contains(&height) {
        return Err("invalid popup height".into());
    }
    let anchor = {
        let state = app.state::<PopupState>();
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.height = height;
        inner.anchor
    };
    let window = app
        .get_webview_window("quick")
        .ok_or("quick window unavailable")?;
    let monitor = match anchor {
        Some(anchor) => app
            .monitor_from_point(anchor.x, anchor.y)
            .map_err(|e| e.to_string())?,
        None => app.primary_monitor().map_err(|e| e.to_string())?,
    }
    .ok_or("monitor unavailable")?;
    let area = work(&monitor);
    let fallback = Anchor {
        rect: Bounds {
            x: area.x + area.width - 32.0,
            y: area.y + area.height - 1.0,
            width: 24.0,
            height: 24.0,
        },
        x: area.x + area.width - 12.0,
        y: area.y + area.height - 1.0,
    };
    let anchor = anchor.unwrap_or(fallback).rect;
    let scale = monitor.scale_factor();
    let bounds = placement(area, anchor, scale, height)?;
    // Keep WebView2's rendering viewport stable throughout a visible session.
    // Resizing its swap chain at the end of an otherwise completed animation can
    // present a stretched/stale intermediate frame. The panel remains content
    // sized; a native region excludes the unused host from painting AND input.
    let host = Bounds {
        x: bounds.x,
        y: area.y,
        width: bounds.width,
        height: area.height,
    };
    let reposition = app
        .state::<PopupState>()
        .inner
        .lock()
        .map_err(|e| e.to_string())?
        .host
        != Some(host);
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    unsafe {
        if reposition {
            SetWindowPos(
                hwnd,
                None,
                host.x.round() as i32,
                host.y.round() as i32,
                host.width.round() as i32,
                host.height.round() as i32,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .map_err(|e| e.to_string())?;
            app.state::<PopupState>()
                .inner
                .lock()
                .map_err(|e| e.to_string())?
                .host = Some(host);
        }
        let region = CreateRectRgn(
            0,
            (bounds.y - host.y).round() as i32,
            host.width.round() as i32,
            (bounds.y - host.y + bounds.height).round() as i32,
        );
        if region.is_invalid() {
            return Err("popup region creation failed".into());
        }
        // The CSS compositor paints its own animation. Do not invalidate/repaint
        // the whole WebView when only its transparent input envelope changes.
        if SetWindowRgn(hwnd, Some(region), false) == 0 {
            let _ = DeleteObject(HGDIOBJ(region.0));
            return Err("popup region update failed".into());
        }
    }
    let (anchor_y, origin) = match dock_edge(area, anchor) {
        2 => (anchor.y + anchor.height + 8.0 * scale, 0.0),
        3 => (anchor.y - 8.0 * scale, 1.0),
        _ => (anchor.y + anchor.height / 2.0, 0.5),
    };
    let layout = Layout {
        anchor: (anchor_y - area.y) / scale,
        origin,
    };
    window
        .emit("jack-popup-layout", &layout)
        .map_err(|e| e.to_string())?;
    Ok(layout)
}
/// Promote a tray double-click to the app without replaying a popup exit.
pub fn hide_now(app: &tauri::AppHandle) {
    if let Ok(mut inner) = app.state::<PopupState>().inner.lock() {
        inner.transition = inner.transition.wrapping_add(1);
        inner.closing = true;
        inner.inspecting = false;
        inner.dismissed_at_tray = None;
    }
    if let Some(window) = app.get_webview_window("quick") {
        let _ = window.emit("jack-visibility", false);
        let _ = window.hide();
        let _ = window.emit("jack-popup-hidden", ());
        crate::idle_webview::suspend_later(&window);
    }
}
pub fn hide(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("quick") {
        let transition = {
            let state = app.state::<PopupState>();
            let Ok(mut inner) = state.inner.lock() else {
                return;
            };
            inner.inspecting = false;
            if inner.closing {
                return;
            }
            inner.closing = true;
            inner.transition = inner.transition.wrapping_add(1);
            inner.transition
        };
        let _ = window.emit("jack-visibility", false);
        // Keep the WebView visible through Bald's 150ms close. A new open
        // invalidates this generation, so rapid reopen cannot hide the new popup.
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(150));
            let callback_app = app.clone();
            let _ = app.run_on_main_thread(move || {
                let should_hide = callback_app
                    .state::<PopupState>()
                    .inner
                    .lock()
                    .is_ok_and(|inner| inner.closing && inner.transition == transition);
                if should_hide && let Some(window) = callback_app.get_webview_window("quick") {
                    // Reset page content only after the native surface is hidden,
                    // never on a competing frontend 150ms timer during its fade.
                    if window.hide().is_ok() {
                        let _ = window.emit("jack-popup-hidden", ());
                    }
                    crate::idle_webview::suspend_later(&window);
                }
            });
        });
    }
}
pub fn is_closing(app: &tauri::AppHandle) -> bool {
    app.state::<PopupState>()
        .inner
        .lock()
        .is_ok_and(|inner| inner.closing)
}
pub fn inspect(app: &tauri::AppHandle) {
    if let Ok(mut inner) = app.state::<PopupState>().inner.lock() {
        inner.inspecting = true;
    }
}
pub fn lost_focus(app: &tauri::AppHandle) {
    // Programmatic hide during app promotion is not a new click-to-dismiss.
    // Do not suppress the user's next immediate single click at the tray.
    if is_closing(app) {
        return;
    }
    if app
        .state::<PopupState>()
        .inner
        .lock()
        .is_ok_and(|inner| inner.inspecting)
    {
        return;
    }
    let mut point = POINT::default();
    let pointer = unsafe { GetCursorPos(&mut point) }.is_ok().then_some(point);
    if let Ok(mut inner) = app.state::<PopupState>().inner.lock() {
        inner.dismissed_at_tray = pointer
            .filter(|p| {
                inner
                    .anchor
                    .is_some_and(|a| a.rect.contains(p.x as f64, p.y as f64))
            })
            .map(|_| Instant::now());
    }
    hide(app);
}
pub fn right_click(app: &tauri::AppHandle) {
    if let Ok(mut inner) = app.state::<PopupState>().inner.lock() {
        inner.dismissed_at_tray = None;
    }
    hide(app);
}
pub fn toggle(
    app: &tauri::AppHandle,
    point: PhysicalPosition<f64>,
    rect: tauri::Rect,
) -> Result<(), String> {
    let monitor = app
        .monitor_from_point(point.x, point.y)
        .map_err(|e| e.to_string())?
        .ok_or("tray monitor unavailable")?;
    let position = rect.position.to_physical::<f64>(monitor.scale_factor());
    let size = rect.size.to_physical::<f64>(monitor.scale_factor());
    let anchor = Anchor {
        rect: Bounds {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        },
        x: point.x,
        y: point.y,
    };
    let state = app.state::<PopupState>();
    let (suppressed, height) = {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        let suppressed = inner
            .dismissed_at_tray
            .take()
            .is_some_and(|at| at.elapsed() < Duration::from_millis(600));
        inner.anchor = Some(anchor);
        (
            suppressed,
            if inner.height > 0.0 {
                inner.height
            } else {
                428.0
            },
        )
    };
    let window = app
        .get_webview_window("quick")
        .ok_or("quick window unavailable")?;
    if suppressed || (window.is_visible().map_err(|e| e.to_string())? && !is_closing(app)) {
        hide(app);
        return Ok(());
    }
    resize(app, height)?;
    {
        let mut inner = state.inner.lock().map_err(|e| e.to_string())?;
        inner.closing = false;
        inner.transition = inner.transition.wrapping_add(1);
    }
    crate::idle_webview::resume(&window);
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    window
        .emit("jack-visibility", true)
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn preview(app: &tauri::AppHandle) -> Result<(), String> {
    let monitor = app
        .primary_monitor()
        .map_err(|e| e.to_string())?
        .ok_or("monitor unavailable")?;
    let area = work(&monitor);
    // Work area can equal the entire screen on an auto-hidden taskbar. Its
    // bottom boundary is exclusive: keep the synthetic preview point inside.
    let point = PhysicalPosition::new(area.x + area.width - 24.0, area.y + area.height - 1.0);
    toggle(
        app,
        point,
        tauri::Rect {
            position: PhysicalPosition::new(point.x - 12.0, point.y).into(),
            size: PhysicalSize::new(24.0, 24.0).into(),
        },
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_edges_and_scale_factors_stay_in_work_area() {
        for work in [
            Bounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1040.0,
            },
            Bounds {
                x: -2560.0,
                y: -300.0,
                width: 2560.0,
                height: 1380.0,
            },
        ] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for (x, y) in [
                    (work.x - 24.0, work.y + 200.0),
                    (work.x + work.width, work.y + 200.0),
                    (work.x + 200.0, work.y - 24.0),
                    (work.x + 200.0, work.y + work.height),
                ] {
                    let b = placement(
                        work,
                        Bounds {
                            x,
                            y,
                            width: 24.0,
                            height: 24.0,
                        },
                        scale,
                        685.0,
                    )
                    .unwrap();
                    assert!(
                        b.x >= work.x
                            && b.y >= work.y
                            && b.x + b.width <= work.x + work.width
                            && b.y + b.height <= work.y + work.height
                    );
                }
            }
        }
    }
    #[test]
    fn tiny_work_area_clamps_both_dimensions() {
        let work = Bounds {
            x: 0.0,
            y: 0.0,
            width: 300.0,
            height: 200.0,
        };
        let b = placement(
            work,
            Bounds {
                x: 280.0,
                y: 200.0,
                width: 20.0,
                height: 20.0,
            },
            2.0,
            685.0,
        )
        .unwrap();
        assert_eq!((b.x, b.y, b.width, b.height), (0.0, 0.0, 300.0, 200.0));
    }
    #[test]
    fn nonfinite_and_empty_geometry_rejected() {
        let b = Bounds {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1040.0,
        };
        for height in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
            assert!(placement(b, b, 1.0, height).is_err());
        }
        assert!(placement(b, b, 0.0, 428.0).is_err());
    }
}
