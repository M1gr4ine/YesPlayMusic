use super::taskbar_docking_model::{self as model, DockMode, DragDockHint, ForegroundSafety, Rect};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{path::BaseDirectory, AppHandle, Emitter, Manager, WebviewWindow};

const EVENT: &str = "desktop://taskbarDockState";
const OVERLAY_SNAPSHOT_EVENT: &str = "desktop://taskbarOverlaySnapshot";
const OVERLAY_VISUAL_EVENT: &str = "desktop://taskbarOverlayVisual";
const OVERLAY_ACTION_EVENT: &str = "desktop://taskbarOverlayAction";
const HINT_EVENT: &str = "desktop://taskbarDockHint";
const OVERLAY_LABEL: &str = "taskbar-lyrics";
const PREF_FILE: &str = "taskbar-docking.json";
const PREF_MAX_BYTES: u64 = 4_096;
const HELPER_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const DRAG_SAMPLE_INTERVAL: Duration = Duration::from_millis(16);
const DRAG_PROBE_INTERVAL: Duration = Duration::from_millis(600);
const VISUAL_READY_TIMEOUT: Duration = Duration::from_secs(2);
const FULL_MOTION_DURATION: Duration = Duration::from_millis(240);
const REDUCED_MOTION_DURATION: Duration = Duration::from_millis(120);

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Win32Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Win32Point {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    fn GetForegroundWindow() -> *mut std::ffi::c_void;
    fn GetWindowRect(hwnd: *mut std::ffi::c_void, rect: *mut Win32Rect) -> i32;
    fn GetClientRect(hwnd: *mut std::ffi::c_void, rect: *mut Win32Rect) -> i32;
    fn ClientToScreen(hwnd: *mut std::ffi::c_void, point: *mut Win32Point) -> i32;
    fn GetDpiForWindow(hwnd: *mut std::ffi::c_void) -> u32;
    fn IsZoomed(hwnd: *mut std::ffi::c_void) -> i32;
    fn IsIconic(hwnd: *mut std::ffi::c_void) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn GetSystemMetrics(index: i32) -> i32;
    fn ShowWindow(hwnd: *mut std::ffi::c_void, command: i32) -> i32;
    fn SetWindowPos(
        hwnd: *mut std::ffi::c_void,
        insert_after: *mut std::ffi::c_void,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmFlush() -> i32;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskbarDockStatus {
    mode: DockModePayload,
    suspended: bool,
    dock_error: String,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum DockModePayload {
    Floating,
    Docking,
    Docked,
    Undocking,
    Waiting,
}

impl From<DockMode> for DockModePayload {
    fn from(value: DockMode) -> Self {
        match value {
            DockMode::Floating => Self::Floating,
            DockMode::Docking => Self::Docking,
            DockMode::Docked => Self::Docked,
            DockMode::Undocking => Self::Undocking,
            DockMode::Waiting => Self::Waiting,
        }
    }
}

#[derive(Clone)]
pub struct TaskbarDockController {
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
    poll_started: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    requested_visible: Arc<AtomicBool>,
    compact_docking_active: Arc<AtomicBool>,
    exit_in_progress: Arc<AtomicBool>,
    exit_prepared: Arc<AtomicBool>,
    overlay_ready: Arc<AtomicBool>,
    overlay_reset_complete: Arc<AtomicBool>,
    visual_ready_token: Arc<AtomicU64>,
    current_visual_token: Arc<AtomicU64>,
    main_hwnd: Arc<AtomicU64>,
    drag_active: Arc<AtomicBool>,
}

struct Inner {
    enabled: bool,
    compact: bool,
    requested_visible: bool,
    transitioning: bool,
    attached: bool,
    attached_bar: Option<String>,
    recovery_pending: bool,
    hwnd: Option<u64>,
    recovery_frame: Option<RecoveryFrame>,
    recovery_resizable: Option<bool>,
    mode: DockMode,
    suspended: bool,
    dock_error: String,
    published_intermediate_status: bool,
    overlay_snapshot: Option<Value>,
    visual_phase: &'static str,
    visual_token: u64,
    native: NativeHelper,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DockingPreference {
    version: u8,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct NativeRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayVisual {
    phase: &'static str,
    token: u64,
    from: Option<NativeRect>,
    to: Option<NativeRect>,
    duration_ms: u32,
}

struct OverlayFlight {
    from: NativeRect,
    to: NativeRect,
    phase: &'static str,
}

impl From<NativeRect> for Rect {
    fn from(value: NativeRect) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

impl From<Rect> for NativeRect {
    fn from(value: Rect) -> Self {
        Self {
            x: value.x,
            y: value.y,
            width: value.width,
            height: value.height,
        }
    }
}

#[derive(Deserialize)]
struct ProbeResult {
    complete: bool,
    bars: Vec<TaskbarObservation>,
}

struct DragProbe {
    complete: bool,
    bars: Vec<TaskbarObservation>,
    overlay_ready: bool,
}

impl DragProbe {
    fn hint(&self, bounds: NativeRect) -> DragDockHint {
        let Some(bar) = bar_for_window(&self.bars, bounds) else {
            return DragDockHint::None;
        };
        let safe = self.complete && !bar.suspended && bar.fullscreen == Some(false);
        let hint = model::drag_dock_hint(
            bounds.into(),
            bar.taskbar.into(),
            bar.free.map(Into::into),
            bar.dpi,
            safe,
        );
        // Without a rendered strip a release could only fall back; say nothing.
        if hint == DragDockHint::Ready && !self.overlay_ready {
            DragDockHint::None
        } else {
            hint
        }
    }
}

#[derive(Deserialize)]
struct TaskbarObservation {
    handle: String,
    taskbar: NativeRect,
    monitor: NativeRect,
    free: Option<NativeRect>,
    dpi: u32,
    suspended: bool,
    fullscreen: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryFrame {
    hwnd: String,
    parent: String,
    style: String,
    ex_style: String,
    rect: NativeRect,
}

#[derive(Deserialize)]
struct HelperReply {
    id: u64,
    result: Option<Value>,
    error: Option<String>,
    code: Option<i32>,
}

struct HelperLine {
    child: Child,
    stdin: ChildStdin,
    receiver: Receiver<Result<String, String>>,
    hwnd: u64,
    sequence: u64,
}

struct NativeHelper {
    helper_path: Option<PathBuf>,
    session: Option<HelperLine>,
}

struct NativeError {
    code: Option<i32>,
    session_failed: bool,
}

impl TaskbarDockController {
    pub fn new(app: AppHandle) -> Self {
        let enabled = load_preference(&app).unwrap_or_else(|error| {
            eprintln!("[taskbar-docking] ignored invalid saved preference: {error}");
            false
        });
        Self {
            app,
            inner: Arc::new(Mutex::new(Inner {
                enabled,
                compact: false,
                requested_visible: false,
                transitioning: false,
                attached: false,
                attached_bar: None,
                recovery_pending: false,
                hwnd: None,
                recovery_frame: None,
                recovery_resizable: None,
                mode: DockMode::Floating,
                suspended: false,
                dock_error: String::new(),
                published_intermediate_status: false,
                overlay_snapshot: None,
                visual_phase: "hidden",
                visual_token: 0,
                native: NativeHelper {
                    helper_path: None,
                    session: None,
                },
            })),
            poll_started: Arc::new(AtomicBool::new(false)),
            stopping: Arc::new(AtomicBool::new(false)),
            requested_visible: Arc::new(AtomicBool::new(false)),
            compact_docking_active: Arc::new(AtomicBool::new(false)),
            exit_in_progress: Arc::new(AtomicBool::new(false)),
            exit_prepared: Arc::new(AtomicBool::new(false)),
            overlay_ready: Arc::new(AtomicBool::new(false)),
            overlay_reset_complete: Arc::new(AtomicBool::new(true)),
            visual_ready_token: Arc::new(AtomicU64::new(0)),
            current_visual_token: Arc::new(AtomicU64::new(0)),
            main_hwnd: Arc::new(AtomicU64::new(0)),
            drag_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn bind_and_start(&self, main: WebviewWindow, overlay: WebviewWindow) {
        if main.label() != "main" || overlay.label() != OVERLAY_LABEL {
            return;
        }
        // Drag sampling reads the player HWND directly from worker threads.
        if let Ok(hwnd) = native_hwnd(&main) {
            self.main_hwnd.store(hwnd, Ordering::Release);
        }
        if let Ok(mut inner) = self.inner.lock() {
            inner.hwnd = native_hwnd(&overlay).ok();
            inner.requested_visible = main.is_visible().unwrap_or(false);
            self.requested_visible
                .store(inner.requested_visible, Ordering::Release);
        }
        self.emit_current();
        if self
            .poll_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let controller = self.clone();
            thread::spawn(move || loop {
                thread::sleep(POLL_INTERVAL);
                if controller.stopping.load(Ordering::Acquire) {
                    break;
                }
                let Some(main) = controller.app.get_webview_window("main") else {
                    continue;
                };
                let Some(overlay) = controller.app.get_webview_window(OVERLAY_LABEL) else {
                    if controller.compact_docking_active.load(Ordering::Acquire) {
                        hide_main_window(&main);
                    }
                    continue;
                };
                if !controller.exit_in_progress.load(Ordering::Acquire) {
                    controller.tick(&main, &overlay);
                }
            });
        }
    }

    pub fn set_enabled(
        &self,
        window: &WebviewWindow,
        enabled: bool,
    ) -> Result<TaskbarDockStatus, String> {
        require_main(window)?;
        let overlay = self.overlay_window()?;
        let mut inner = self.lock()?;
        if enabled && !window_is_live_compact(window)? {
            return Err("Taskbar docking requires a normal compact window".to_string());
        }
        inner.transitioning = true;
        let result = if enabled {
            match persist_preference(&self.app, true) {
                Ok(()) => {
                    inner.enabled = true;
                    self.compact_docking_active
                        .store(inner.compact, Ordering::Release);
                    self.refresh_locked(&mut inner, window, &overlay)
                }
                Err(error) => Err(error),
            }
        } else {
            match persist_preference(&self.app, false) {
                Ok(()) => {
                    inner.enabled = false;
                    match self.return_to_main_locked(&mut inner, window, &overlay) {
                        Ok(()) => Ok(()),
                        Err(error) => {
                            inner.enabled = true;
                            if let Err(persist_error) = persist_preference(&self.app, true) {
                                eprintln!("[taskbar-docking] failed to restore enabled preference: {persist_error}");
                            }
                            Err(error)
                        }
                    }
                }
                Err(error) => Err(error),
            }
        };
        inner.transitioning = false;
        self.compact_docking_active
            .store(inner.enabled && inner.compact, Ordering::Release);
        if !enabled && result.is_ok() {
            inner.mode = DockMode::Floating;
            inner.suspended = false;
            inner.dock_error.clear();
            self.set_visual_phase(&mut inner, "hidden", None, None);
        }
        let status = status(&inner);
        drop(inner);
        self.emit(status.clone());
        if !enabled && result.is_ok() {
            hide_overlay_window(&overlay);
        }
        if let Err(error) = result {
            eprintln!("[taskbar-docking] could not apply the docking preference: {error}");
            if !enabled {
                return Err(error);
            }
        }
        Ok(status)
    }

    pub fn set_compact(
        &self,
        window: &WebviewWindow,
        compact: bool,
    ) -> Result<TaskbarDockStatus, String> {
        require_main(window)?;
        let overlay = self.overlay_window()?;
        let size_is_valid = if compact {
            window_is_live_compact(window)?
        } else {
            let (width, height) = logical_inner_size(window)?;
            model::compact_mode_is_valid(false, width, height)
        };
        if !size_is_valid {
            return Err("Compact mode does not match the main window's current size".to_string());
        }

        let mut inner = self.lock()?;
        inner.transitioning = true;
        let result = if !compact {
            self.return_to_main_locked(&mut inner, window, &overlay)
                .map(|()| {
                    inner.compact = false;
                    inner.mode = DockMode::Floating;
                    inner.suspended = false;
                    inner.dock_error.clear();
                    self.set_visual_phase(&mut inner, "hidden", None, None);
                })
        } else {
            inner.compact = true;
            self.compact_docking_active
                .store(inner.enabled, Ordering::Release);
            if inner.enabled {
                self.refresh_locked(&mut inner, window, &overlay)
            } else {
                inner.mode = DockMode::Floating;
                inner.suspended = false;
                inner.dock_error.clear();
                Ok(())
            }
        };
        inner.transitioning = false;
        self.compact_docking_active
            .store(inner.enabled && inner.compact, Ordering::Release);
        let current = status(&inner);
        drop(inner);
        self.emit(current.clone());
        if !compact && result.is_ok() {
            hide_overlay_window(&overlay);
        }
        result.map(|()| current)
    }

    fn overlay_window(&self) -> Result<WebviewWindow, String> {
        self.app
            .get_webview_window(OVERLAY_LABEL)
            .ok_or_else(|| "taskbar overlay window is unavailable".to_string())
    }

    pub fn request_show(&self, window: &WebviewWindow) -> Result<bool, String> {
        require_main(window)?;
        self.requested_visible.store(true, Ordering::Release);
        match self.inner.try_lock() {
            Ok(mut inner) => {
                inner.requested_visible = true;
                // The poller performs all native show operations off the UI
                // callback thread. Avoid activating a compact window while
                // the foreground/fullscreen probe is being refreshed.
                Ok(!(inner.enabled && inner.compact))
            }
            Err(_) => Ok(false),
        }
    }

    pub fn is_compact_docking_active(&self) -> bool {
        self.compact_docking_active.load(Ordering::Acquire)
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub fn overlay_reset_complete(&self) -> bool {
        self.overlay_reset_complete.load(Ordering::Acquire)
    }

    pub fn overlay_destroyed(&self) -> bool {
        if self.stopping.load(Ordering::Acquire) {
            return false;
        }
        self.overlay_ready.store(false, Ordering::Release);
        self.overlay_reset_complete.store(false, Ordering::Release);
        let controller = self.clone();
        thread::spawn(move || loop {
            if controller.stopping.load(Ordering::Acquire) {
                return;
            }
            if let Ok(mut inner) = controller.inner.try_lock() {
                inner.native.close();
                inner.hwnd = None;
                inner.attached = false;
                inner.attached_bar = None;
                inner.recovery_pending = false;
                inner.recovery_frame = None;
                inner.recovery_resizable = None;
                inner.mode = if inner.enabled && inner.compact {
                    DockMode::Waiting
                } else {
                    DockMode::Floating
                };
                inner.suspended = false;
                inner.dock_error.clear();
                controller.set_visual_phase(&mut inner, "hidden", None, None);
                drop(inner);
                // Keep main hidden until the replacement overlay is ready and
                // a fresh foreground probe confirms it is safe to show.
                controller
                    .overlay_reset_complete
                    .store(true, Ordering::Release);
                return;
            }
            thread::sleep(Duration::from_millis(20));
        });
        true
    }

    pub fn request_hide(&self) {
        self.requested_visible.store(false, Ordering::Release);
        if let Some(main) = self.app.get_webview_window("main") {
            hide_main_window(&main);
        }
        if let Some(overlay) = self.app.get_webview_window(OVERLAY_LABEL) {
            hide_overlay_window(&overlay);
        }
        if let Ok(mut inner) = self.inner.try_lock() {
            inner.requested_visible = false;
            self.set_visual_phase(&mut inner, "hidden", None, None);
            let current = status(&inner);
            drop(inner);
            self.emit(current);
        }
    }

    /// Called from the main window's move and resize events. Only a pointer
    /// gesture starts a drag session; programmatic or keyboard moves never dock.
    pub fn note_window_move(&self, window: &WebviewWindow) {
        if window.label() != "main"
            || self.drag_active.load(Ordering::Acquire)
            || self.compact_docking_active.load(Ordering::Acquire)
            || self.stopping.load(Ordering::Acquire)
            || self.exit_in_progress.load(Ordering::Acquire)
        {
            return;
        }
        let hwnd = self.main_hwnd.load(Ordering::Acquire);
        if hwnd == 0 || !primary_button_pressed() || !native_floating_compact(hwnd) {
            return;
        }
        if self
            .drag_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let controller = self.clone();
        thread::spawn(move || {
            controller.watch_drag(hwnd);
            controller.drag_active.store(false, Ordering::Release);
        });
    }

    fn watch_drag(&self, main_hwnd: u64) {
        let mut probe: Option<(Instant, DragProbe)> = None;
        let mut hint = DragDockHint::None;
        loop {
            if self.stopping.load(Ordering::Acquire)
                || self.exit_in_progress.load(Ordering::Acquire)
                || self.compact_docking_active.load(Ordering::Acquire)
            {
                break;
            }
            // Sample the button first so a release during the probe still uses
            // the final window position.
            let released = !primary_button_pressed();
            if probe
                .as_ref()
                .is_none_or(|(sampled, _)| sampled.elapsed() >= DRAG_PROBE_INTERVAL)
            {
                probe = self.probe_for_drag().map(|sample| (Instant::now(), sample));
            }
            let next = match (probe.as_ref(), native_client_screen_rect(main_hwnd)) {
                (Some((_, sample)), Some(bounds)) if native_floating_compact(main_hwnd) => {
                    sample.hint(bounds)
                }
                _ => DragDockHint::None,
            };
            if next != hint {
                self.emit_drag_hint(next);
                hint = next;
            }
            if released {
                if hint == DragDockHint::Ready {
                    self.dock_after_release();
                }
                break;
            }
            thread::sleep(DRAG_SAMPLE_INTERVAL);
        }
        if hint != DragDockHint::None {
            self.emit_drag_hint(DragDockHint::None);
        }
    }

    fn probe_for_drag(&self) -> Option<DragProbe> {
        let mut inner = self.inner.lock().ok()?;
        if inner.attached || inner.transitioning || !inner.compact {
            return None;
        }
        let hwnd = inner.hwnd?;
        let value = inner
            .native
            .request(&self.app, hwnd, "probe", json!({}))
            .ok()?;
        let probe: ProbeResult = serde_json::from_value(value).ok()?;
        Some(DragProbe {
            complete: probe.complete,
            bars: probe.bars,
            overlay_ready: self.overlay_ready.load(Ordering::Acquire)
                && inner.overlay_snapshot.is_some(),
        })
    }

    fn emit_drag_hint(&self, hint: DragDockHint) {
        if let Err(error) = self
            .app
            .emit_to("main", HINT_EVENT, json!({ "state": hint.as_str() }))
        {
            eprintln!("[taskbar-docking] failed to publish the drag hint: {error}");
        }
    }

    fn dock_after_release(&self) {
        let Some(main) = self.app.get_webview_window("main") else {
            return;
        };
        let Ok(overlay) = self.overlay_window() else {
            return;
        };
        let Ok(mut inner) = self.lock() else {
            return;
        };
        if inner.enabled || inner.attached || !inner.compact {
            return;
        }
        if let Err(error) = persist_preference(&self.app, true) {
            eprintln!("[taskbar-docking] could not save the docking preference: {error}");
            return;
        }
        inner.enabled = true;
        self.compact_docking_active.store(true, Ordering::Release);
        inner.transitioning = true;
        let result = self.refresh_locked(&mut inner, &main, &overlay);
        inner.transitioning = false;
        if !inner.attached {
            // A release that could not dock leaves an ordinary floating player
            // instead of a pending intent that would snap later.
            if let Err(error) = &result {
                eprintln!("[taskbar-docking] release did not dock: {error}");
            }
            if self
                .return_to_main_locked(&mut inner, &main, &overlay)
                .is_ok()
            {
                inner.enabled = false;
                if let Err(error) = persist_preference(&self.app, false) {
                    eprintln!("[taskbar-docking] could not clear the docking preference: {error}");
                }
                inner.mode = DockMode::Floating;
                inner.suspended = false;
                inner.dock_error.clear();
            }
        }
        self.compact_docking_active
            .store(inner.enabled && inner.compact, Ordering::Release);
        let current = status(&inner);
        drop(inner);
        self.emit(current);
    }

    pub fn should_persist_main_frame(&self, window: &WebviewWindow) -> bool {
        let Ok(inner) = self.inner.try_lock() else {
            // This runs from native window callbacks. If the docking worker is
            // waiting for the UI thread, skip this persistence attempt rather
            // than making the two threads wait on each other.
            return false;
        };
        let Ok((_, logical_height)) = logical_inner_size(window) else {
            return false;
        };
        model::should_persist_frame(
            inner.compact || logical_height < 340,
            inner.attached,
            inner.transitioning,
        )
    }

    pub fn defer_exit(&self, exit_code: i32) -> bool {
        if self.exit_prepared.load(Ordering::Acquire) {
            return false;
        }
        if self.exit_in_progress.swap(true, Ordering::AcqRel) {
            return true;
        }
        self.requested_visible.store(false, Ordering::Release);
        let controller = self.clone();
        // SetParent can send synchronous UI messages; keep the event loop alive.
        thread::spawn(move || {
            let restored = (|| -> Result<(), String> {
                let Some(overlay) = controller.app.get_webview_window(OVERLAY_LABEL) else {
                    return Ok(());
                };
                let mut inner = controller.lock()?;
                inner.requested_visible = false;
                let expected = inner.recovery_frame.clone();
                let mut result = Err("native detach was not attempted".to_string());
                for _ in 0..2 {
                    result = controller.detach_locked(&mut inner, &overlay);
                    if result.is_ok() {
                        break;
                    }
                }
                result?;
                if let Some(expected) = expected {
                    let hwnd = native_hwnd(&overlay)?;
                    let actual = inner
                        .native
                        .request(&controller.app, hwnd, "inspect", json!({}))
                        .map_err(|error| format_native_error(&error))?;
                    let actual = parse_recovery_frame(actual, hwnd)?;
                    if !exit_recovery_frame_matches(&expected, &actual) {
                        return Err("native frame was not restored before exit".to_string());
                    }
                    eprintln!(
                        "[taskbar-docking] restored native frame before exit: {}",
                        serde_json::to_string(&actual).map_err(|error| error.to_string())?
                    );
                }
                inner.compact = false;
                inner.mode = DockMode::Floating;
                controller
                    .compact_docking_active
                    .store(false, Ordering::Release);
                Ok(())
            })();
            if let Err(error) = restored {
                eprintln!("[taskbar-docking] exit deferred because restoration failed: {error}");
                controller.exit_in_progress.store(false, Ordering::Release);
                return;
            }
            controller.stopping.store(true, Ordering::Release);
            controller.exit_prepared.store(true, Ordering::Release);
            controller.app.exit(exit_code);
        });
        true
    }

    pub fn shutdown(&self, _window: Option<&WebviewWindow>) {
        self.stopping.store(true, Ordering::Release);
        self.requested_visible.store(false, Ordering::Release);
        if let Ok(mut inner) = self.inner.try_lock() {
            inner.requested_visible = false;
            inner.native.close_nonblocking();
        }
    }

    fn tick(&self, main: &WebviewWindow, overlay: &WebviewWindow) {
        if main.label() != "main" || overlay.label() != OVERLAY_LABEL {
            return;
        }
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.requested_visible = self.requested_visible.load(Ordering::Acquire);
        let old = status(&inner);
        let _ = self.refresh_locked(&mut inner, main, overlay);
        let current = status(&inner);
        let publish =
            should_publish_refreshed_status(&old, &current, inner.published_intermediate_status);
        drop(inner);
        if publish {
            self.emit(current);
        }
    }

    fn refresh_locked(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
    ) -> Result<(), String> {
        inner.published_intermediate_status = false;
        let window = main;
        let current_hwnd = match native_hwnd(overlay) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                if inner.enabled && inner.compact {
                    fail_closed(
                        inner,
                        main,
                        overlay,
                        "The overlay window handle is unavailable",
                    );
                }
                return Err(error);
            }
        };
        if inner.hwnd != Some(current_hwnd) {
            let replacing_window = inner.hwnd.is_some();
            let previous_resizable = inner.recovery_resizable;
            inner.native.close();
            inner.attached = false;
            inner.attached_bar = None;
            inner.recovery_pending = false;
            inner.recovery_frame = None;
            inner.recovery_resizable = None;
            if replacing_window {
                let value =
                    match inner
                        .native
                        .request(&self.app, current_hwnd, "inspect", json!({}))
                    {
                        Ok(value) => value,
                        Err(error) => {
                            if inner.enabled || inner.compact {
                                fail_closed(
                                    inner,
                                    main,
                                    overlay,
                                    "The replacement overlay frame is unavailable",
                                );
                            }
                            return Err(format_native_error(&error));
                        }
                    };
                let frame = match parse_recovery_frame(value, current_hwnd) {
                    Ok(frame) => frame,
                    Err(error) => {
                        fail_closed(
                            inner,
                            main,
                            overlay,
                            "The replacement overlay frame is invalid",
                        );
                        return Err(error);
                    }
                };
                // Preserve the replacement HWND's own frame; never restore a
                // stale recovery frame from the HWND that was destroyed. Keep
                // the resize state instead of a temporary native dock flag.
                let resizable = previous_resizable.unwrap_or(false);
                let frame = recovery_frame_with_resizable(frame, resizable)?;
                inner.hwnd = Some(current_hwnd);
                inner.recovery_resizable = Some(resizable);
                inner.recovery_frame = Some(frame);
                inner.recovery_pending = true;
            } else {
                inner.hwnd = Some(current_hwnd);
            }
        }

        if inner.recovery_pending
            || (inner.attached && !inner.native.has_live_session(current_hwnd))
        {
            inner.recovery_pending = true;
            if !self.restore_recovery_frame(inner, overlay, current_hwnd) {
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "The previous overlay attachment is being recovered",
                );
                return Ok(());
            }
        }

        if !inner.compact || !inner.enabled {
            if inner.attached || inner.recovery_frame.is_some() {
                self.detach_locked(inner, overlay)?;
            }
            inner.mode = DockMode::Floating;
            inner.suspended = false;
            inner.dock_error.clear();
            self.show_regular_requested(main);
            hide_overlay_window(overlay);
            return Ok(());
        }
        let initial_live_compact = match window_is_live_compact(window) {
            Ok(eligible) => eligible,
            Err(error) => {
                fail_closed(inner, main, overlay, "The main window state is unavailable");
                return Err(error);
            }
        };
        if !initial_live_compact {
            if inner.suspended {
                self.ensure_transition_safety(inner, main, overlay, None)?;
            }
            self.detach_for_compact_guard(inner, main, overlay)?;
            inner.mode = DockMode::Floating;
            inner.suspended = false;
            inner.dock_error.clear();
            self.show_regular_requested(main);
            hide_overlay_window(overlay);
            return Ok(());
        }

        let foreground_before_probe = foreground_context();
        let probe_value = match inner
            .native
            .request(&self.app, current_hwnd, "probe", json!({}))
        {
            Ok(value) => value,
            Err(error) => {
                self.recover_helper(inner, main, current_hwnd, &error);
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "Taskbar detection is temporarily unavailable",
                );
                return Err("taskbar detection failed".to_string());
            }
        };
        let probe: ProbeResult = match serde_json::from_value(probe_value) {
            Ok(probe) => probe,
            Err(_) => {
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "Taskbar detection returned invalid data",
                );
                return Err("invalid taskbar probe".to_string());
            }
        };
        if !probe.complete {
            fail_closed(inner, main, overlay, "Taskbar detection is incomplete");
            return Ok(());
        }
        let mut foreground_checkpoint = foreground_before_probe;
        let foreground_after_probe = foreground_context();
        if model::foreground_context_changed(foreground_checkpoint, foreground_after_probe) {
            self.ensure_transition_safety_if_foreground_changed(
                inner,
                main,
                overlay,
                None,
                &mut foreground_checkpoint,
            )?;
        } else {
            foreground_checkpoint = foreground_after_probe;
        }
        let bounds = match window_rect(window) {
            Ok(bounds) => bounds,
            Err(error) => {
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "The main window geometry is unavailable",
                );
                return Err(error);
            }
        };
        let Some(bar) = bar_for_window(&probe.bars, bounds) else {
            fail_closed(
                inner,
                main,
                overlay,
                "The taskbar for this monitor is unavailable",
            );
            return Ok(());
        };
        let safety = match bar.fullscreen {
            Some(true) => ForegroundSafety::Fullscreen,
            Some(false) if !bar.suspended => ForegroundSafety::Clear,
            _ => ForegroundSafety::Unknown,
        };
        let safety_decision = model::dock_decision(
            inner.enabled,
            inner.compact,
            inner.requested_visible,
            safety,
        );
        if safety_decision.suspended {
            inner.mode = safety_decision.mode;
            inner.suspended = true;
            inner.dock_error.clear();
            hide_overlay_window(overlay);
            hide_main_window(main);
            return Ok(());
        }
        if safety != ForegroundSafety::Clear {
            inner.mode = DockMode::Floating;
            inner.suspended = false;
            inner.dock_error.clear();
            return Ok(());
        }

        // The UI compact flag can lag native resizing, maximizing, or entering
        // fullscreen. Recheck the actual HWND immediately before docking.
        let live_compact = match window_is_live_compact(window) {
            Ok(eligible) => eligible,
            Err(error) => {
                fail_closed(inner, main, overlay, "The main window state is unavailable");
                return Err(error);
            }
        };
        if !live_compact {
            self.detach_for_compact_guard(inner, main, overlay)?;
            inner.mode = DockMode::Floating;
            inner.suspended = false;
            inner.dock_error.clear();
            self.show_regular_requested(main);
            hide_overlay_window(overlay);
            return Ok(());
        }

        let Some(free) = bar.free else {
            inner.mode = DockMode::Waiting;
            inner.suspended = false;
            inner.dock_error = "Taskbar layout is temporarily unavailable".to_string();
            if inner.attached {
                inner.suspended = true;
                hide_overlay_window(overlay);
                hide_main_window(main);
            } else if let Err(error) = self.show_requested_locked(inner, overlay, current_hwnd) {
                self.recover_helper(inner, main, current_hwnd, &error);
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "The overlay could not be shown safely",
                );
                return Err("native show failed".to_string());
            }
            return Ok(());
        };
        let Some(target) = model::dock_target(free.into(), bar.dpi) else {
            inner.mode = DockMode::Waiting;
            inner.suspended = false;
            inner.dock_error = "There is not enough free taskbar space".to_string();
            if inner.attached {
                inner.suspended = true;
                hide_overlay_window(overlay);
                hide_main_window(main);
            } else {
                self.show_regular_requested(main);
                hide_overlay_window(overlay);
            }
            return Ok(());
        };

        if !self.overlay_ready.load(Ordering::Acquire) || inner.overlay_snapshot.is_none() {
            inner.mode = DockMode::Waiting;
            inner.suspended = false;
            inner.dock_error.clear();
            hide_overlay_window(overlay);
            self.show_regular_requested(main);
            return Ok(());
        }

        if !inner.attached && primary_button_pressed() {
            // A move or resize loop still owns the pointer. Native operations
            // queued behind that loop would land after attachment; dock after
            // the release instead.
            inner.mode = DockMode::Waiting;
            inner.suspended = false;
            inner.dock_error.clear();
            hide_overlay_window(overlay);
            self.show_regular_requested(main);
            return Ok(());
        }

        let was_suspended = inner.suspended;
        if !inner.attached {
            // Capture the overlay's actual floating frame before its visual
            // transition; the real player HWND is never passed to the helper.
            if inner.recovery_frame.is_none() {
                let frame_value = inner
                    .native
                    .request(&self.app, current_hwnd, "inspect", json!({}))
                    .map_err(|error| format_native_error(&error))?;
                inner.recovery_frame = Some(parse_recovery_frame(frame_value, current_hwnd)?);
                inner.recovery_resizable = Some(false);
            }
            inner.mode = DockMode::Docking;
            if self.requested_visible.load(Ordering::Acquire) {
                let flight = OverlayFlight {
                    from: client_screen_rect(main)?,
                    to: target.into(),
                    phase: "snapping",
                };
                set_native_window_rect(current_hwnd, flight.from, false)?;
                let flown = match self.prepare_overlay_visual(inner, overlay, &flight) {
                    Some(_) => self.run_overlay_flight(
                        inner,
                        main,
                        overlay,
                        &flight,
                        Some(&bar.handle),
                        &mut foreground_checkpoint,
                    )?,
                    None => false,
                };
                if !flown {
                    inner.mode = DockMode::Waiting;
                    inner.suspended = false;
                    hide_overlay_window(overlay);
                    self.show_regular_requested(main);
                    return Ok(());
                }
            }
            inner.published_intermediate_status = true;
            self.emit(status(inner));
        }
        match self.attach_locked(inner, main, overlay, current_hwnd, bar) {
            Ok(true) => {}
            Ok(false) => {
                inner.mode = DockMode::Floating;
                inner.suspended = false;
                inner.dock_error.clear();
                self.show_regular_requested(main);
                hide_overlay_window(overlay);
                return Ok(());
            }
            Err(error) => {
                if inner.recovery_frame.is_some() || is_recoverable_native_error(&error) {
                    self.recover_helper(
                        inner,
                        main,
                        current_hwnd,
                        &NativeError {
                            code: None,
                            session_failed: true,
                        },
                    );
                }
                if !inner.attached {
                    hide_overlay_window(overlay);
                    self.show_regular_requested(main);
                    self.set_visual_phase(inner, "hidden", None, None);
                } else {
                    fail_closed(inner, main, overlay, "Taskbar attachment will be retried");
                }
                return Err(error);
            }
        }
        // Hand the landed flight to the docked strip before the slower
        // foreground probe; an unsafe result hides it again.
        inner.mode = DockMode::Docked;
        inner.suspended = false;
        inner.dock_error.clear();
        if inner.visual_phase != "docked" {
            self.set_visual_phase(inner, "docked", None, None);
        }
        self.ensure_transition_safety(inner, main, overlay, Some(&bar.handle))?;
        if was_suspended || inner.requested_visible {
            if let Err(error) = self.show_requested_locked(inner, overlay, current_hwnd) {
                self.recover_helper(inner, main, current_hwnd, &error);
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "The overlay could not be shown safely",
                );
                return Err("native show failed".to_string());
            }
        }
        if !self.requested_visible.load(Ordering::Acquire) {
            hide_overlay_window(overlay);
            hide_main_window(main);
        }
        Ok(())
    }

    fn attach_locked(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
        hwnd: u64,
        bar: &TaskbarObservation,
    ) -> Result<bool, String> {
        if !window_is_live_compact(main)? {
            self.detach_for_compact_guard(inner, main, overlay)?;
            return Ok(false);
        }
        if inner.attached && inner.attached_bar.as_deref() != Some(bar.handle.as_str()) {
            self.detach_locked(inner, overlay)?;
        }
        let (content_width, content_height) = native_client_size(main)?;
        if content_width == 0
            || content_height == 0
            || content_width > 100_000
            || content_height > 100_000
        {
            return Err("main window client geometry is outside the supported range".to_string());
        }
        if inner.recovery_frame.is_none() {
            let value = inner
                .native
                .request(&self.app, hwnd, "inspect", json!({}))
                .map_err(|error| format_native_error(&error))?;
            let frame = parse_recovery_frame(value, hwnd)?;
            inner.recovery_resizable = Some(false);
            inner.recovery_frame = Some(frame);
            // This is the dedicated borderless overlay HWND. The real player
            // window is intentionally never resized, restyled, or reparented.
            overlay.outer_size().map_err(|error| error.to_string())?;
        }
        let request = json!({
            "bar": bar.handle,
            "contentWidth": content_width,
            "contentHeight": content_height
        });
        // Inspect can block on the native helper. Validate again after it so
        // a resize/maximize/fullscreen transition during IPC cannot attach a
        // now-normal window using stale compact state.
        if !window_is_live_compact(main)? {
            self.detach_for_compact_guard(inner, main, overlay)?;
            return Ok(false);
        }
        match inner.native.request(&self.app, hwnd, "attach", request) {
            Ok(_) => {
                inner.attached = true;
                inner.attached_bar = Some(bar.handle.clone());
                if !window_is_live_compact(main)? {
                    self.detach_for_compact_guard(inner, main, overlay)?;
                    return Ok(false);
                }
                Ok(true)
            }
            Err(error) => {
                if let Some(frame) = inner.recovery_frame.clone() {
                    let restored = self.restore_overlay_frame(inner, hwnd, &frame).is_ok();
                    if restored {
                        inner.recovery_frame = None;
                        inner.recovery_resizable = None;
                        inner.attached = false;
                        inner.attached_bar = None;
                        inner.recovery_pending = false;
                        self.hide_if_not_requested_visible(overlay);
                    } else {
                        inner.recovery_pending = true;
                    }
                }
                Err(format_native_error(&error))
            }
        }
    }

    fn detach_locked(&self, inner: &mut Inner, overlay: &WebviewWindow) -> Result<(), String> {
        if !inner.attached && inner.recovery_frame.is_none() {
            return Ok(());
        }
        let hwnd = inner
            .hwnd
            .or_else(|| native_hwnd(overlay).ok())
            .ok_or_else(|| "overlay window handle is unavailable".to_string())?;
        let frame = inner
            .recovery_frame
            .clone()
            .ok_or_else(|| "the saved native frame is unavailable".to_string())?;
        if self.restore_overlay_frame(inner, hwnd, &frame).is_err() {
            inner.native.reset();
            inner.recovery_pending = true;
            let _ = self.restore_recovery_frame(inner, overlay, hwnd);
            if inner.attached || inner.recovery_frame.is_some() {
                inner.recovery_pending = true;
                inner.mode = DockMode::Waiting;
                inner.suspended = true;
                inner.dock_error = "The overlay could not be detached safely".to_string();
                return Err("native detach failed; overlay frame was preserved".to_string());
            }
        }

        inner.attached = false;
        inner.attached_bar = None;
        inner.recovery_pending = false;
        inner.recovery_frame = None;
        inner.recovery_resizable = None;
        self.hide_if_not_requested_visible(overlay);
        Ok(())
    }

    fn detach_for_compact_guard(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
    ) -> Result<(), String> {
        let result = if inner.attached || inner.recovery_frame.is_some() {
            self.detach_locked(inner, overlay)
        } else {
            Ok(())
        };
        // Restoring the saved native style can reapply WS_VISIBLE. Reassert
        // an explicit hide even when detach reports a recovery error.
        self.hide_if_not_requested_visible(overlay);
        if !self.requested_visible.load(Ordering::Acquire) {
            hide_main_window(main);
        }
        result
    }

    fn recover_helper(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        hwnd: u64,
        _error: &NativeError,
    ) {
        inner.native.reset();
        if inner.recovery_frame.is_none() {
            return;
        }
        inner.recovery_pending = true;
        if let Some(overlay) = self.app.get_webview_window(OVERLAY_LABEL) {
            let _ = self.restore_recovery_frame(inner, &overlay, hwnd);
        }
        if self.compact_docking_active.load(Ordering::Acquire) {
            hide_main_window(main);
        }
    }

    fn restore_recovery_frame(&self, inner: &mut Inner, window: &WebviewWindow, hwnd: u64) -> bool {
        let Some(frame) = inner
            .recovery_frame
            .clone()
            .filter(|frame| frame.hwnd == hwnd.to_string())
        else {
            return false;
        };
        if self.restore_overlay_frame(inner, hwnd, &frame).is_ok() {
            inner.attached = false;
            inner.attached_bar = None;
            inner.recovery_frame = None;
            inner.recovery_resizable = None;
            inner.recovery_pending = false;
            self.hide_if_not_requested_visible(window);
            true
        } else {
            inner.recovery_pending = true;
            false
        }
    }

    fn restore_overlay_frame(
        &self,
        inner: &mut Inner,
        hwnd: u64,
        frame: &RecoveryFrame,
    ) -> Result<(), String> {
        self.restore_exact_frame(inner, hwnd, frame)
    }

    fn restore_exact_frame(
        &self,
        inner: &mut Inner,
        hwnd: u64,
        frame: &RecoveryFrame,
    ) -> Result<(), String> {
        // Recovery always restores the independent overlay hidden. The caller
        // may only show it again after a fresh probe validates the environment.
        let target = hidden_recovery_frame(frame)?;
        let encoded = serde_json::to_value(&target).map_err(|error| error.to_string())?;
        inner
            .native
            .request(&self.app, hwnd, "restore", json!({"frame":encoded}))
            .map_err(|error| format_native_error(&error))?;
        let actual = inner
            .native
            .request(&self.app, hwnd, "inspect", json!({}))
            .map_err(|error| format_native_error(&error))?;
        let actual = parse_recovery_frame(actual, hwnd)?;
        if !recovery_frames_match(&target, &actual) {
            return Err("restored native frame does not match the saved frame".to_string());
        }
        Ok(())
    }

    fn show_requested_locked(
        &self,
        inner: &mut Inner,
        window: &WebviewWindow,
        hwnd: u64,
    ) -> Result<(), NativeError> {
        if self.requested_visible.load(Ordering::Acquire)
            && (window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(false))
        {
            inner.native.request(&self.app, hwnd, "show", json!({}))?;
            if !self.requested_visible.load(Ordering::Acquire) {
                hide_overlay_window(window);
            }
        }
        Ok(())
    }

    fn show_regular_requested(&self, window: &WebviewWindow) {
        if self.requested_visible.load(Ordering::Acquire)
            && (window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(false))
        {
            if self.requested_visible.load(Ordering::Acquire) {
                show_main_inactive(window);
            } else {
                hide_taskbar_window(window);
            }
        }
    }

    fn hide_if_not_requested_visible(&self, overlay: &WebviewWindow) {
        if !self.requested_visible.load(Ordering::Acquire) {
            hide_overlay_window(overlay);
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, String> {
        self.inner
            .lock()
            .map_err(|error| format!("taskbar docking state is unavailable: {error}"))
    }

    fn emit_current(&self) {
        if let Ok(inner) = self.inner.lock() {
            self.emit(status(&inner));
        }
    }

    fn emit(&self, state: TaskbarDockStatus) {
        if let Err(error) = self.app.emit(EVENT, state) {
            eprintln!("[taskbar-docking] failed to publish state: {error}");
        }
    }

    fn set_visual_phase(
        &self,
        inner: &mut Inner,
        phase: &'static str,
        from: Option<NativeRect>,
        to: Option<NativeRect>,
    ) {
        inner.visual_token = inner.visual_token.wrapping_add(1).max(1);
        inner.visual_phase = phase;
        self.current_visual_token
            .store(inner.visual_token, Ordering::Release);
        let payload = OverlayVisual {
            phase,
            token: inner.visual_token,
            from,
            to,
            duration_ms: overlay_animation_duration(inner.overlay_snapshot.as_ref()).as_millis()
                as u32,
        };
        if let Err(error) = self
            .app
            .emit_to(OVERLAY_LABEL, OVERLAY_VISUAL_EVENT, payload)
        {
            eprintln!("[taskbar-docking] failed to publish overlay visual state: {error}");
        }
    }

    fn ensure_transition_safety(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
        selected_bar: Option<&str>,
    ) -> Result<(), String> {
        let hwnd = match native_hwnd(overlay) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "The overlay window handle is unavailable",
                );
                self.set_visual_phase(inner, "hidden", None, None);
                return Err(error);
            }
        };
        let value = match inner.native.request(&self.app, hwnd, "probe", json!({})) {
            Ok(value) => value,
            Err(error) => {
                self.recover_helper(inner, main, hwnd, &error);
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "Foreground safety could not be verified during the transition",
                );
                self.set_visual_phase(inner, "hidden", None, None);
                return Err("foreground safety probe failed during overlay transition".to_string());
            }
        };
        let probe = match serde_json::from_value::<ProbeResult>(value) {
            Ok(probe) => probe,
            Err(_) => {
                self.recover_helper(
                    inner,
                    main,
                    hwnd,
                    &NativeError {
                        code: None,
                        session_failed: true,
                    },
                );
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "Foreground safety returned invalid data during the transition",
                );
                self.set_visual_phase(inner, "hidden", None, None);
                return Err("foreground safety probe returned invalid data".to_string());
            }
        };
        let is_safe = if let Some(handle) = selected_bar {
            selected_taskbar_is_safe(&probe, handle)
        } else if probe.complete {
            match window_rect(main) {
                Ok(bounds) => bar_for_window(&probe.bars, bounds)
                    .is_some_and(|bar| selected_taskbar_is_safe(&probe, &bar.handle)),
                Err(error) => {
                    fail_closed(
                        inner,
                        main,
                        overlay,
                        "Foreground safety could not be matched to the main window",
                    );
                    self.set_visual_phase(inner, "hidden", None, None);
                    return Err(error);
                }
            }
        } else {
            false
        };
        if !is_safe {
            inner.mode = DockMode::Waiting;
            inner.suspended = true;
            inner.dock_error.clear();
            hide_main_window(main);
            hide_overlay_window(overlay);
            self.set_visual_phase(inner, "hidden", None, None);
            return Err("foreground safety changed during overlay transition".to_string());
        }
        Ok(())
    }

    fn ensure_transition_safety_if_foreground_changed(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
        selected_bar: Option<&str>,
        checkpoint: &mut Option<model::ForegroundContext>,
    ) -> Result<(), String> {
        let mut before = foreground_context();
        if before.is_none() {
            fail_closed(
                inner,
                main,
                overlay,
                "Foreground context is unavailable during transition",
            );
            self.set_visual_phase(inner, "hidden", None, None);
            return Err("foreground context is unavailable during transition".to_string());
        }
        if !model::foreground_context_changed(*checkpoint, before) {
            return Ok(());
        }

        // A complete taskbar probe can take long enough for the foreground to
        // change again. Repeat once when its HWND/rect changed during the
        // probe; if it keeps moving, leave both windows hidden and let the
        // normal poller retry after the transition settles.
        for _ in 0..2 {
            self.ensure_transition_safety(inner, main, overlay, selected_bar)?;
            let after = foreground_context();
            if after.is_none() {
                fail_closed(
                    inner,
                    main,
                    overlay,
                    "Foreground context became unavailable during transition",
                );
                self.set_visual_phase(inner, "hidden", None, None);
                return Err("foreground context became unavailable during transition".to_string());
            }
            if before == after {
                *checkpoint = after;
                return Ok(());
            }
            before = Some(after.expect("foreground context checked above"));
        }

        fail_closed(
            inner,
            main,
            overlay,
            "Foreground changed while safety was being verified",
        );
        self.set_visual_phase(inner, "hidden", None, None);
        Err("foreground changed during transition safety verification".to_string())
    }

    fn recheck_flight_safety_if_foreground_changed(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
        selected_bar: Option<&str>,
        checkpoint: &mut Option<model::ForegroundContext>,
        overlay_visible: bool,
    ) -> Result<(), String> {
        let token = inner.visual_token;
        if !model::foreground_context_changed(*checkpoint, foreground_context()) {
            return Ok(());
        }
        if overlay_visible {
            hide_overlay_window(overlay);
        }
        self.ensure_transition_safety_if_foreground_changed(
            inner,
            main,
            overlay,
            selected_bar,
            checkpoint,
        )?;
        if overlay_visible
            && self.requested_visible.load(Ordering::Acquire)
            && self.current_visual_token.load(Ordering::Acquire) == token
        {
            if let Ok(hwnd) = native_hwnd(overlay) {
                show_overlay_on_top(hwnd);
            }
        }
        Ok(())
    }

    /// Publishes a flight phase and waits until the overlay has painted it,
    /// so the window is never revealed with stale content.
    fn prepare_overlay_visual(
        &self,
        inner: &mut Inner,
        overlay: &WebviewWindow,
        flight: &OverlayFlight,
    ) -> Option<u64> {
        if !flight.from.into_rect().is_valid() || !flight.to.into_rect().is_valid() {
            return None;
        }
        self.set_visual_phase(inner, flight.phase, Some(flight.from), Some(flight.to));
        let token = inner.visual_token;
        let deadline = Instant::now() + VISUAL_READY_TIMEOUT;
        loop {
            if !self.requested_visible.load(Ordering::Acquire)
                || self.current_visual_token.load(Ordering::Acquire) != token
            {
                hide_overlay_window(overlay);
                self.set_visual_phase(inner, "hidden", None, None);
                return None;
            }
            if self.visual_ready_token.load(Ordering::Acquire) == token {
                return Some(token);
            }
            if Instant::now() >= deadline {
                self.set_visual_phase(inner, "hidden", None, None);
                return None;
            }
            thread::sleep(Duration::from_millis(4));
        }
    }

    /// Moves the prepared overlay between the player and the taskbar. The
    /// loop samples the foreground HWND and rect, running the full taskbar
    /// safety probe only when that lightweight context changes.
    fn run_overlay_flight(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
        flight: &OverlayFlight,
        selected_bar: Option<&str>,
        foreground_checkpoint: &mut Option<model::ForegroundContext>,
    ) -> Result<bool, String> {
        let token = inner.visual_token;
        let hwnd = native_hwnd(overlay)?;
        set_native_window_rect(hwnd, flight.from, false)?;
        self.recheck_flight_safety_if_foreground_changed(
            inner,
            main,
            overlay,
            selected_bar,
            foreground_checkpoint,
            false,
        )?;
        if !self.requested_visible.load(Ordering::Acquire)
            || self.current_visual_token.load(Ordering::Acquire) != token
        {
            hide_overlay_window(overlay);
            self.set_visual_phase(inner, "hidden", None, None);
            return Ok(false);
        }
        // Cover the player before hiding it so no frame shows neither window.
        show_overlay_on_top(hwnd);
        if flight.phase == "snapping" {
            hide_main_window(main);
        }
        let duration = overlay_animation_duration(inner.overlay_snapshot.as_ref());
        let reduced = duration == REDUCED_MOTION_DURATION;
        if reduced {
            set_native_window_rect(hwnd, flight.to, false)?;
        }
        let start = Instant::now();
        loop {
            if !self.requested_visible.load(Ordering::Acquire)
                || self.current_visual_token.load(Ordering::Acquire) != token
            {
                hide_overlay_window(overlay);
                self.set_visual_phase(inner, "hidden", None, None);
                return Ok(false);
            }
            self.recheck_flight_safety_if_foreground_changed(
                inner,
                main,
                overlay,
                selected_bar,
                foreground_checkpoint,
                true,
            )?;
            if !self.requested_visible.load(Ordering::Acquire)
                || self.current_visual_token.load(Ordering::Acquire) != token
            {
                hide_overlay_window(overlay);
                self.set_visual_phase(inner, "hidden", None, None);
                return Ok(false);
            }
            let progress = (start.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0);
            if !reduced {
                if let Some(rect) =
                    model::interpolate_rect(flight.from.into(), flight.to.into(), progress)
                {
                    set_native_window_rect(hwnd, rect.into(), false)?;
                }
            }
            if progress >= 1.0 {
                if flight.phase == "returning" {
                    // Hide before the final full probe so a newly foregrounded
                    // game is never covered while the safety check runs.
                    hide_overlay_window(overlay);
                    // This is the final full verification before the caller
                    // reveals the main window.
                    *foreground_checkpoint = None;
                    self.ensure_transition_safety_if_foreground_changed(
                        inner,
                        main,
                        overlay,
                        selected_bar,
                        foreground_checkpoint,
                    )?;
                }
                if !self.requested_visible.load(Ordering::Acquire)
                    || self.current_visual_token.load(Ordering::Acquire) != token
                {
                    hide_overlay_window(overlay);
                    self.set_visual_phase(inner, "hidden", None, None);
                    return Ok(false);
                }
                return Ok(true);
            }
            wait_for_next_frame();
        }
    }

    fn return_to_main_locked(
        &self,
        inner: &mut Inner,
        main: &WebviewWindow,
        overlay: &WebviewWindow,
    ) -> Result<(), String> {
        let requested = self.requested_visible.load(Ordering::Acquire);
        let selected_bar = inner.attached_bar.clone();
        let docked_rect = if inner.attached
            && requested
            && selected_bar.is_some()
            && self.overlay_ready.load(Ordering::Acquire)
        {
            let hwnd = inner
                .hwnd
                .or_else(|| native_hwnd(overlay).ok())
                .ok_or_else(|| "overlay window handle is unavailable".to_string())?;
            inner
                .native
                .request(&self.app, hwnd, "inspect", json!({}))
                .map_err(|error| format_native_error(&error))
                .and_then(|value| parse_recovery_frame(value, hwnd))
                .ok()
                .map(|frame| frame.rect)
        } else {
            None
        };
        let flight = match (docked_rect, client_screen_rect(main)) {
            (Some(from), Ok(to)) => Some(OverlayFlight {
                from,
                to,
                phase: "returning",
            }),
            _ => None,
        };
        // Switch the strip to its flight look while it is still in the
        // taskbar; leaving Explorer then costs a single frame.
        let token = flight
            .as_ref()
            .and_then(|flight| self.prepare_overlay_visual(inner, overlay, flight));
        hide_overlay_window(overlay);
        self.detach_locked(inner, overlay)?;
        let mut foreground_checkpoint = None;
        if self.requested_visible.load(Ordering::Acquire) {
            self.ensure_transition_safety_if_foreground_changed(
                inner,
                main,
                overlay,
                selected_bar.as_deref(),
                &mut foreground_checkpoint,
            )?;
        }
        let mut final_flight_verified = false;
        if let (Some(flight), Some(_)) = (flight.as_ref(), token) {
            inner.mode = DockMode::Undocking;
            final_flight_verified = self.run_overlay_flight(
                inner,
                main,
                overlay,
                flight,
                selected_bar.as_deref(),
                &mut foreground_checkpoint,
            )?;
        }
        // Reveal the player before removing the flight so no frame is empty.
        if self.requested_visible.load(Ordering::Acquire) {
            if !final_flight_verified {
                foreground_checkpoint = None;
            }
            self.ensure_transition_safety_if_foreground_changed(
                inner,
                main,
                overlay,
                selected_bar.as_deref(),
                &mut foreground_checkpoint,
            )?;
            if !self.requested_visible.load(Ordering::Acquire) {
                hide_main_window(main);
                hide_overlay_window(overlay);
                self.set_visual_phase(inner, "hidden", None, None);
                return Ok(());
            }
            show_main_inactive(main);
        } else {
            hide_main_window(main);
        }
        hide_overlay_window(overlay);
        self.set_visual_phase(inner, "hidden", None, None);
        Ok(())
    }

    fn publish_overlay_snapshot(&self, snapshot: Value) -> Result<(), String> {
        model::validate_overlay_snapshot(&snapshot).map_err(str::to_string)?;
        let mut inner = self.lock()?;
        inner.overlay_snapshot = Some(snapshot.clone());
        if self.overlay_ready.load(Ordering::Acquire) {
            self.app
                .emit_to(OVERLAY_LABEL, OVERLAY_SNAPSHOT_EVENT, snapshot)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    fn overlay_renderer_ready(&self) -> Result<Value, String> {
        let inner = self.lock()?;
        let snapshot = inner.overlay_snapshot.clone();
        // The renderer has installed both listeners even when the first
        // playback snapshot has not arrived yet. Remember that readiness so
        // the first later publish is delivered instead of being stranded.
        self.overlay_ready.store(true, Ordering::Release);
        if let Some(snapshot) = snapshot.as_ref() {
            self.app
                .emit_to(OVERLAY_LABEL, OVERLAY_SNAPSHOT_EVENT, snapshot)
                .map_err(|error| error.to_string())?;
        }
        Ok(json!({
            "snapshot": snapshot,
            "visual": { "phase": inner.visual_phase, "token": inner.visual_token },
            "ready": self.overlay_ready.load(Ordering::Acquire)
        }))
    }

    fn mark_visual_ready(&self, token: u64) -> bool {
        if token == 0 || self.current_visual_token.load(Ordering::Acquire) != token {
            return false;
        }
        self.visual_ready_token.store(token, Ordering::Release);
        true
    }
}

#[tauri::command]
pub async fn set_taskbar_docking(
    window: WebviewWindow,
    enabled: bool,
) -> Result<TaskbarDockStatus, String> {
    require_main(&window)?;
    let controller = (*window.app_handle().state::<TaskbarDockController>()).clone();
    tauri::async_runtime::spawn_blocking(move || controller.set_enabled(&window, enabled))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn sync_taskbar_compact_mode(
    window: WebviewWindow,
    compact: bool,
) -> Result<TaskbarDockStatus, String> {
    require_main(&window)?;
    let controller = (*window.app_handle().state::<TaskbarDockController>()).clone();
    tauri::async_runtime::spawn_blocking(move || controller.set_compact(&window, compact))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn publish_taskbar_overlay_snapshot(
    window: WebviewWindow,
    snapshot: Value,
) -> Result<(), String> {
    require_main(&window)?;
    let controller = (*window.app_handle().state::<TaskbarDockController>()).clone();
    tauri::async_runtime::spawn_blocking(move || controller.publish_overlay_snapshot(snapshot))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn taskbar_overlay_ready(window: WebviewWindow) -> Result<Value, String> {
    require_overlay(&window)?;
    let controller = (*window.app_handle().state::<TaskbarDockController>()).clone();
    tauri::async_runtime::spawn_blocking(move || controller.overlay_renderer_ready())
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn taskbar_overlay_visual_ready(window: WebviewWindow, token: u64) -> bool {
    if require_overlay(&window).is_err() {
        return false;
    }
    window
        .app_handle()
        .try_state::<TaskbarDockController>()
        .is_some_and(|controller| controller.mark_visual_ready(token))
}

#[tauri::command]
pub async fn taskbar_overlay_action(
    window: WebviewWindow,
    action: String,
    value: Option<f64>,
) -> Result<(), String> {
    require_overlay(&window)?;
    model::validate_overlay_action(&action, value).map_err(str::to_string)?;
    let app = window.app_handle().clone();
    let controller = (*app.state::<TaskbarDockController>()).clone();
    match action.as_str() {
        "close" => controller.request_hide(),
        "undock" => {
            let main = app
                .get_webview_window("main")
                .ok_or_else(|| "main window is unavailable".to_string())?;
            tauri::async_runtime::spawn_blocking(move || controller.set_enabled(&main, false))
                .await
                .map_err(|error| error.to_string())??;
        }
        "previous" | "play" | "next" | "shuffle" | "seek" => {
            app.emit_to(
                "main",
                OVERLAY_ACTION_EVENT,
                json!({ "action": action, "value": value }),
            )
            .map_err(|error| error.to_string())?;
        }
        _ => return Err("overlay action is invalid".to_string()),
    }
    Ok(())
}

fn require_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("taskbar docking is restricted to the main window".to_string())
    }
}

fn require_overlay(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == OVERLAY_LABEL {
        Ok(())
    } else {
        Err("taskbar overlay command is restricted to the overlay window".to_string())
    }
}

fn should_publish_refreshed_status(
    previous: &TaskbarDockStatus,
    current: &TaskbarDockStatus,
    published_intermediate_status: bool,
) -> bool {
    // A reattachment can begin and end Docked while publishing Waiting in
    // between. Always finish that transition for the renderer.
    previous != current || published_intermediate_status
}

fn status(inner: &Inner) -> TaskbarDockStatus {
    TaskbarDockStatus {
        mode: inner.mode.into(),
        suspended: inner.suspended,
        dock_error: inner.dock_error.clone(),
        enabled: inner.enabled,
    }
}

fn hide_taskbar_window(window: &WebviewWindow) {
    // Keep Tao's cached visibility in sync, then hide the actual HWND too.
    // Native no-activate reopen bypasses that cache, so another hide(false)
    // can otherwise be a no-op after the first close/fullscreen cycle.
    let _ = window.hide();
    if let Ok(hwnd) = native_hwnd(window) {
        // SAFETY: this handle comes from an app-owned WebviewWindow.
        // SW_HIDE neither activates a window nor changes its saved frame.
        unsafe { ShowWindow(hwnd as usize as *mut std::ffi::c_void, 0) };
    }
}

fn hide_main_window(window: &WebviewWindow) {
    hide_taskbar_window(window);
}

fn show_main_inactive(window: &WebviewWindow) {
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    let _ = window.show();
    if let Ok(hwnd) = native_hwnd(window) {
        // SAFETY: this is the app-owned main HWND; SW_SHOWNOACTIVATE does not
        // change its parent, style, or saved geometry.
        unsafe { ShowWindow(hwnd as usize as *mut std::ffi::c_void, 4) };
    }
}

/// The overlay's visibility is driven only through Win32. Tao's show/hide is
/// queued to the event loop and rewrites the window style from its cached
/// top-level flags; landing after attachment, that puts a caption on a taskbar
/// child, which then stays painted in Explorer's surface.
fn hide_overlay_window(window: &WebviewWindow) {
    if let Ok(hwnd) = native_hwnd(window) {
        // SAFETY: this handle comes from the app-owned overlay WebviewWindow.
        // SW_HIDE neither activates a window nor changes its saved frame.
        unsafe { ShowWindow(hwnd as usize as *mut std::ffi::c_void, 0) };
    }
}

fn show_overlay_on_top(hwnd: u64) {
    const HWND_TOPMOST: isize = -1;
    const FLAGS: u32 = 0x0001 | 0x0002 | 0x0010 | 0x0040;
    // SAFETY: callers pass the detached, app-owned overlay HWND. The flags keep
    // its size and position, avoid activation and only raise and show it.
    unsafe {
        SetWindowPos(
            hwnd as usize as *mut std::ffi::c_void,
            HWND_TOPMOST as *mut std::ffi::c_void,
            0,
            0,
            0,
            0,
            FLAGS,
        )
    };
}

/// Pace flight frames with DWM composition, falling back to a short sleep
/// when composition does not block.
fn wait_for_next_frame() {
    let started = Instant::now();
    // SAFETY: DwmFlush takes no arguments and only blocks until the next present.
    let flushed = unsafe { DwmFlush() } >= 0;
    if !flushed || started.elapsed() < Duration::from_millis(2) {
        thread::sleep(Duration::from_millis(6));
    }
}

/// GetAsyncKeyState reports physical buttons, so honor swapped-button setups.
fn primary_button_pressed() -> bool {
    const SM_SWAPBUTTON: i32 = 23;
    const VK_LBUTTON: i32 = 0x01;
    const VK_RBUTTON: i32 = 0x02;
    // SAFETY: both calls only read global input state.
    let swapped = unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0;
    let key = if swapped { VK_RBUTTON } else { VK_LBUTTON };
    (unsafe { GetAsyncKeyState(key) } as u16 & 0x8000) != 0
}

/// A cheap signature for the foreground window. Full taskbar enumeration is
/// only needed when this context changes or cannot be read.
fn foreground_context() -> Option<model::ForegroundContext> {
    // SAFETY: GetForegroundWindow and GetWindowRect only read desktop/window
    // state; the returned handle is checked by GetWindowRect before use.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut bounds = Win32Rect::default();
    if unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
        return None;
    }
    let bounds = Rect {
        x: bounds.left,
        y: bounds.top,
        width: bounds.right.checked_sub(bounds.left)?,
        height: bounds.bottom.checked_sub(bounds.top)?,
    };
    bounds.is_valid().then_some(model::ForegroundContext {
        hwnd: hwnd as usize as u64,
        bounds,
    })
}

/// The visible client area of a window in physical screen pixels. The main
/// window's outer rectangle also contains invisible resize borders.
fn native_client_screen_rect(hwnd: u64) -> Option<NativeRect> {
    let handle = hwnd as usize as *mut std::ffi::c_void;
    let mut client = Win32Rect::default();
    let mut origin = Win32Point::default();
    // SAFETY: `hwnd` is an app-owned window and both out-parameters point to
    // writable structs with the Win32 layout.
    if unsafe { GetClientRect(handle, &mut client) } == 0
        || unsafe { ClientToScreen(handle, &mut origin) } == 0
    {
        return None;
    }
    let rect = NativeRect {
        x: origin.x,
        y: origin.y,
        width: client.right.checked_sub(client.left)?,
        height: client.bottom.checked_sub(client.top)?,
    };
    rect.into_rect().is_valid().then_some(rect)
}

fn client_screen_rect(window: &WebviewWindow) -> Result<NativeRect, String> {
    native_client_screen_rect(native_hwnd(window)?)
        .ok_or_else(|| "main window client geometry is unavailable".to_string())
}

/// A visible, normal compact player, read without a round trip to the UI
/// thread so it also works inside a move or size loop.
fn native_floating_compact(hwnd: u64) -> bool {
    let handle = hwnd as usize as *mut std::ffi::c_void;
    // SAFETY: these calls only read state of the app-owned main HWND.
    let (zoomed, iconic, dpi) =
        unsafe { (IsZoomed(handle), IsIconic(handle), GetDpiForWindow(handle)) };
    if zoomed != 0 || iconic != 0 || !(48..=768).contains(&dpi) {
        return false;
    }
    let Some(rect) = native_client_screen_rect(hwnd) else {
        return false;
    };
    let Ok((width, height)) = logical_client_size(
        rect.width.unsigned_abs(),
        rect.height.unsigned_abs(),
        f64::from(dpi) / 96.0,
    ) else {
        return false;
    };
    model::compact_mode_is_valid(true, width, height)
}

fn set_native_window_rect(hwnd: u64, rect: NativeRect, show: bool) -> Result<(), String> {
    if !rect.into_rect().is_valid() {
        return Err("overlay window geometry is invalid".to_string());
    }
    let flags = 0x0004 | 0x0010 | if show { 0x0040 } else { 0 };
    // SAFETY: callers pass the app-owned taskbar overlay HWND and validated
    // physical-pixel dimensions; flags preserve z-order and activation.
    if unsafe {
        SetWindowPos(
            hwnd as usize as *mut std::ffi::c_void,
            std::ptr::null_mut(),
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            flags,
        )
    } == 0
    {
        return Err(format!(
            "could not position taskbar overlay: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn fail_closed(inner: &mut Inner, main: &WebviewWindow, overlay: &WebviewWindow, message: &str) {
    inner.mode = DockMode::Waiting;
    inner.suspended = true;
    inner.dock_error = message.chars().take(240).collect();
    hide_main_window(main);
    hide_overlay_window(overlay);
}

fn bar_for_window(bars: &[TaskbarObservation], bounds: NativeRect) -> Option<&TaskbarObservation> {
    if !bounds.into_rect().is_valid() {
        return None;
    }
    let center_x = i64::from(bounds.x) + i64::from(bounds.width) / 2;
    let center_y = i64::from(bounds.y) + i64::from(bounds.height) / 2;
    bars.iter().find(|bar| {
        let monitor: Rect = bar.monitor.into();
        bar.handle.parse::<i64>().is_ok_and(|handle| handle != 0)
            && monitor.is_valid()
            && center_x >= i64::from(monitor.x)
            && center_x < i64::from(monitor.x) + i64::from(monitor.width)
            && center_y >= i64::from(monitor.y)
            && center_y < i64::from(monitor.y) + i64::from(monitor.height)
    })
}

impl NativeRect {
    fn into_rect(self) -> Rect {
        self.into()
    }
}

fn parse_recovery_frame(value: Value, hwnd: u64) -> Result<RecoveryFrame, String> {
    let frame: RecoveryFrame = serde_json::from_value(value)
        .map_err(|_| "native inspection data is invalid".to_string())?;
    if frame.hwnd != hwnd.to_string()
        || frame.parent.len() > 32
        || frame.style.len() > 32
        || frame.ex_style.len() > 32
        || !frame.rect.into_rect().is_valid()
    {
        return Err("native recovery frame is outside the supported range".to_string());
    }
    Ok(frame)
}

fn recovery_frame_with_resizable(
    mut frame: RecoveryFrame,
    resizable: bool,
) -> Result<RecoveryFrame, String> {
    // A replacement HWND may inherit Tao's temporary dock resize flag. Keep
    // its own frame and all other native bits, restoring only the saved flag.
    const WS_THICKFRAME: i64 = 0x0004_0000;
    let style = frame
        .style
        .parse::<i64>()
        .map_err(|_| "replacement native style is invalid".to_string())?;
    frame.style = if resizable {
        style | WS_THICKFRAME
    } else {
        style & !WS_THICKFRAME
    }
    .to_string();
    Ok(frame)
}

fn recovery_frames_match(expected: &RecoveryFrame, actual: &RecoveryFrame) -> bool {
    expected == actual
}

fn hidden_recovery_frame(frame: &RecoveryFrame) -> Result<RecoveryFrame, String> {
    const WS_VISIBLE: i64 = 0x1000_0000;
    let style = frame
        .style
        .parse::<i64>()
        .map_err(|_| "saved native style is invalid".to_string())?;
    let mut hidden = frame.clone();
    hidden.style = (style & !WS_VISIBLE).to_string();
    Ok(hidden)
}

fn exit_recovery_frame_matches(expected: &RecoveryFrame, actual: &RecoveryFrame) -> bool {
    const WS_VISIBLE: i64 = 0x1000_0000;
    let (Ok(expected_style), Ok(actual_style)) =
        (expected.style.parse::<i64>(), actual.style.parse::<i64>())
    else {
        return false;
    };

    expected.hwnd == actual.hwnd
        && expected.parent == actual.parent
        && (expected_style & !WS_VISIBLE) == (actual_style & !WS_VISIBLE)
        && expected.ex_style == actual.ex_style
        && expected.rect == actual.rect
}

fn logical_inner_size(window: &WebviewWindow) -> Result<(u32, u32), String> {
    let (width, height) = native_client_size(window)?;
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    logical_client_size(width, height, scale)
}

fn logical_client_size(width: u32, height: u32, scale: f64) -> Result<(u32, u32), String> {
    if !scale.is_finite() || !(0.5..=8.0).contains(&scale) {
        return Err("main window scale factor is invalid".to_string());
    }
    let width = (f64::from(width) / scale).round();
    let height = (f64::from(height) / scale).round();
    if width < 0.0 || height < 0.0 || width > 100_000.0 || height > 100_000.0 {
        return Err("main window size is outside the supported range".to_string());
    }
    Ok((width as u32, height as u32))
}

#[cfg(target_os = "windows")]
pub(crate) fn correct_client_size_after_restore(
    window: &WebviewWindow,
    logical_width: f64,
    logical_height: f64,
) -> Result<(), String> {
    if !logical_width.is_finite()
        || !logical_height.is_finite()
        || logical_width <= 0.0
        || logical_height <= 0.0
        || logical_width > 100_000.0
        || logical_height > 100_000.0
    {
        return Err("requested main window client size is outside the supported range".to_string());
    }
    let scale = window.scale_factor().map_err(|error| error.to_string())?;
    if !scale.is_finite() || !(0.5..=8.0).contains(&scale) {
        return Err("main window scale factor is invalid".to_string());
    }
    let desired_width = physical_client_dimension(logical_width, scale)?;
    let desired_height = physical_client_dimension(logical_height, scale)?;
    // Complete queued fullscreen/maximize changes before measuring the HWND.
    let outer = window.outer_size().map_err(|error| error.to_string())?;
    let (actual_width, actual_height) = native_client_size(window)?;
    if (actual_width, actual_height) == (desired_width, desired_height) {
        return Ok(());
    }
    let target_outer_width = corrected_outer_dimension(outer.width, actual_width, desired_width)?;
    let target_outer_height =
        corrected_outer_dimension(outer.height, actual_height, desired_height)?;
    let hwnd = native_hwnd(window)?;
    // SAFETY: `native_hwnd` returns the live main HWND. SWP_NOMOVE, SWP_NOZORDER,
    // and SWP_NOACTIVATE keep this correction limited to its outer dimensions.
    if unsafe {
        SetWindowPos(
            hwnd as usize as *mut std::ffi::c_void,
            std::ptr::null_mut(),
            0,
            0,
            i32::try_from(target_outer_width)
                .map_err(|_| "corrected main window width is invalid".to_string())?,
            i32::try_from(target_outer_height)
                .map_err(|_| "corrected main window height is invalid".to_string())?,
            0x02 | 0x04 | 0x10,
        )
    } == 0
    {
        return Err(format!(
            "could not correct main window client size: {}",
            io::Error::last_os_error()
        ));
    }

    let verified = native_client_size(window)?;
    if verified != (desired_width, desired_height) {
        return Err("main window client size did not match the requested restore size".to_string());
    }
    Ok(())
}

fn physical_client_dimension(logical: f64, scale: f64) -> Result<u32, String> {
    let physical = (logical * scale).round();
    if !physical.is_finite() || !(1.0..=65_535.0).contains(&physical) {
        return Err("requested main window client size is outside the supported range".to_string());
    }
    Ok(physical as u32)
}

fn corrected_outer_dimension(
    outer: u32,
    actual_client: u32,
    desired_client: u32,
) -> Result<u32, String> {
    if !(1..=100_000).contains(&outer)
        || !(1..=outer).contains(&actual_client)
        || !(1..=100_000).contains(&desired_client)
    {
        return Err("main window size correction input is outside the supported range".to_string());
    }
    let corrected = i64::from(outer) + i64::from(desired_client) - i64::from(actual_client);
    if !(1..=100_000).contains(&corrected) {
        return Err("corrected main window outer size is outside the supported range".to_string());
    }
    u32::try_from(corrected)
        .map_err(|_| "corrected main window outer size is outside the supported range".to_string())
}

#[cfg(target_os = "windows")]
fn native_client_size(window: &WebviewWindow) -> Result<(u32, u32), String> {
    let hwnd = native_hwnd(window)?;
    let mut rect = Win32Rect::default();
    // SAFETY: `native_hwnd` returns this live window's HWND, and `rect` points
    // to a writable RECT with the Win32 layout required by GetClientRect.
    if unsafe {
        GetClientRect(
            hwnd as usize as *mut std::ffi::c_void,
            &mut rect as *mut Win32Rect,
        )
    } == 0
    {
        return Err(format!(
            "could not read main window client geometry: {}",
            io::Error::last_os_error()
        ));
    }
    let width = rect
        .right
        .checked_sub(rect.left)
        .ok_or_else(|| "main window client geometry is invalid".to_string())?;
    let height = rect
        .bottom
        .checked_sub(rect.top)
        .ok_or_else(|| "main window client geometry is invalid".to_string())?;
    if width < 0 || height < 0 || width > 100_000 || height > 100_000 {
        return Err("main window client geometry is outside the supported range".to_string());
    }
    Ok((width as u32, height as u32))
}

#[cfg(not(target_os = "windows"))]
fn native_client_size(window: &WebviewWindow) -> Result<(u32, u32), String> {
    let size = window.inner_size().map_err(|error| error.to_string())?;
    Ok((size.width, size.height))
}

fn window_is_live_compact(window: &WebviewWindow) -> Result<bool, String> {
    let (width, height) = logical_inner_size(window)?;
    let maximized = window.is_maximized().map_err(|error| error.to_string())?;
    let fullscreen = window.is_fullscreen().map_err(|error| error.to_string())?;
    Ok(model::live_compact_mode_is_valid(
        width, height, maximized, fullscreen,
    ))
}

fn window_rect(window: &WebviewWindow) -> Result<NativeRect, String> {
    let position = window.outer_position().map_err(|error| error.to_string())?;
    let size = window.outer_size().map_err(|error| error.to_string())?;
    if size.width == 0 || size.height == 0 || size.width > 100_000 || size.height > 100_000 {
        return Err("main window geometry is invalid".to_string());
    }
    Ok(NativeRect {
        x: position.x,
        y: position.y,
        width: i32::try_from(size.width).map_err(|_| "main window width is invalid")?,
        height: i32::try_from(size.height).map_err(|_| "main window height is invalid")?,
    })
}

fn selected_taskbar_is_safe(probe: &ProbeResult, handle: &str) -> bool {
    probe.complete
        && probe.bars.iter().any(|bar| {
            bar.handle == handle
                && bar.monitor.into_rect().is_valid()
                && !bar.suspended
                && bar.fullscreen == Some(false)
        })
}

fn overlay_animation_duration(snapshot: Option<&Value>) -> Duration {
    if snapshot
        .and_then(|snapshot| snapshot.get("reducedMotion"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        REDUCED_MOTION_DURATION
    } else {
        FULL_MOTION_DURATION
    }
}

fn native_hwnd(window: &WebviewWindow) -> Result<u64, String> {
    let handle = window.hwnd().map_err(|error| error.to_string())?;
    let value = handle.0 as usize as u64;
    if value == 0 {
        return Err("main window handle is null".to_string());
    }
    Ok(value)
}

fn is_recoverable_native_error(error: &str) -> bool {
    error.contains("helper stopped")
        || error.contains("invalid taskbar attachment geometry")
        || error.contains("main window handle is no longer valid")
}

fn format_native_error(error: &NativeError) -> String {
    match error.code {
        Some(87) => "invalid taskbar attachment geometry".to_string(),
        Some(1400) => "main window handle is no longer valid".to_string(),
        _ if error.session_failed => "native taskbar helper stopped unexpectedly".to_string(),
        _ => "native taskbar operation failed".to_string(),
    }
}

fn load_preference(app: &AppHandle) -> Result<bool, String> {
    let path = preference_path(app)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > PREF_MAX_BYTES {
        return Err("preference file is not a small regular file".to_string());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > PREF_MAX_BYTES {
        return Err("preference file is too large".to_string());
    }
    let value: DockingPreference =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if value.version != 1 {
        return Err("preference version is unsupported".to_string());
    }
    Ok(value.enabled)
}

fn persist_preference(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let path = preference_path(app)?;
    let directory = path
        .parent()
        .ok_or_else(|| "preference path has no parent".to_string())?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&DockingPreference {
        version: 1,
        enabled,
    })
    .map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    temporary
        .write_all(&bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| error.error.to_string())?;
    Ok(())
}

fn preference_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|path| path.join(PREF_FILE))
        .map_err(|error| error.to_string())
}

impl NativeHelper {
    fn has_live_session(&mut self, hwnd: u64) -> bool {
        self.session.as_mut().is_some_and(|session| {
            session.hwnd == hwnd && matches!(session.child.try_wait(), Ok(None))
        })
    }

    fn request(
        &mut self,
        app: &AppHandle,
        hwnd: u64,
        op: &str,
        args: Value,
    ) -> Result<Value, NativeError> {
        let dead = self.session.as_mut().is_some_and(|session| {
            session.hwnd != hwnd || !matches!(session.child.try_wait(), Ok(None))
        });
        if dead {
            self.close();
        }
        if self.session.is_none() {
            let path = match self.path(app) {
                Ok(path) => path,
                Err(_) => {
                    return Err(NativeError {
                        code: None,
                        session_failed: true,
                    })
                }
            };
            self.session = Some(spawn_helper(path, hwnd).map_err(|_| NativeError {
                code: None,
                session_failed: true,
            })?);
        }
        let result = self.session.as_mut().unwrap().request(op, args);
        if result.as_ref().is_err_and(|error| error.session_failed) {
            self.close();
        }
        result
    }

    fn path(&mut self, app: &AppHandle) -> Result<PathBuf, String> {
        if let Some(path) = &self.helper_path {
            return Ok(path.clone());
        }
        if let Ok(path) = app
            .path()
            .resolve("windows-taskbar-native.exe", BaseDirectory::Resource)
        {
            if path.is_file() {
                self.helper_path = Some(path.clone());
                return Ok(path);
            }
        }
        #[cfg(debug_assertions)]
        {
            let path = PathBuf::from(env!("TASKBAR_NATIVE_HELPER_PATH"));
            if path.is_file() {
                self.helper_path = Some(path.clone());
                return Ok(path);
            }
        }
        Err("native taskbar helper is missing from the application resources".to_string())
    }

    fn close(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        drop(session.stdin);
        reap_helper(session.child, Duration::from_secs(2));
    }

    fn close_nonblocking(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        // EOF runs the helper's finally/Restore path. Do not wait here: this
        // can be called from Tauri's exit event and SetParent may need the UI
        // thread that is currently dispatching that event.
        drop(session.stdin);
        thread::spawn(move || {
            reap_helper(session.child, Duration::from_secs(2));
        });
    }

    fn reset(&mut self) {
        self.close();
    }
}

impl Drop for NativeHelper {
    fn drop(&mut self) {
        self.close_nonblocking();
    }
}

fn reap_helper(mut child: Child, grace: Duration) {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) if start.elapsed() < grace => thread::sleep(Duration::from_millis(50)),
            _ => {
                // A timed-out helper must not outlive a replacement helper and
                // later reapply its stale saved frame from its EOF cleanup.
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
        }
    }
}

fn spawn_helper(path: PathBuf, hwnd: u64) -> Result<HelperLine, String> {
    let mut command = Command::new(path);
    command
        .arg(std::process::id().to_string())
        .arg(hwnd.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "native helper stdin is unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "native helper stdout is unavailable".to_string())?;
    if let Some(mut stderr) = child.stderr.take() {
        thread::spawn(move || {
            let _ = io::copy(&mut stderr, &mut io::sink());
        });
    }
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(length) if length <= 65_536 => {
                    if sender.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(_) => {
                    let _ =
                        sender.send(Err("native helper response exceeded size limit".to_string()));
                    break;
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    break;
                }
            }
        }
    });
    Ok(HelperLine {
        child,
        stdin,
        receiver,
        hwnd,
        sequence: 0,
    })
}

impl HelperLine {
    fn request(&mut self, op: &str, args: Value) -> Result<Value, NativeError> {
        if !matches!(self.child.try_wait(), Ok(None)) {
            return Err(NativeError {
                code: None,
                session_failed: true,
            });
        }
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let id = self.sequence;
        let mut request = serde_json::Map::new();
        request.insert("id".to_string(), Value::from(id));
        request.insert("op".to_string(), Value::String(op.to_string()));
        if let Some(object) = args.as_object() {
            for (key, value) in object {
                if key != "id" && key != "op" {
                    request.insert(key.clone(), value.clone());
                }
            }
        }
        let mut bytes = serde_json::to_vec(&Value::Object(request)).map_err(|_| NativeError {
            code: None,
            session_failed: false,
        })?;
        if bytes.len() > 4_000 {
            return Err(NativeError {
                code: None,
                session_failed: false,
            });
        }
        bytes.push(b'\n');
        if self
            .stdin
            .write_all(&bytes)
            .and_then(|_| self.stdin.flush())
            .is_err()
        {
            return Err(NativeError {
                code: None,
                session_failed: true,
            });
        }
        let start = Instant::now();
        loop {
            let remaining = HELPER_TIMEOUT.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err(NativeError {
                    code: None,
                    session_failed: true,
                });
            }
            let line = self
                .receiver
                .recv_timeout(remaining)
                .map_err(|_| NativeError {
                    code: None,
                    session_failed: true,
                })?
                .map_err(|_| NativeError {
                    code: None,
                    session_failed: true,
                })?;
            if line.len() > 65_536 {
                return Err(NativeError {
                    code: None,
                    session_failed: true,
                });
            }
            let reply: HelperReply = serde_json::from_str(&line).map_err(|_| NativeError {
                code: None,
                session_failed: true,
            })?;
            if reply.id != id {
                continue;
            }
            if reply.error.is_some() {
                return Err(NativeError {
                    code: reply.code,
                    session_failed: false,
                });
            }
            return reply.result.ok_or(NativeError {
                code: None,
                session_failed: false,
            });
        }
    }
}

#[cfg(test)]
fn is_recoverable_native_error_value(error: &NativeError) -> bool {
    error.session_failed || matches!(error.code, Some(87 | 1400))
}

#[cfg(test)]
mod tests {
    use super::{
        overlay_animation_duration, selected_taskbar_is_safe, should_publish_refreshed_status,
        DockModePayload, ProbeResult, TaskbarDockStatus, TaskbarObservation, FULL_MOTION_DURATION,
        REDUCED_MOTION_DURATION,
    };

    #[test]
    fn helper_reattachment_publishes_docked_after_an_intermediate_waiting_event() {
        let docked = TaskbarDockStatus {
            mode: DockModePayload::Docked,
            suspended: false,
            dock_error: String::new(),
            enabled: true,
        };
        assert!(should_publish_refreshed_status(&docked, &docked, true));
        assert!(!should_publish_refreshed_status(&docked, &docked, false));
        let waiting = TaskbarDockStatus {
            mode: DockModePayload::Waiting,
            ..docked.clone()
        };
        assert!(should_publish_refreshed_status(&docked, &waiting, false));
    }

    use super::{
        corrected_outer_dimension, exit_recovery_frame_matches, hidden_recovery_frame,
        is_recoverable_native_error_value, logical_client_size, parse_recovery_frame,
        physical_client_dimension, recovery_frame_with_resizable, recovery_frames_match,
        NativeError, NativeRect, RecoveryFrame,
    };
    use serde_json::json;

    #[test]
    fn invalid_hwnd_and_helper_exit_errors_are_classified_as_recoverable() {
        assert!(is_recoverable_native_error_value(&NativeError {
            code: Some(87),
            session_failed: false,
        }));
        assert!(is_recoverable_native_error_value(&NativeError {
            code: Some(1400),
            session_failed: false,
        }));
        assert!(is_recoverable_native_error_value(&NativeError {
            code: None,
            session_failed: true,
        }));
        assert!(!is_recoverable_native_error_value(&NativeError {
            code: None,
            session_failed: false,
        }));
    }

    #[test]
    fn parses_native_inspection_fixture_for_the_requested_window() {
        let frame = parse_recovery_frame(
            json!({
                "hwnd": "62787600",
                "parent": "0",
                "rect": {"x": 1000, "y": 979, "width": 649, "height": 80},
                "visible": false,
                "style": "134217728",
                "exStyle": "0",
                "awareness": 2
            }),
            62_787_600,
        )
        .expect("the native inspect response should produce a recovery frame");

        assert_eq!(
            frame,
            RecoveryFrame {
                hwnd: "62787600".to_string(),
                parent: "0".to_string(),
                style: "134217728".to_string(),
                ex_style: "0".to_string(),
                rect: NativeRect {
                    x: 1000,
                    y: 979,
                    width: 649,
                    height: 80,
                },
            }
        );
    }

    #[test]
    fn rejects_native_inspection_frame_for_a_different_window() {
        let result = parse_recovery_frame(
            json!({
                "hwnd": "62787601",
                "parent": "0",
                "rect": {"x": 1000, "y": 979, "width": 649, "height": 80},
                "visible": false,
                "style": "134217728",
                "exStyle": "0",
                "awareness": 2
            }),
            62_787_600,
        );

        assert_eq!(
            result.unwrap_err(),
            "native recovery frame is outside the supported range"
        );
    }

    #[test]
    fn logical_window_size_uses_the_physical_client_rect_and_dpi_scale() {
        assert_eq!(logical_client_size(900, 350, 1.0).unwrap(), (900, 350));
        assert_eq!(logical_client_size(900, 525, 1.5).unwrap(), (600, 350));
    }

    #[test]
    fn client_size_correction_preserves_the_window_frame_extent() {
        assert_eq!(corrected_outer_dimension(916, 900, 900).unwrap(), 916);
        assert_eq!(corrected_outer_dimension(178, 170, 150).unwrap(), 158);
        assert_eq!(physical_client_dimension(900.0, 1.5).unwrap(), 1350);
    }

    #[test]
    fn client_size_correction_rejects_invalid_and_overflowing_dimensions() {
        assert!(corrected_outer_dimension(178, 170, 0).is_err());
        assert!(corrected_outer_dimension(100_000, 0, 100_001).is_err());
        assert!(physical_client_dimension(100_000.0, 8.0).is_err());
        assert!(physical_client_dimension(f64::INFINITY, 1.0).is_err());
    }

    fn recovery_frame(style: &str) -> RecoveryFrame {
        RecoveryFrame {
            hwnd: "62787600".to_string(),
            parent: "0".to_string(),
            style: style.to_string(),
            ex_style: "0".to_string(),
            rect: NativeRect {
                x: 1000,
                y: 979,
                width: 649,
                height: 80,
            },
        }
    }

    fn taskbar_observation(
        handle: &str,
        fullscreen: Option<bool>,
        suspended: bool,
        monitor_x: i32,
    ) -> TaskbarObservation {
        TaskbarObservation {
            handle: handle.to_string(),
            taskbar: NativeRect {
                x: monitor_x,
                y: 1040,
                width: 1920,
                height: 40,
            },
            monitor: NativeRect {
                x: monitor_x,
                y: 0,
                width: 1920,
                height: 1080,
            },
            free: None,
            dpi: 96,
            suspended,
            fullscreen,
        }
    }

    #[test]
    fn animation_safety_uses_the_selected_monitor_and_fails_closed_on_unknown_data() {
        let probe = ProbeResult {
            complete: true,
            bars: vec![
                taskbar_observation("primary", Some(false), false, 0),
                taskbar_observation("secondary", Some(true), false, 1920),
            ],
        };

        assert!(selected_taskbar_is_safe(&probe, "primary"));
        assert!(!selected_taskbar_is_safe(&probe, "secondary"));
        assert!(!selected_taskbar_is_safe(&probe, "missing"));

        let incomplete = ProbeResult {
            complete: false,
            bars: vec![taskbar_observation("primary", Some(false), false, 0)],
        };
        assert!(!selected_taskbar_is_safe(&incomplete, "primary"));

        let unknown = ProbeResult {
            complete: true,
            bars: vec![taskbar_observation("primary", None, false, 0)],
        };
        assert!(!selected_taskbar_is_safe(&unknown, "primary"));
    }

    #[test]
    fn reduced_motion_uses_the_same_120ms_duration_as_the_overlay_fade() {
        let reduced = json!({"reducedMotion":true});
        let full = json!({"reducedMotion":false});

        assert_eq!(
            overlay_animation_duration(Some(&reduced)),
            REDUCED_MOTION_DURATION
        );
        assert_eq!(
            overlay_animation_duration(Some(&full)),
            FULL_MOTION_DURATION
        );
    }

    #[test]
    fn replacement_resize_recovery_preserves_the_new_hwnd_frame_and_unrelated_style_bits() {
        for (style, resizable, expected_style) in [
            ("268435488", true, "268697632"),
            ("268697632", false, "268435488"),
        ] {
            let original = recovery_frame(style);
            let mut expected = original.clone();
            expected.style = expected_style.to_string();
            assert_eq!(
                recovery_frame_with_resizable(original, resizable).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn replacement_resize_recovery_rejects_an_unreadable_style() {
        assert!(recovery_frame_with_resizable(recovery_frame("invalid"), true).is_err());
    }

    #[test]
    fn exit_frame_check_allows_only_the_requested_visibility_change() {
        let expected = recovery_frame("268435456");
        let actual = recovery_frame("0");

        assert!(exit_recovery_frame_matches(&expected, &actual));
    }

    #[test]
    fn exit_frame_check_rejects_other_style_and_frame_changes() {
        let expected = recovery_frame("268435456");

        assert!(!exit_recovery_frame_matches(
            &expected,
            &recovery_frame("268435488")
        ));

        let mut changed_rect = recovery_frame("0");
        changed_rect.rect.x += 1;
        assert!(!exit_recovery_frame_matches(&expected, &changed_rect));

        let mut changed_parent = recovery_frame("0");
        changed_parent.parent = "65552".to_string();
        assert!(!exit_recovery_frame_matches(&expected, &changed_parent));

        let mut changed_ex_style = recovery_frame("0");
        changed_ex_style.ex_style = "1".to_string();
        assert!(!exit_recovery_frame_matches(&expected, &changed_ex_style));

        let mut changed_hwnd = recovery_frame("0");
        changed_hwnd.hwnd = "62787601".to_string();
        assert!(!exit_recovery_frame_matches(&expected, &changed_hwnd));
    }

    #[test]
    fn exit_frame_check_rejects_unparseable_style_values() {
        assert!(!exit_recovery_frame_matches(
            &recovery_frame("not-a-style"),
            &recovery_frame("0")
        ));
        assert!(!exit_recovery_frame_matches(
            &recovery_frame("268435456"),
            &recovery_frame("not-a-style")
        ));
    }

    #[test]
    fn ordinary_recovery_frame_check_rejects_a_twenty_pixel_height_change() {
        let expected = recovery_frame("268435456");
        let mut actual = expected.clone();
        actual.rect.height += 20;

        assert!(!recovery_frames_match(&expected, &actual));
    }

    #[test]
    fn hidden_recovery_frame_clears_only_ws_visible_without_changing_the_saved_frame() {
        let saved = recovery_frame("268435488");
        let hidden = hidden_recovery_frame(&saved).unwrap();
        let mut expected_hidden = saved.clone();
        expected_hidden.style = "32".to_string();

        assert_eq!(hidden, expected_hidden);
        assert_eq!(saved.style, "268435488");
        assert!(!recovery_frames_match(&saved, &hidden));
    }

    #[test]
    fn hidden_recovery_frame_rejects_unparseable_style_values() {
        assert!(hidden_recovery_frame(&recovery_frame("not-a-style")).is_err());
    }
}
