//! Non-client handling for Jack's own UI only. Never attach to another process.
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::Threading::{GetCurrentProcessId, GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, VK_LBUTTON},
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GetCursorPos, GetWindowThreadProcessId, HTCAPTION, MA_ACTIVATE, SWP_FRAMECHANGED,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
            SetForegroundWindow, SetWindowPos, WM_MOUSEACTIVATE, WM_NCCALCSIZE, WM_NCDESTROY,
            WM_NCLBUTTONDOWN,
        },
    },
};

const SUBCLASS_ID: usize = 0x4a41434b;

/// Enter the OS move loop once on Jack's UI thread. Completion means the user
/// released the drag, so the frontend can safely resume replacing its header.
pub fn drag(hwnd: HWND) -> Result<(), String> {
    unsafe {
        let mut process_id = 0;
        let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if process_id != GetCurrentProcessId() || thread_id != GetCurrentThreadId() {
            return Err("Jack drag must run on its own window thread".into());
        }
        // An IPC queued after a quick click must not start a phantom move loop.
        if GetAsyncKeyState(VK_LBUTTON.0 as i32) >= 0 {
            return Ok(());
        }
        let _ = SetForegroundWindow(hwnd);
        let mut point = POINT::default();
        GetCursorPos(&mut point).map_err(|e| e.to_string())?;
        // No capture is also a valid state. A failed ReleaseCapture alone must
        // not discard a first-click drag from a WebView child window.
        let _ = ReleaseCapture();
        let coordinates = ((point.y as u16 as u32) << 16) | point.x as u16 as u32;
        SendMessageW(
            hwnd,
            WM_NCLBUTTONDOWN,
            Some(WPARAM(HTCAPTION as usize)),
            Some(LPARAM(coordinates as isize)),
        );
    }
    Ok(())
}

pub fn install(hwnd: HWND) -> Result<(), String> {
    unsafe {
        let mut process_id = 0;
        let thread_id = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if process_id != GetCurrentProcessId() || thread_id != GetCurrentThreadId() {
            return Err("Jack frame handler must run on its own window thread".into());
        }
        if !SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, 0).as_bool() {
            return Err("install Jack frame handler failed".into());
        }
        SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // Tao already handles TRUE. FALSE must also leave the proposed rectangle
    // untouched; DefWindowProc otherwise calculates the retained WS_CAPTION.
    if message == WM_NCCALCSIZE {
        return LRESULT(0);
    }
    if message == WM_MOUSEACTIVATE {
        // Activate AND deliver the same initial press to the WebView.
        return LRESULT(MA_ACTIVATE as isize);
    }
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}
