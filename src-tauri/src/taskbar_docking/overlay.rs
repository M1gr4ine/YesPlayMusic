//! Native frame policy for the transparent taskbar strip.

use crate::win32::hwnd;
use tauri::WebviewWindow;
use windows::{
    core::Result as WinResult,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, STYLESTRUCT,
                SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
                WM_NCACTIVATE, WM_NCDESTROY, WM_NCPAINT, WM_STYLECHANGING, WS_CAPTION,
            },
        },
    },
};

pub fn configure_overlay(window: &WebviewWindow) -> tauri::Result<()> {
    let raw = window.hwnd()?.0 as usize;
    window.with_webview(move |_| {
        // Subclass helpers must run on the window's UI thread.
        if let Err(error) = configure_frame(hwnd(raw)) {
            eprintln!("[taskbar-docking] could not remove the strip native frame: {error}");
        }
    })
}

fn configure_frame(window: HWND) -> WinResult<()> {
    unsafe { SetWindowSubclass(window, Some(frame_proc), 0, 0) }.ok()?;
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
    if unsafe { SetWindowLongPtrW(window, GWL_STYLE, style & !(WS_CAPTION.0 as isize)) } == 0 {
        return Err(windows::core::Error::from_win32());
    }
    unsafe {
        SetWindowPos(
            window,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
}

unsafe extern "system" fn frame_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    // Windows can repaint a cached frame even after the caption style is cleared.
    if message == WM_NCPAINT {
        return LRESULT(0);
    }
    // Tao retains caption bits on borderless windows and can write them back.
    // The strip has no native frame, including during activation or theme changes.
    if message == WM_STYLECHANGING && wparam.0 as isize == GWL_STYLE.0 as isize && lparam.0 != 0 {
        let styles = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
        styles.styleNew &= !WS_CAPTION.0;
    }
    if message == WM_NCDESTROY {
        let _ = unsafe { RemoveWindowSubclass(window, Some(frame_proc), id) };
    }
    // Keep activation handling without repainting the cached frame.
    let lparam = if message == WM_NCACTIVATE {
        LPARAM(-1)
    } else {
        lparam
    };
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}
