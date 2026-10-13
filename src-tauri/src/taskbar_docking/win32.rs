//! Direct Win32 calls on app-owned windows. None of them round-trips through
//! the UI thread, so they also work while a move or size loop owns it.

use std::{
    ffi::c_void,
    io, thread,
    time::{Duration, Instant},
};

use tauri::WebviewWindow;

use crate::taskbar_docking_model::{self as model, Rect};

type Hwnd = *mut c_void;

#[repr(C)]
#[derive(Default)]
struct Win32Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct Win32Point {
    x: i32,
    y: i32,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowRect(hwnd: Hwnd, rect: *mut Win32Rect) -> i32;
    fn GetClientRect(hwnd: Hwnd, rect: *mut Win32Rect) -> i32;
    fn ClientToScreen(hwnd: Hwnd, point: *mut Win32Point) -> i32;
    fn GetDpiForWindow(hwnd: Hwnd) -> u32;
    fn IsZoomed(hwnd: Hwnd) -> i32;
    fn IsIconic(hwnd: Hwnd) -> i32;
    fn IsWindowVisible(hwnd: Hwnd) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetSystemMetrics(index: i32) -> i32;
    fn ShowWindow(hwnd: Hwnd, command: i32) -> i32;
    fn SetWindowPos(
        hwnd: Hwnd,
        insert_after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
}

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmFlush() -> i32;
}

const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;
const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;

// SAFETY (whole module): every handle comes from an app-owned WebviewWindow,
// and out-parameters point to writable structs with the Win32 layout.

fn handle(hwnd: u64) -> Hwnd {
    hwnd as usize as Hwnd
}

pub fn hwnd(window: &WebviewWindow) -> Result<u64, String> {
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as usize as u64;
    if hwnd == 0 {
        return Err(format!("window {} has no handle", window.label()));
    }
    Ok(hwnd)
}

fn rect_from(rect: &Win32Rect) -> Option<Rect> {
    let rect = Rect {
        x: rect.left,
        y: rect.top,
        width: rect.right.checked_sub(rect.left)?,
        height: rect.bottom.checked_sub(rect.top)?,
    };
    rect.is_valid().then_some(rect)
}

/// The outer rectangle in screen pixels, also for a taskbar child.
pub fn window_rect(hwnd: u64) -> Option<Rect> {
    let mut rect = Win32Rect::default();
    (unsafe { GetWindowRect(handle(hwnd), &mut rect) } != 0).then_some(())?;
    rect_from(&rect)
}

/// The visible client area in screen pixels; the outer rectangle of the
/// main window also contains invisible resize borders.
pub fn client_rect(hwnd: u64) -> Option<Rect> {
    let mut client = Win32Rect::default();
    let mut origin = Win32Point::default();
    let read = unsafe {
        GetClientRect(handle(hwnd), &mut client) != 0
            && ClientToScreen(handle(hwnd), &mut origin) != 0
    };
    read.then_some(())?;
    let size = rect_from(&client)?;
    Some(Rect {
        x: origin.x,
        y: origin.y,
        ..size
    })
}

/// A normal window whose logical client size is the compact bar.
pub fn is_floating_compact(hwnd: u64) -> bool {
    let (zoomed, iconic, dpi) = unsafe {
        (
            IsZoomed(handle(hwnd)),
            IsIconic(handle(hwnd)),
            GetDpiForWindow(handle(hwnd)),
        )
    };
    if zoomed != 0 || iconic != 0 || !(48..=768).contains(&dpi) {
        return false;
    }
    client_rect(hwnd).is_some_and(|client| {
        let logical = |pixels: i32| (f64::from(pixels) * 96.0 / f64::from(dpi)).round() as u32;
        model::is_compact_size(logical(client.width), logical(client.height))
    })
}

pub fn is_visible(hwnd: u64) -> bool {
    unsafe { IsWindowVisible(handle(hwnd)) != 0 }
}

/// Hides without activation or frame changes. The overlay is driven only
/// through Win32: Tao's queued show/hide reapplies its cached top-level
/// style, which would paint a caption onto a taskbar child.
pub fn hide(hwnd: u64) {
    unsafe { ShowWindow(handle(hwnd), SW_HIDE) };
}

/// Hides the player, keeping Tao's cached visibility in sync.
pub fn hide_player(window: &WebviewWindow) {
    let _ = window.hide();
    if let Ok(hwnd) = hwnd(window) {
        hide(hwnd);
    }
}

/// Shows the player without stealing focus from the foreground app.
pub fn show_player(window: &WebviewWindow) {
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    let _ = window.show();
    if let Ok(hwnd) = hwnd(window) {
        unsafe { ShowWindow(handle(hwnd), SW_SHOWNOACTIVATE) };
    }
}

/// Raises and shows a detached overlay above every window, unactivated.
pub fn show_topmost(hwnd: u64) {
    const HWND_TOPMOST: isize = -1;
    unsafe {
        SetWindowPos(
            handle(hwnd),
            HWND_TOPMOST as Hwnd,
            0,
            0,
            0,
            0,
            SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        )
    };
}

pub fn set_rect(hwnd: u64, rect: Rect) -> Result<(), String> {
    let moved = unsafe {
        SetWindowPos(
            handle(hwnd),
            std::ptr::null_mut(),
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if moved == 0 {
        return Err(format!(
            "could not move the taskbar overlay: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

/// GetAsyncKeyState reports physical buttons, so honor swapped buttons.
pub fn primary_button_pressed() -> bool {
    const SM_SWAPBUTTON: i32 = 23;
    const VK_LBUTTON: i32 = 0x01;
    const VK_RBUTTON: i32 = 0x02;
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    let key = if swapped { VK_RBUTTON } else { VK_LBUTTON };
    (unsafe { GetAsyncKeyState(key) }) < 0
}

/// Paces flight frames with DWM composition, falling back to a short sleep
/// when composition does not block.
pub fn wait_for_next_frame() {
    let started = Instant::now();
    let flushed = unsafe { DwmFlush() } >= 0;
    if !flushed || started.elapsed() < Duration::from_millis(2) {
        thread::sleep(Duration::from_millis(6));
    }
}

/// Resizes the player so its client area, not its outer frame, has the given
/// logical size; the undecorated, shadowed window has a non-client extent.
pub fn set_player_client_size(
    window: &WebviewWindow,
    logical_width: f64,
    logical_height: f64,
) -> Result<(), String> {
    let hwnd = hwnd(window)?;
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    let physical = |logical: f64| (logical * scale).round() as i32;
    let (width, height) = (physical(logical_width), physical(logical_height));
    // Measuring outer_size first completes queued fullscreen/maximize changes.
    let outer = window.outer_size().map_err(|error| error.to_string())?;
    let client = client_rect(hwnd).ok_or("player client geometry is unavailable")?;
    if (client.width, client.height) == (width, height) {
        return Ok(());
    }
    let outer_width = i32::try_from(outer.width).map_err(|error| error.to_string())?;
    let outer_height = i32::try_from(outer.height).map_err(|error| error.to_string())?;
    let resized = unsafe {
        SetWindowPos(
            handle(hwnd),
            std::ptr::null_mut(),
            0,
            0,
            outer_width + width - client.width,
            outer_height + height - client.height,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
    if resized == 0 {
        return Err(format!(
            "could not resize the player: {}",
            io::Error::last_os_error()
        ));
    }
    match client_rect(hwnd) {
        Some(client) if (client.width, client.height) == (width, height) => Ok(()),
        _ => Err("player client size did not match the restore size".to_owned()),
    }
}
