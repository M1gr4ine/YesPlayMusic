//! Windows taskbar docking for the compact player bar.
//!
//! Dragging the bar onto the taskbar and releasing it hides the player and
//! shows a 380-DIP lyrics strip over the empty part of the taskbar. The
//! strip is an ordinary top-most overlay window: Explorer does not make room
//! for foreign children, so parenting into it would only cover buttons.
//!
//! - `model`: geometry and the state machine, tested on every platform.
//! - `probe`: measures the taskbars with UI Automation, failing closed.
//! - `attach`: strip window placement plus the hooks that report drags,
//!   foreground changes and taskbar changes.
//! - `controller`: owns the state and runs effects on one worker thread.

#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
pub mod model;

#[cfg(windows)]
mod attach;
#[cfg(windows)]
mod controller;
#[cfg(windows)]
mod probe;

#[cfg(windows)]
pub use controller::Controller;

/// Label of the strip window; `tauri_plugin_window_state` must never restore it.
pub const OVERLAY_LABEL: &str = "taskbar-strip";
