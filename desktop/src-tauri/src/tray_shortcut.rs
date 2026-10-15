//! A Windows global hotkey owned by Jack's main HWND, even while it is hidden.
use jack_core::TrayShortcut;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering},
    mpsc,
};
use tauri::{Emitter, Manager};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, HOT_KEY_MODIFIERS, RegisterHotKey, UnregisterHotKey,
        },
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            CallNextHookEx, GetCursorPos, HHOOK, KBDLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW,
            UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_APP, WM_HOTKEY, WM_KEYDOWN, WM_KEYUP,
            WM_NCDESTROY, WM_SYSKEYDOWN, WM_SYSKEYUP,
        },
    },
};
const SUBCLASS: usize = 0x4a53484b;
const HOOK_TRIGGER: u32 = WM_APP + 0x351;
const RECORD_PRINT_SCREEN: u32 = WM_APP + 0x352;
static HOOK: AtomicIsize = AtomicIsize::new(0);
static TARGET: AtomicIsize = AtomicIsize::new(0);
static BINDING: AtomicU32 = AtomicU32::new(0);
static RECORDING: AtomicBool = AtomicBool::new(false);
static BLOCKED: AtomicU32 = AtomicU32::new(0);

fn modifiers() -> u32 {
    let held = |key| unsafe { GetAsyncKeyState(key) < 0 };
    (if held(0x12) { 1 } else { 0 })
        | (if held(0x11) { 2 } else { 0 })
        | (if held(0x10) { 4 } else { 0 })
        | (if held(0x5b) || held(0x5c) { 8 } else { 0 })
}
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let key = event.vkCode;
        let message = wparam.0 as u32;
        let up = message == WM_KEYUP || message == WM_SYSKEYUP;
        let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
        let was_blocked = BLOCKED.load(Ordering::Relaxed) == key;
        if up && was_blocked {
            BLOCKED.store(0, Ordering::Relaxed);
            return LRESULT(1);
        }
        let mods = modifiers();
        let recording = RECORDING.load(Ordering::Relaxed) && key == 0x2c;
        let binding = BINDING.load(Ordering::Relaxed);
        let selected = binding != 0 && binding == (key | (mods << 16));
        // Snapshot sometimes produces only key-up. Capture it natively before
        // Windows' snipping handler; do no I/O or UI work on the hook's clock.
        if (recording || selected) && (down || (up && key == 0x2c)) {
            if !was_blocked {
                if down {
                    BLOCKED.store(key, Ordering::Relaxed);
                }
                let hwnd = HWND(TARGET.load(Ordering::Relaxed) as *mut _);
                let _ = unsafe {
                    PostMessageW(
                        Some(hwnd),
                        if recording {
                            RECORD_PRINT_SCREEN
                        } else {
                            HOOK_TRIGGER
                        },
                        WPARAM(mods as usize),
                        LPARAM(binding as isize),
                    )
                };
            }
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
fn hook_mode(needed: bool, binding: u32, recording: bool) -> Result<(), String> {
    if needed && HOOK.load(Ordering::Relaxed) == 0 {
        let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
        let hook = unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(module.into()), 0)
        }
        .map_err(|e| e.to_string())?;
        HOOK.store(hook.0 as isize, Ordering::Relaxed);
    }
    BINDING.store(binding, Ordering::Relaxed);
    RECORDING.store(recording, Ordering::Relaxed);
    if !needed {
        let hook = HOOK.swap(0, Ordering::Relaxed);
        if hook != 0 {
            let _ = unsafe { UnhookWindowsHookEx(HHOOK(hook as *mut _)) };
        }
        BLOCKED.store(0, Ordering::Relaxed);
    }
    Ok(())
}
#[derive(Default)]
struct Inner {
    config: TrayShortcut,
    id: Option<i32>,
    paused: bool,
}
#[derive(Default)]
pub struct State(Mutex<Inner>);

fn virtual_key(code: &str) -> Option<u32> {
    if let Some(letter) = code.strip_prefix("Key").filter(|s| s.len() == 1) {
        let key = letter.as_bytes()[0];
        if key.is_ascii_uppercase() {
            return Some(key as u32);
        }
    }
    if let Some(digit) = code.strip_prefix("Digit").filter(|s| s.len() == 1) {
        let key = digit.as_bytes()[0];
        if key.is_ascii_digit() {
            return Some(key as u32);
        }
    }
    if let Some(number) = code
        .strip_prefix('F')
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| (1..=24).contains(n))
    {
        return Some(0x6f + number);
    }
    Some(match code {
        "Space" => 0x20,
        "Enter" => 0x0d,
        "Tab" => 0x09,
        "Backspace" => 0x08,
        "PrintScreen" => 0x2c,
        "ScrollLock" => 0x91,
        "Pause" => 0x13,
        "CapsLock" => 0x14,
        "NumLock" => 0x90,
        "Numpad0" => 0x60,
        "Numpad1" => 0x61,
        "Numpad2" => 0x62,
        "Numpad3" => 0x63,
        "Numpad4" => 0x64,
        "Numpad5" => 0x65,
        "Numpad6" => 0x66,
        "Numpad7" => 0x67,
        "Numpad8" => 0x68,
        "Numpad9" => 0x69,
        "NumpadMultiply" => 0x6a,
        "NumpadAdd" => 0x6b,
        "NumpadSubtract" => 0x6d,
        "NumpadDecimal" => 0x6e,
        "NumpadDivide" => 0x6f,
        "NumpadEnter" => 0x0d,
        "AudioVolumeMute" => 0xad,
        "AudioVolumeDown" => 0xae,
        "AudioVolumeUp" => 0xaf,
        "MediaTrackNext" => 0xb0,
        "MediaTrackPrevious" => 0xb1,
        "MediaStop" => 0xb2,
        "MediaPlayPause" => 0xb3,
        "ContextMenu" => 0x5d,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Insert" => 0x2d,
        "Delete" => 0x2e,
        "Minus" => 0xbd,
        "Equal" => 0xbb,
        "BracketLeft" => 0xdb,
        "BracketRight" => 0xdd,
        "Backslash" => 0xdc,
        "Semicolon" => 0xba,
        "Quote" => 0xde,
        "Comma" => 0xbc,
        "Period" => 0xbe,
        "Slash" => 0xbf,
        "Backquote" => 0xc0,
        _ => return None,
    })
}

pub fn current(app: &tauri::AppHandle) -> TrayShortcut {
    app.state::<State>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .config
        .clone()
}
fn apply(app: &tauri::AppHandle, config: TrayShortcut, paused: bool) -> Result<(), String> {
    if config.modifiers & !15 != 0 {
        return Err("지원하지 않는 보조 키입니다.".into());
    }
    let vk = config
        .key
        .as_deref()
        .map(|code| virtual_key(code).ok_or("지원하지 않는 키입니다."))
        .transpose()?;
    let window = app
        .get_webview_window("main")
        .ok_or("main window unavailable")?;
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    let state = app.state::<State>();
    let mut inner = state.0.lock().map_err(|e| e.to_string())?;
    let active = config.enabled && vk.is_some() && !paused;
    if active && inner.id.is_some() && inner.config == config && inner.paused == paused {
        return Ok(());
    }
    let id = if inner.id == Some(1001) { 1002 } else { 1001 };
    let mut registered = false;
    if active && vk != Some(0x2c) {
        // Register the replacement first: a conflict must not discard the old key.
        registered = unsafe {
            RegisterHotKey(
                Some(hwnd),
                id,
                HOT_KEY_MODIFIERS(config.modifiers | 0x4000),
                vk.unwrap(),
            )
        }
        .is_ok();
    }
    // A selected key may override its existing action (including Print Screen).
    // Only use the low-level hook for capture or when normal registration cannot.
    let intercepted = active && !registered;
    hook_mode(
        paused || intercepted,
        if intercepted {
            vk.unwrap() | (config.modifiers << 16)
        } else {
            0
        },
        paused,
    )?;
    if let Some(previous) = inner.id {
        let _ = unsafe { UnregisterHotKey(Some(hwnd), previous) };
    }
    inner.id = registered.then_some(id);
    inner.config = config;
    inner.paused = paused;
    Ok(())
}
pub async fn configure(app: &tauri::AppHandle, config: TrayShortcut) -> Result<(), String> {
    let (done, result) = mpsc::sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = done.send(apply(&handle, config, false));
    })
    .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || result.recv().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn shortcut_recording(app: tauri::AppHandle, recording: bool) -> Result<(), String> {
    let (done, result) = mpsc::sync_channel(1);
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let config = current(&handle);
        let _ = done.send(apply(&handle, config, recording));
    })
    .map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || result.recv().map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?
}
pub fn install(app: &tauri::AppHandle, config: TrayShortcut) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("main window unavailable")?;
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
    let data = Box::into_raw(Box::new(app.clone()));
    if !unsafe { SetWindowSubclass(hwnd, Some(callback), SUBCLASS, data as usize) }.as_bool() {
        unsafe {
            drop(Box::from_raw(data));
        }
        return Err("install shortcut handler failed".into());
    }
    apply(app, config, false)
}
unsafe extern "system" fn callback(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let app = unsafe { &*(data as *const tauri::AppHandle) };
    if message == RECORD_PRINT_SCREEN {
        if RECORDING.load(Ordering::Relaxed)
            && let Some(window) = app.get_webview_window("main")
        {
            let _ = window.emit(
                "jack-shortcut-key",
                serde_json::json!({"enabled":true,"key":"PrintScreen","modifiers":wparam.0 as u32}),
            );
        }
        return LRESULT(0);
    }
    if message == WM_HOTKEY || message == HOOK_TRIGGER {
        let owned = app.state::<State>().0.lock().is_ok_and(|inner| {
            !inner.paused
                && if message == HOOK_TRIGGER {
                    BINDING.load(Ordering::Relaxed) != 0
                        && BINDING.load(Ordering::Relaxed) == lparam.0 as u32
                } else {
                    inner.id == Some(wparam.0 as i32)
                }
        });
        if owned {
            let mut cursor = POINT::default();
            if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
                crate::tray_menu::hide(app);
                let point = tauri::PhysicalPosition::new(cursor.x as f64, cursor.y as f64);
                let rect = tauri::Rect {
                    position: tauri::PhysicalPosition::new(point.x - 10.0, point.y - 10.0).into(),
                    size: tauri::PhysicalSize::new(20.0, 20.0).into(),
                };
                if let Err(error) = crate::popup::toggle(app, point, rect) {
                    let _ = app.emit("jack-error", error);
                }
            }
            return LRESULT(0);
        }
    }
    if message == WM_NCDESTROY {
        let _ = hook_mode(false, 0, false);
        let _ = unsafe { RemoveWindowSubclass(hwnd, Some(callback), SUBCLASS) };
        unsafe {
            drop(Box::from_raw(data as *mut tauri::AppHandle));
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}
