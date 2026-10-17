//! Small Win32 helpers shared by window placement and taskbar docking.

use std::ffi::c_void;

use tauri::WebviewWindow;
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, RECT, RPC_E_CHANGED_MODE},
        System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED},
        UI::WindowsAndMessaging::{
            FindWindowExW, FindWindowW, GetWindowRect, IsWindowVisible, SetWindowPos,
            SWP_NOACTIVATE, SWP_NOZORDER,
        },
    },
};

use crate::taskbar_docking::model::Rect;

pub fn hwnd(raw: usize) -> HWND {
    HWND(raw as *mut c_void)
}

/// The window handle as a plain integer, so it can cross threads.
pub fn hwnd_of(window: &WebviewWindow) -> Result<usize, String> {
    let handle = window.hwnd().map_err(|error| error.to_string())?.0 as usize;
    if handle == 0 {
        return Err(format!("window {} has no handle", window.label()));
    }
    Ok(handle)
}

/// Moves and resizes in one call so the window never shows an intermediate frame.
pub fn set_rect(window: HWND, rect: Rect) -> Result<(), String> {
    unsafe {
        SetWindowPos(
            window,
            None,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
    .map_err(|error| format!("could not place window: {error}"))
}

pub fn place_window(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    set_rect(hwnd(hwnd_of(window)?), Rect::new(x, y, width, height))
}

pub fn window_rect(window: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(window, &mut rect) }.ok()?;
    rect_from(rect)
}

/// `None` for empty or inverted rectangles.
pub fn rect_from(rect: RECT) -> Option<Rect> {
    let width = rect.right.checked_sub(rect.left)?;
    let height = rect.bottom.checked_sub(rect.top)?;
    (width > 0 && height > 0).then(|| Rect::new(rect.left, rect.top, width, height))
}

/// Every visible taskbar window, primary first.
pub fn taskbar_windows() -> Vec<HWND> {
    let mut taskbars = Vec::new();
    if let Ok(primary) = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) } {
        if !primary.0.is_null() && unsafe { IsWindowVisible(primary) }.as_bool() {
            taskbars.push(primary);
        }
    }
    let mut cursor = HWND::default();
    while let Ok(next) =
        unsafe { FindWindowExW(None, Some(cursor), w!("Shell_SecondaryTrayWnd"), None) }
    {
        if next.0.is_null() {
            break;
        }
        if unsafe { IsWindowVisible(next) }.as_bool() {
            taskbars.push(next);
        }
        cursor = next;
    }
    taskbars
}

/// Joins the multithreaded apartment for the current thread.
pub struct Apartment {
    owns: bool,
}

impl Apartment {
    pub fn enter() -> Option<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() {
            Some(Self { owns: true })
        } else if result == RPC_E_CHANGED_MODE {
            Some(Self { owns: false })
        } else {
            None
        }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.owns {
            unsafe { CoUninitialize() };
        }
    }
}
