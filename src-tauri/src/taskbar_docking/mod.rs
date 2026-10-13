//! Windows compact-player taskbar docking.
//!
//! The player window is never reparented or restyled: docking hides it and
//! attaches a dedicated, transparent `taskbar-lyrics` overlay to Explorer's
//! taskbar through a native helper. The overlay boots a lightweight page that
//! mirrors playback from snapshots the player publishes, and sends playback
//! actions back to that one player.
//!
//! Window callbacks run on the UI thread and only touch atomics: the worker
//! holding `state` may itself be waiting for the UI thread.

mod helper;
mod win32;

use std::{
    fs,
    ops::Deref,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow, WindowEvent};

use self::helper::Helper;
use crate::taskbar_docking_model::{self as model, DragHint, OverlaySnapshot, Rect};

pub use self::win32::set_player_client_size;

const MAIN_LABEL: &str = "main";
pub const OVERLAY_LABEL: &str = "taskbar-lyrics";
const HINT_EVENT: &str = "desktop://taskbarDockHint";
const SNAPSHOT_EVENT: &str = "desktop://taskbarOverlaySnapshot";
const VISUAL_EVENT: &str = "desktop://taskbarOverlayVisual";
const ACTION_EVENT: &str = "desktop://taskbarOverlayAction";
const PREFERENCE_FILE: &str = "taskbar-docking.json";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const DRAG_SAMPLE_INTERVAL: Duration = Duration::from_millis(16);
const DRAG_PROBE_INTERVAL: Duration = Duration::from_millis(600);
const PAINT_TIMEOUT: Duration = Duration::from_secs(2);
const FLIGHT_DURATION: Duration = Duration::from_millis(240);
const REDUCED_FLIGHT_DURATION: Duration = Duration::from_millis(120);

type OverlayFactory = dyn Fn(&AppHandle) -> tauri::Result<WebviewWindow> + Send + Sync;

/// What the overlay renders; every change gets a new token.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    Hidden,
    Snapping,
    Docked,
    Returning,
}

#[derive(Clone)]
pub struct TaskbarDockController(Arc<Shared>);

pub struct Shared {
    app: AppHandle,
    state: Mutex<State>,
    overlay_factory: OnceLock<Box<OverlayFactory>>,
    /// The user wants the player visible (tray, close and minimize clear it).
    visible: AtomicBool,
    /// Docking is enabled and the player is in compact mode.
    active: AtomicBool,
    dragging: AtomicBool,
    stopping: AtomicBool,
    /// The overlay page is listening for snapshots and phases.
    overlay_ready: AtomicBool,
    phase_token: AtomicU64,
    painted_token: AtomicU64,
    main_hwnd: AtomicU64,
}

struct State {
    /// Persisted dock intent, set by releasing a drag near the taskbar.
    enabled: bool,
    compact: bool,
    /// The taskbar the overlay is currently a child of.
    attached_bar: Option<String>,
    phase: Phase,
    snapshot: Option<OverlaySnapshot>,
    helper: Helper,
}

impl Deref for TaskbarDockController {
    type Target = Shared;

    fn deref(&self) -> &Shared {
        &self.0
    }
}

struct Flight {
    phase: Phase,
    from: Rect,
    to: Rect,
}

impl TaskbarDockController {
    pub fn new(app: AppHandle) -> Self {
        let enabled = load_preference(&app);
        Self(Arc::new(Shared {
            app,
            state: Mutex::new(State {
                enabled,
                compact: false,
                attached_bar: None,
                phase: Phase::Hidden,
                snapshot: None,
                helper: Helper::default(),
            }),
            overlay_factory: OnceLock::new(),
            visible: AtomicBool::new(false),
            active: AtomicBool::new(false),
            dragging: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            overlay_ready: AtomicBool::new(false),
            phase_token: AtomicU64::new(0),
            painted_token: AtomicU64::new(0),
            main_hwnd: AtomicU64::new(0),
        }))
    }

    /// Creates the overlay and starts polling the taskbar for `main`.
    pub fn start(
        &self,
        main: &WebviewWindow,
        overlay_factory: impl Fn(&AppHandle) -> tauri::Result<WebviewWindow> + Send + Sync + 'static,
    ) -> tauri::Result<()> {
        if let Ok(hwnd) = win32::hwnd(main) {
            self.main_hwnd.store(hwnd, Ordering::Release);
        }
        self.visible
            .store(main.is_visible().unwrap_or(false), Ordering::Release);
        let _ = self.overlay_factory.set(Box::new(overlay_factory));
        self.create_overlay()?;

        let controller = self.clone();
        thread::spawn(move || {
            while !controller.stopping.load(Ordering::Acquire) {
                thread::sleep(POLL_INTERVAL);
                controller.tick();
            }
        });
        Ok(())
    }

    fn create_overlay(&self) -> tauri::Result<()> {
        let Some(factory) = self.overlay_factory.get() else {
            return Ok(());
        };
        let overlay = factory(&self.app)?;
        let controller = self.clone();
        overlay.on_window_event(move |event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                controller.request_hide();
            }
            WindowEvent::Destroyed => controller.replace_overlay(),
            _ => {}
        });
        Ok(())
    }

    /// Rebuilds a destroyed overlay. The player stays hidden meanwhile when
    /// docking is active; the next poll docks the replacement.
    fn replace_overlay(&self) {
        if self.stopping.load(Ordering::Acquire) {
            return;
        }
        self.overlay_ready.store(false, Ordering::Release);
        let controller = self.clone();
        thread::spawn(move || {
            if let Ok(mut state) = controller.state.lock() {
                state.helper.close();
                state.attached_bar = None;
                controller.set_phase(&mut state, Phase::Hidden);
            }
            let mut delay = Duration::from_millis(100);
            while !controller.stopping.load(Ordering::Acquire)
                && controller.app.get_webview_window(MAIN_LABEL).is_some()
            {
                thread::sleep(delay);
                if controller.app.get_webview_window(OVERLAY_LABEL).is_some() {
                    return;
                }
                match controller.create_overlay() {
                    Ok(()) => return,
                    Err(error) => eprintln!("[taskbar-docking] overlay rebuild failed: {error}"),
                }
                delay = (delay * 2).min(Duration::from_secs(2));
            }
        });
    }

    /// Whether `show_main_window` should show the player itself; otherwise
    /// the poller reveals the docked strip without activating anything.
    pub fn request_show(&self) -> bool {
        self.visible.store(true, Ordering::Release);
        !self.active.load(Ordering::Acquire)
    }

    pub fn request_hide(&self) {
        self.visible.store(false, Ordering::Release);
        if let Some(main) = self.app.get_webview_window(MAIN_LABEL) {
            win32::hide_player(&main);
        }
        if let Some(hwnd) = self.overlay_hwnd() {
            win32::hide(hwnd);
        }
        if let Ok(mut state) = self.state.try_lock() {
            self.set_phase(&mut state, Phase::Hidden);
        }
    }

    pub fn is_docking_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        if let Ok(mut state) = self.state.try_lock() {
            state.helper.close_in_background();
        }
    }

    /// The renderer switched the player into or out of the compact bar.
    pub fn set_compact(&self, compact: bool) -> Result<(), String> {
        let main = self.main()?;
        if compact && !win32::is_floating_compact(win32::hwnd(&main)?) {
            return Err("the player is not a normal compact window".to_owned());
        }
        let mut state = self.lock()?;
        if compact {
            state.compact = true;
            self.update_active(&state);
            self.refresh(&mut state, &main);
            return Ok(());
        }
        // The player is hidden only while docking is active or attached.
        if state.attached_bar.is_some() || self.active.load(Ordering::Acquire) {
            self.return_to_player(&mut state, &main)?;
        }
        state.compact = false;
        self.update_active(&state);
        Ok(())
    }

    /// The strip's return button: fly back and stop docking.
    fn undock(&self) -> Result<(), String> {
        let main = self.main()?;
        let mut state = self.lock()?;
        self.return_to_player(&mut state, &main)?;
        self.set_enabled(&mut state, false);
        Ok(())
    }

    fn set_enabled(&self, state: &mut State, enabled: bool) {
        state.enabled = enabled;
        self.update_active(state);
        if let Err(error) = save_preference(&self.app, enabled) {
            eprintln!("[taskbar-docking] could not save the docking preference: {error}");
        }
    }

    fn update_active(&self, state: &State) {
        self.active
            .store(state.enabled && state.compact, Ordering::Release);
    }

    fn tick(&self) {
        let Ok(main) = self.main() else {
            return;
        };
        if self.overlay_hwnd().is_none() {
            // Keep the player hidden while a destroyed overlay is rebuilt.
            if self.active.load(Ordering::Acquire) {
                win32::hide_player(&main);
            }
            return;
        }
        if let Ok(mut state) = self.state.lock() {
            self.refresh(&mut state, &main);
        }
    }

    /// Converges the windows on the current intent and taskbar.
    fn refresh(&self, state: &mut State, main: &WebviewWindow) {
        let (Some(overlay), Ok(main_hwnd)) = (self.overlay_hwnd(), win32::hwnd(main)) else {
            return self.wait_hidden(main);
        };
        if state.attached_bar.is_some() && !state.helper.is_alive_for(overlay) {
            // Only the helper that attached the overlay can undo it.
            return self.discard_overlay(state, main);
        }
        let active = state.enabled && state.compact;
        if !active && state.attached_bar.is_none() {
            return;
        }
        if !active || !win32::is_floating_compact(main_hwnd) {
            self.detach(state, main);
            win32::hide(overlay);
            return self.reveal_player(main);
        }

        let Some(player) = win32::window_rect(main_hwnd) else {
            return self.wait_hidden(main);
        };
        let probe = match state.helper.probe(&self.app, overlay) {
            Ok(probe) if probe.complete => probe,
            _ => return self.wait_hidden(main),
        };
        // Fullscreen apps and unreadable foregrounds hide both windows while
        // keeping the intent, so docking resumes once they are gone.
        let Some(bar) = probe.bar_for(player).filter(|bar| bar.is_clear()) else {
            return self.wait_hidden(main);
        };
        let Some(target) = bar.dock_target() else {
            if state.attached_bar.is_some() {
                return self.wait_hidden(main);
            }
            win32::hide(overlay);
            return self.reveal_player(main);
        };
        // A move loop that still owns the pointer would queue our native calls
        // behind it; wait for the release.
        let pointer_busy = state.attached_bar.is_none() && win32::primary_button_pressed();
        if !self.overlay_ready.load(Ordering::Acquire) || state.snapshot.is_none() || pointer_busy {
            win32::hide(overlay);
            return self.reveal_player(main);
        }

        if state.attached_bar.is_none() && self.visible.load(Ordering::Acquire) {
            let landed = win32::client_rect(main_hwnd).is_some_and(|from| {
                let flight = Flight {
                    phase: Phase::Snapping,
                    from,
                    to: target,
                };
                self.fly(state, main, overlay, &flight)
            });
            if !landed {
                win32::hide(overlay);
                return self.reveal_player(main);
            }
        }
        if let Err(error) = self.attach(state, main, overlay, &bar.handle) {
            eprintln!("[taskbar-docking] taskbar attachment failed: {error}");
            self.detach(state, main);
            win32::hide(overlay);
            self.set_phase(state, Phase::Hidden);
            return self.reveal_player(main);
        }
        if state.phase != Phase::Docked {
            self.set_phase(state, Phase::Docked);
        }
        if !self.visible.load(Ordering::Acquire) {
            win32::hide(overlay);
            return win32::hide_player(main);
        }
        // "show" also strips any caption Tao reapplied to the child.
        if !win32::is_visible(overlay) {
            if let Err(error) = state.helper.request(&self.app, overlay, "show", json!({})) {
                eprintln!("[taskbar-docking] could not show the strip: {error}");
                self.wait_hidden(main);
            }
        }
    }

    fn attach(
        &self,
        state: &mut State,
        main: &WebviewWindow,
        overlay: u64,
        bar: &str,
    ) -> Result<(), String> {
        if state
            .attached_bar
            .as_deref()
            .is_some_and(|attached| attached != bar)
        {
            self.detach(state, main);
        }
        state
            .helper
            .request(&self.app, overlay, "attach", json!({ "bar": bar }))?;
        state.attached_bar = Some(bar.to_owned());
        Ok(())
    }

    /// Returns the overlay to a hidden top-level window.
    fn detach(&self, state: &mut State, main: &WebviewWindow) {
        if state.attached_bar.is_none() {
            return;
        }
        let detached = self.overlay_hwnd().is_some_and(|overlay| {
            state.helper.is_alive_for(overlay)
                && state
                    .helper
                    .request(&self.app, overlay, "detach", json!({}))
                    .is_ok()
        });
        state.attached_bar = None;
        match self.overlay_hwnd() {
            Some(overlay) if detached => win32::hide(overlay),
            _ => self.discard_overlay(state, main),
        }
    }

    /// Destroys an overlay whose attachment can no longer be undone; the
    /// replacement starts clean.
    fn discard_overlay(&self, state: &mut State, main: &WebviewWindow) {
        state.helper.close();
        state.attached_bar = None;
        self.set_phase(state, Phase::Hidden);
        if let Some(overlay) = self.app.get_webview_window(OVERLAY_LABEL) {
            let _ = overlay.destroy();
        }
        self.wait_hidden(main);
    }

    /// Flies the strip back to the player, detaches it and reveals the player.
    fn return_to_player(&self, state: &mut State, main: &WebviewWindow) -> Result<(), String> {
        let main_hwnd = win32::hwnd(main)?;
        let visible = self.visible.load(Ordering::Acquire);
        if visible && state.attached_bar.is_some() && !self.foreground_is_clear(state, main_hwnd) {
            self.wait_hidden(main);
            return Err("a fullscreen window is in front of the taskbar".to_owned());
        }
        let flight = match (self.overlay_hwnd(), &state.attached_bar) {
            (Some(overlay), Some(_)) if visible && self.overlay_ready.load(Ordering::Acquire) => {
                win32::window_rect(overlay)
                    .zip(win32::client_rect(main_hwnd))
                    .map(|(from, to)| Flight {
                        phase: Phase::Returning,
                        from,
                        to,
                    })
            }
            _ => None,
        };
        // Switch the strip to its flight look while it is still docked;
        // leaving Explorer then costs a single frame.
        let token = flight
            .as_ref()
            .and_then(|flight| self.prepare_flight(state, flight));
        self.detach(state, main);
        if let (Some(flight), Some(token), Some(overlay)) = (&flight, token, self.overlay_hwnd()) {
            self.run_flight(state, main, overlay, flight, token);
        }
        if self.visible.load(Ordering::Acquire) {
            win32::show_player(main);
        } else {
            win32::hide_player(main);
        }
        if let Some(overlay) = self.overlay_hwnd() {
            win32::hide(overlay);
        }
        self.set_phase(state, Phase::Hidden);
        Ok(())
    }

    fn foreground_is_clear(&self, state: &mut State, main_hwnd: u64) -> bool {
        let (Some(overlay), Some(player)) = (self.overlay_hwnd(), win32::window_rect(main_hwnd))
        else {
            return false;
        };
        state
            .helper
            .probe(&self.app, overlay)
            .is_ok_and(|probe| probe.is_clear_for(player))
    }

    fn fly(&self, state: &mut State, main: &WebviewWindow, overlay: u64, flight: &Flight) -> bool {
        if win32::set_rect(overlay, flight.from).is_err() {
            return false;
        }
        self.prepare_flight(state, flight)
            .is_some_and(|token| self.run_flight(state, main, overlay, flight, token))
    }

    /// Publishes a flight phase and waits until the overlay has painted it,
    /// so the window is never revealed with stale content.
    fn prepare_flight(&self, state: &mut State, flight: &Flight) -> Option<u64> {
        let token = self.set_phase(state, flight.phase);
        let deadline = Instant::now() + PAINT_TIMEOUT;
        while self.is_current(token) && Instant::now() < deadline {
            if self.painted_token.load(Ordering::Acquire) == token {
                return Some(token);
            }
            thread::sleep(Duration::from_millis(4));
        }
        self.set_phase(state, Phase::Hidden);
        None
    }

    /// Moves the painted overlay along the flight. Returns false when it was
    /// superseded by a hide request or a newer phase.
    fn run_flight(
        &self,
        state: &mut State,
        main: &WebviewWindow,
        overlay: u64,
        flight: &Flight,
        token: u64,
    ) -> bool {
        let reduced_motion = state.snapshot.as_ref().is_some_and(|s| s.reduced_motion);
        let (duration, start_rect) = if reduced_motion {
            // Reduced motion fades in place instead of travelling.
            (REDUCED_FLIGHT_DURATION, flight.to)
        } else {
            (FLIGHT_DURATION, flight.from)
        };
        if win32::set_rect(overlay, start_rect).is_err() {
            return false;
        }
        // Cover the player before hiding it so no frame shows neither window.
        win32::show_topmost(overlay);
        if flight.phase == Phase::Snapping {
            win32::hide_player(main);
        }
        let start = Instant::now();
        loop {
            if !self.is_current(token) {
                win32::hide(overlay);
                self.set_phase(state, Phase::Hidden);
                return false;
            }
            let progress = (start.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0);
            if !reduced_motion {
                let rect = model::interpolate_rect(flight.from, flight.to, progress);
                if win32::set_rect(overlay, rect).is_err() {
                    return false;
                }
            }
            if progress >= 1.0 {
                return true;
            }
            win32::wait_for_next_frame();
        }
    }

    fn is_current(&self, token: u64) -> bool {
        self.visible.load(Ordering::Acquire) && self.phase_token.load(Ordering::Acquire) == token
    }

    fn set_phase(&self, state: &mut State, phase: Phase) -> u64 {
        state.phase = phase;
        let token = self.phase_token.fetch_add(1, Ordering::AcqRel) + 1;
        self.emit_phase(state, token);
        token
    }

    fn emit_phase(&self, state: &State, token: u64) {
        let reduced_motion = state.snapshot.as_ref().is_some_and(|s| s.reduced_motion);
        let duration = if reduced_motion {
            REDUCED_FLIGHT_DURATION
        } else {
            FLIGHT_DURATION
        };
        let payload = json!({
            "phase": state.phase,
            "token": token,
            "durationMs": duration.as_millis() as u64,
        });
        if let Err(error) = self.app.emit_to(OVERLAY_LABEL, VISUAL_EVENT, payload) {
            eprintln!("[taskbar-docking] could not publish the overlay phase: {error}");
        }
    }

    /// Keeps the intent but shows neither window until the next poll.
    fn wait_hidden(&self, main: &WebviewWindow) {
        if let Some(overlay) = self.overlay_hwnd() {
            win32::hide(overlay);
        }
        if self.active.load(Ordering::Acquire) {
            win32::hide_player(main);
        }
    }

    fn reveal_player(&self, main: &WebviewWindow) {
        let hidden = main.is_minimized().unwrap_or(false) || !main.is_visible().unwrap_or(false);
        if hidden && self.visible.load(Ordering::Acquire) {
            win32::show_player(main);
        }
    }

    /// Called from the player's move and resize events. Only a pointer
    /// gesture starts a drag session; programmatic or keyboard moves never dock.
    pub fn note_player_moved(&self) {
        let hwnd = self.main_hwnd.load(Ordering::Acquire);
        if hwnd == 0
            || self.active.load(Ordering::Acquire)
            || self.stopping.load(Ordering::Acquire)
            || !win32::primary_button_pressed()
            || !win32::is_floating_compact(hwnd)
            || self.dragging.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let controller = self.clone();
        thread::spawn(move || {
            controller.watch_drag(hwnd);
            controller.dragging.store(false, Ordering::Release);
        });
    }

    fn watch_drag(&self, main_hwnd: u64) {
        let mut probe: Option<(Instant, model::Probe)> = None;
        let mut hint = DragHint::None;
        while !self.stopping.load(Ordering::Acquire) && !self.active.load(Ordering::Acquire) {
            // Sample the button first so a release during the probe still uses
            // the final window position.
            let released = !win32::primary_button_pressed();
            if probe
                .as_ref()
                .is_none_or(|(sampled, _)| sampled.elapsed() >= DRAG_PROBE_INTERVAL)
            {
                probe = self.probe_for_drag().map(|probe| (Instant::now(), probe));
            }
            let next = match (&probe, win32::client_rect(main_hwnd)) {
                (Some((_, probe)), Some(player)) if win32::is_floating_compact(main_hwnd) => {
                    model::drag_hint(probe, player)
                }
                _ => DragHint::None,
            };
            if next != hint {
                self.emit_hint(next);
                hint = next;
            }
            if released {
                if hint == DragHint::Ready {
                    self.dock_after_release();
                }
                break;
            }
            thread::sleep(DRAG_SAMPLE_INTERVAL);
        }
        if hint != DragHint::None {
            self.emit_hint(DragHint::None);
        }
    }

    /// A probe while the bar floats, or nothing when the strip could not be
    /// rendered on release anyway.
    fn probe_for_drag(&self) -> Option<model::Probe> {
        let overlay = self.overlay_hwnd()?;
        let mut state = self.state.lock().ok()?;
        let ready = self.overlay_ready.load(Ordering::Acquire) && state.snapshot.is_some();
        if !ready || state.attached_bar.is_some() || !state.compact {
            return None;
        }
        state.helper.probe(&self.app, overlay).ok()
    }

    fn emit_hint(&self, hint: DragHint) {
        if let Err(error) = self
            .app
            .emit_to(MAIN_LABEL, HINT_EVENT, json!({ "state": hint }))
        {
            eprintln!("[taskbar-docking] could not publish the drag hint: {error}");
        }
    }

    fn dock_after_release(&self) {
        let (Ok(main), Ok(mut state)) = (self.main(), self.lock()) else {
            return;
        };
        if state.enabled || !state.compact {
            return;
        }
        self.set_enabled(&mut state, true);
        self.refresh(&mut state, &main);
        if state.attached_bar.is_none() {
            // A release that could not dock leaves an ordinary floating
            // player, not an intent that would snap later.
            if self.return_to_player(&mut state, &main).is_ok() {
                self.set_enabled(&mut state, false);
            }
        }
    }

    fn publish_snapshot(&self, snapshot: OverlaySnapshot) -> Result<(), String> {
        snapshot.validate()?;
        let mut state = self.lock()?;
        if self.overlay_ready.load(Ordering::Acquire) {
            self.app
                .emit_to(OVERLAY_LABEL, SNAPSHOT_EVENT, &snapshot)
                .map_err(|error| error.to_string())?;
        }
        state.snapshot = Some(snapshot);
        Ok(())
    }

    /// The overlay page is listening: replay the latest snapshot and phase.
    fn overlay_page_ready(&self) -> Result<(), String> {
        let state = self.lock()?;
        self.overlay_ready.store(true, Ordering::Release);
        if let Some(snapshot) = &state.snapshot {
            self.app
                .emit_to(OVERLAY_LABEL, SNAPSHOT_EVENT, snapshot)
                .map_err(|error| error.to_string())?;
        }
        self.emit_phase(&state, self.phase_token.load(Ordering::Acquire));
        Ok(())
    }

    fn main(&self) -> Result<WebviewWindow, String> {
        self.app
            .get_webview_window(MAIN_LABEL)
            .ok_or_else(|| "the player window is unavailable".to_owned())
    }

    fn overlay_hwnd(&self) -> Option<u64> {
        self.app
            .get_webview_window(OVERLAY_LABEL)
            .and_then(|overlay| win32::hwnd(&overlay).ok())
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, String> {
        self.state
            .lock()
            .map_err(|_| "taskbar docking state is unavailable".to_owned())
    }
}

fn controller(window: &WebviewWindow, label: &str) -> Result<TaskbarDockController, String> {
    if window.label() != label {
        return Err(format!("this command is restricted to the {label} window"));
    }
    Ok((*window.state::<TaskbarDockController>()).clone())
}

/// Runs native docking work off the async runtime's worker threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn sync_taskbar_compact_mode(window: WebviewWindow, compact: bool) -> Result<(), String> {
    let controller = controller(&window, MAIN_LABEL)?;
    blocking(move || controller.set_compact(compact)).await
}

#[tauri::command]
pub async fn publish_taskbar_overlay_snapshot(
    window: WebviewWindow,
    snapshot: OverlaySnapshot,
) -> Result<(), String> {
    let controller = controller(&window, MAIN_LABEL)?;
    blocking(move || controller.publish_snapshot(snapshot)).await
}

#[tauri::command]
pub async fn taskbar_overlay_ready(window: WebviewWindow) -> Result<(), String> {
    let controller = controller(&window, OVERLAY_LABEL)?;
    blocking(move || controller.overlay_page_ready()).await
}

#[tauri::command]
pub fn taskbar_overlay_visual_ready(window: WebviewWindow, token: u64) -> Result<(), String> {
    let controller = controller(&window, OVERLAY_LABEL)?;
    controller.painted_token.store(token, Ordering::Release);
    Ok(())
}

#[tauri::command]
pub async fn taskbar_overlay_action(
    window: WebviewWindow,
    action: String,
    value: Option<f64>,
) -> Result<(), String> {
    let controller = controller(&window, OVERLAY_LABEL)?;
    match (action.as_str(), value) {
        ("close", None) => controller.request_hide(),
        ("undock", None) => blocking(move || controller.undock()).await?,
        ("previous" | "play" | "next" | "shuffle", None) => {
            controller.relay_action(json!({ "action": action }))?;
        }
        ("seek", Some(value)) if model::is_track_time(value) => {
            controller.relay_action(json!({ "action": action, "value": value }))?;
        }
        _ => return Err("taskbar overlay action is invalid".to_owned()),
    }
    Ok(())
}

impl TaskbarDockController {
    fn relay_action(&self, action: serde_json::Value) -> Result<(), String> {
        self.app
            .emit_to(MAIN_LABEL, ACTION_EVENT, action)
            .map_err(|error| error.to_string())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preference {
    version: u8,
    enabled: bool,
}

fn preference_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map(|directory| directory.join(PREFERENCE_FILE))
        .map_err(|error| error.to_string())
}

fn load_preference(app: &AppHandle) -> bool {
    let Ok(bytes) = preference_path(app).and_then(|path| fs::read(path).map_err(|e| e.to_string()))
    else {
        return false;
    };
    match serde_json::from_slice::<Preference>(&bytes) {
        Ok(Preference {
            version: 1,
            enabled,
        }) => enabled,
        _ => {
            eprintln!("[taskbar-docking] ignored an invalid docking preference");
            false
        }
    }
}

fn save_preference(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let path = preference_path(app)?;
    let directory = path.parent().ok_or("preference path has no parent")?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&Preference {
        version: 1,
        enabled,
    })
    .map_err(|error| error.to_string())?;
    // Write and rename so a crash never leaves a truncated preference.
    let temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    fs::write(temporary.path(), bytes).map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map(drop)
        .map_err(|error| error.error.to_string())
}
