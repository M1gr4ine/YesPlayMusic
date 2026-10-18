//! Owns the docking state and runs its effects.
//!
//! Every input (window events, hooks, probe answers, IPC) is a message on
//! one queue, and one worker thread drains it: it steps the model, runs the
//! effects in order, advances the active flight between messages and keeps
//! the probe debounce. Nothing else touches windows, so an abort can never
//! race a flight that is still hiding the player. The main thread only ever
//! enqueues; the worker is the one that blocks on the main thread for
//! window calls.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow, WindowEvent};

use super::{
    attach::{self, HookEvent, Watcher},
    model::{self, Effect, Event, Flight, Motion, Phase, Rect, Settings, State, StripPhase},
    probe::{Measurement, ProbeThread},
    OVERLAY_LABEL,
};

const MAIN_LABEL: &str = "main";
const HINT_EVENT: &str = "desktop://taskbarDockHint";
const DOCK_STATE_EVENT: &str = "desktop://taskbarDockState";
/// Sent to the player page, which relays it to the strip over their channel.
const STRIP_PHASE_EVENT: &str = "desktop://taskbarStripPhase";
/// Explorer reports a burst of changes per layout; one probe per burst.
const TASKBAR_SETTLE: Duration = Duration::from_millis(150);
/// A burst that never ends still gets probed this often.
const TASKBAR_SETTLE_MAX: Duration = Duration::from_millis(600);
/// How long a freshly created strip page may take to start listening.
const OVERLAY_READY_TIMEOUT: Duration = Duration::from_millis(1500);
/// The probe bounds each provider call to 1.5 s; longer means it is wedged.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// Explorer can re-order windows in bursts; one raise per burst is enough.
const RAISE_INTERVAL: Duration = Duration::from_millis(50);

pub type OverlayFactory = Box<dyn Fn(&AppHandle) -> tauri::Result<WebviewWindow> + Send + Sync>;

enum Message {
    Event(Event),
    /// A raw taskbar change; the worker coalesces these into one event.
    TaskbarNotice,
    Probed {
        generation: u64,
        serial: u64,
        measurement: Measurement,
    },
    OverlayGone,
    Shutdown,
}

#[derive(Clone)]
pub struct Controller(Arc<Inner>);

struct Inner {
    app: AppHandle,
    state: Mutex<State>,
    overlay_factory: OverlayFactory,
    main_hwnd: AtomicUsize,
    inbox: mpsc::Sender<Message>,
    stopping: AtomicBool,
}

impl Controller {
    pub fn new(app: AppHandle, overlay_factory: OverlayFactory) -> Self {
        let (inbox, outbox) = mpsc::channel();
        let inner = Arc::new(Inner {
            app,
            state: Mutex::new(State::default()),
            overlay_factory,
            main_hwnd: AtomicUsize::new(0),
            inbox: inbox.clone(),
            stopping: AtomicBool::new(false),
        });
        let worker = Worker::new(Arc::clone(&inner), inbox, outbox);
        if let Err(error) = thread::Builder::new()
            .name("taskbar-dock".into())
            .spawn(move || worker.run())
        {
            eprintln!("[taskbar-docking] no worker thread: {error}");
            inner.stopping.store(true, Ordering::Release);
        }
        Self(inner)
    }

    /// Remembers the player window; the hooks are installed once the
    /// settings say docking is on.
    pub fn attach_main(&self, window: &WebviewWindow) -> Result<(), String> {
        let hwnd = crate::win32::hwnd_of(window)?;
        self.0.main_hwnd.store(hwnd, Ordering::Release);
        self.sync_player_frame(window);
        Ok(())
    }

    pub fn on_main_window_event(&self, window: &WebviewWindow, event: &WindowEvent) {
        if matches!(
            event,
            WindowEvent::Moved(_)
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
        ) {
            self.sync_player_frame(window);
        }
    }

    pub fn settings(&self, docking: bool, animations: bool, reduced_motion: bool) {
        self.dispatch(Event::Settings(Settings {
            docking,
            animations,
            reduced_motion,
        }));
    }

    pub fn is_docked(&self) -> bool {
        lock(&self.0.state).is_docked()
    }

    pub fn strip_visible(&self) -> bool {
        lock(&self.0.state).strip_visible()
    }

    /// Returns whether the request was taken over by the strip; `false`
    /// means the caller should show the player window itself.
    pub fn show_requested(&self) -> bool {
        let docked = self.is_docked();
        if docked {
            self.dispatch(Event::ShowRequested);
        }
        docked
    }

    pub fn hide_requested(&self) {
        self.dispatch(Event::HideRequested);
    }

    pub fn undock(&self) {
        self.dispatch(Event::UndockRequested);
    }

    /// Leaves the taskbar at once; the player is about to be resized or shown.
    pub fn detach(&self) {
        self.dispatch(Event::Abort);
    }

    /// The strip page is listening: replay the phase it missed while loading.
    pub fn strip_ready(&self) {
        self.dispatch(Event::StripReady);
    }

    pub fn shutdown(&self) {
        self.0.stopping.store(true, Ordering::Release);
        let _ = self.0.inbox.send(Message::Shutdown);
    }

    fn on_hook(&self, event: HookEvent) {
        match event {
            HookEvent::DragStarted => self.dispatch(Event::DragStarted),
            HookEvent::DragEnded { frame, compact } => {
                self.dispatch(Event::DragEnded { frame, compact });
            }
            HookEvent::TaskbarChanged => self.send(Message::TaskbarNotice),
            HookEvent::TaskbarRaised => self.dispatch(Event::TaskbarRaised),
            HookEvent::Foreground { fullscreen } => {
                self.dispatch(Event::Foreground { fullscreen });
            }
            HookEvent::ExplorerRestarted => self.dispatch(Event::ExplorerRestarted),
        }
    }

    fn sync_player_frame(&self, window: &WebviewWindow) {
        let read = || -> tauri::Result<(Rect, bool)> {
            let position = window.outer_position()?;
            let outer = window.outer_size()?;
            let inner = window.inner_size()?;
            let scale = window.scale_factor()?;
            let normal =
                !window.is_minimized()? && !window.is_maximized()? && !window.is_fullscreen()?;
            let compact = normal && f64::from(inner.height) / scale < model::COMPACT_BAR_MAX_HEIGHT;
            let frame = Rect::new(
                position.x,
                position.y,
                i32::try_from(outer.width).unwrap_or(i32::MAX),
                i32::try_from(outer.height).unwrap_or(i32::MAX),
            );
            Ok((frame, compact))
        };
        if let Ok((frame, compact)) = read() {
            self.dispatch(Event::PlayerFrame { frame, compact });
        }
    }

    fn dispatch(&self, event: Event) {
        self.send(Message::Event(event));
    }

    /// Once shutting down, nothing may reach the worker: a strip destroyed
    /// on exit must not flash the player back.
    fn send(&self, message: Message) {
        if self.0.stopping.load(Ordering::Acquire) {
            return;
        }
        let _ = self.0.inbox.send(message);
    }
}

struct ActiveFlight {
    flight: Flight,
    overlay: usize,
    started: Instant,
}

/// The single probe in flight, and whether another was asked for meanwhile.
#[derive(Default)]
struct ProbeState {
    thread: Option<ProbeThread>,
    running: Option<(u64, Instant)>,
    rerun: bool,
    /// Stopped answering: fail closed until it does answer again.
    wedged: bool,
}

struct Worker {
    inner: Arc<Inner>,
    inbox: mpsc::Sender<Message>,
    outbox: mpsc::Receiver<Message>,
    flight: Option<ActiveFlight>,
    /// When to turn the taskbar notices into one `TaskbarChanged`.
    settle: Option<(Instant, Instant)>,
    /// When a strip page that never listened gets given up on.
    prepare_deadline: Option<Instant>,
    probe: ProbeState,
    /// Count across enable cycles, so an answer from a retired probe
    /// thread can never match a current request.
    probe_generation: u64,
    probe_serial: u64,
    /// Spawned on the first enable and kept for the app's lifetime.
    watcher: Option<Watcher>,
    overlay_listening: bool,
    last_raise: Option<Instant>,
    raise_due: Option<Instant>,
    aborting: bool,
}

impl Worker {
    fn new(
        inner: Arc<Inner>,
        inbox: mpsc::Sender<Message>,
        outbox: mpsc::Receiver<Message>,
    ) -> Self {
        Self {
            inner,
            inbox,
            outbox,
            flight: None,
            settle: None,
            prepare_deadline: None,
            probe: ProbeState::default(),
            probe_generation: 0,
            probe_serial: 0,
            watcher: None,
            overlay_listening: false,
            last_raise: None,
            raise_due: None,
            aborting: false,
        }
    }

    fn run(mut self) {
        loop {
            let message = match self.next_wake() {
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    Some(wait) if !wait.is_zero() => self.outbox.recv_timeout(wait),
                    _ => self.outbox.try_recv().map_err(|error| match error {
                        mpsc::TryRecvError::Empty => RecvTimeoutError::Timeout,
                        mpsc::TryRecvError::Disconnected => RecvTimeoutError::Disconnected,
                    }),
                },
                None => self
                    .outbox
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            // Once the app is exiting, whatever was queued before (an abort,
            // a dying overlay) must not touch windows any more.
            if self.inner.stopping.load(Ordering::Acquire) {
                break;
            }
            match message {
                Ok(Message::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(message) => self.handle(message),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
        }
        self.flight = None;
        self.uninstall_watcher();
        self.watcher = None;
    }

    fn next_wake(&self) -> Option<Instant> {
        if self.flight.is_some() {
            return Some(Instant::now());
        }
        [
            self.settle.map(|(due, _)| due),
            self.prepare_deadline,
            self.probe
                .running
                .map(|(_, started)| started + PROBE_TIMEOUT),
            self.raise_due,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Event(event) => {
                if event == Event::StripReady {
                    self.overlay_listening = true;
                    self.prepare_deadline = None;
                }
                self.step(event);
            }
            Message::TaskbarNotice => {
                let now = Instant::now();
                let first = self.settle.map_or(now, |(_, first)| first);
                self.settle = Some((
                    (now + TASKBAR_SETTLE).min(first + TASKBAR_SETTLE_MAX),
                    first,
                ));
            }
            Message::Probed {
                generation,
                serial,
                measurement,
            } => {
                if generation != self.probe_generation {
                    return;
                }
                // Any answer proves the thread is alive, even a stale one.
                self.probe.wedged = false;
                if self
                    .probe
                    .running
                    .is_some_and(|(running, _)| running == serial)
                {
                    self.probe.running = None;
                    let taskbars = measurement
                        .map_err(|error| eprintln!("[taskbar-docking] probe failed: {error}"))
                        .ok();
                    self.step(Event::Taskbars(taskbars));
                    if std::mem::take(&mut self.probe.rerun) {
                        self.request_probe();
                    }
                    if let Some(watcher) = &self.watcher {
                        watcher.refresh_explorer();
                    }
                }
            }
            Message::OverlayGone => {
                self.overlay_listening = false;
                self.flight = None;
                self.step(Event::Abort);
            }
            Message::Shutdown => {}
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if self.settle.is_some_and(|(due, _)| due <= now) {
            self.settle = None;
            self.step(Event::TaskbarChanged);
        }
        if self.prepare_deadline.is_some_and(|due| due <= now) {
            self.prepare_deadline = None;
            let still_waiting = matches!(
                lock(&self.inner.state).phase,
                Phase::Docking {
                    prepared: false,
                    ..
                }
            );
            // A page that never listens would fly invisible: fail closed and
            // let the next dock build a fresh one.
            if still_waiting {
                eprintln!("[taskbar-docking] the taskbar strip did not start listening");
                self.step(Event::Abort);
                if let Some(overlay) = self.inner.app.get_webview_window(OVERLAY_LABEL) {
                    let _ = overlay.destroy();
                }
            }
        }
        if self
            .probe
            .running
            .is_some_and(|(_, started)| started + PROBE_TIMEOUT <= now)
        {
            eprintln!("[taskbar-docking] the taskbar probe is not answering");
            self.probe.running = None;
            self.probe.rerun = false;
            self.probe.wedged = true;
            self.step(Event::Taskbars(None));
        }
        if self.raise_due.is_some_and(|due| due <= now) {
            self.raise_due = None;
            self.raise_now();
        }
        self.advance_flight();
    }

    fn advance_flight(&mut self) {
        let Some(active) = &self.flight else {
            return;
        };
        let duration = Duration::from_millis(u64::from(active.flight.motion.duration_ms()));
        let progress = if duration.is_zero() {
            1.0
        } else {
            (active.started.elapsed().as_secs_f64() / duration.as_secs_f64()).min(1.0)
        };
        let flight = active.flight;
        let overlay = active.overlay;
        let placed = if flight.motion == Motion::Fly && progress < 1.0 {
            attach::set_rect(
                overlay,
                model::interpolate(flight.from, flight.to, progress),
            )
        } else if progress >= 1.0 {
            settle_rect(overlay, flight.to)
        } else {
            Ok(())
        };
        if let Err(error) = placed {
            eprintln!("[taskbar-docking] {error}");
            self.flight = None;
            self.abort();
            return;
        }
        if progress >= 1.0 {
            self.flight = None;
            self.step(Event::FlightFinished);
        } else {
            match flight.motion {
                Motion::Fly => attach::wait_frame(),
                Motion::Fade | Motion::Instant => thread::sleep(Duration::from_millis(16)),
            }
        }
    }

    fn step(&mut self, event: Event) {
        let effects = {
            let mut state = lock(&self.inner.state);
            let (next, effects) = model::step(std::mem::take(&mut state), event);
            *state = next;
            effects
        };
        for effect in effects {
            if self.inner.stopping.load(Ordering::Acquire) {
                return;
            }
            if let Err(error) = self.run_effect(effect) {
                eprintln!("[taskbar-docking] {error}");
                self.abort();
                return;
            }
        }
    }

    /// An effect failed: drop the strip. Abort effects themselves never
    /// fail, so one level is enough.
    fn abort(&mut self) {
        if self.aborting {
            return;
        }
        self.aborting = true;
        self.flight = None;
        self.step(Event::Abort);
        self.aborting = false;
    }

    fn run_effect(&mut self, effect: Effect) -> Result<(), String> {
        match effect {
            Effect::InstallWatcher => self.install_watcher()?,
            Effect::UninstallWatcher => self.uninstall_watcher(),
            Effect::Probe => self.request_probe(),
            Effect::RetryProbe(ms) => {
                let due = Instant::now() + Duration::from_millis(u64::from(ms));
                let sooner = self.settle.is_none_or(|(current, _)| due < current);
                if sooner {
                    self.settle = Some((due, due));
                }
            }
            Effect::Hint(hint) => self.emit_to_main(HINT_EVENT, json!({ "state": hint })),
            Effect::PrepareStrip(rect) => {
                let overlay = self.ensure_overlay()?;
                attach::set_rect(overlay, rect)?;
                if self.overlay_listening {
                    // Queued, not stepped: anything that arrived meanwhile
                    // (an abort, a new drag) is handled first.
                    let _ = self.inbox.send(Message::Event(Event::StripReady));
                } else {
                    self.prepare_deadline = Some(Instant::now() + OVERLAY_READY_TIMEOUT);
                }
            }
            Effect::ShowStrip => attach::show_topmost(self.overlay_hwnd()?)?,
            Effect::HidePlayer => {
                let main = self.inner.main_hwnd.load(Ordering::Acquire);
                if attach::in_move_loop(main) {
                    return Err("the player is being dragged again".to_owned());
                }
                if let Some(player) = self.inner.app.get_webview_window(MAIN_LABEL) {
                    player.hide().map_err(|error| error.to_string())?;
                }
            }
            Effect::StartFlight(flight) => {
                let overlay = self.overlay_hwnd()?;
                attach::set_rect(overlay, flight.start())?;
                self.flight = Some(ActiveFlight {
                    flight,
                    overlay,
                    started: Instant::now(),
                });
            }
            Effect::PlaceStrip(rect) => settle_rect(self.overlay_hwnd()?, rect)?,
            Effect::Raise => self.raise(),
            Effect::HideStrip => {
                self.flight = None;
                if let Ok(overlay) = self.overlay_hwnd() {
                    attach::hide(overlay);
                }
            }
            Effect::ShowPlayer(home) => self.show_player(home),
            Effect::StripPhase(phase) => self.emit_phase(phase),
            Effect::DockedChanged(docked) => {
                self.emit_to_main(DOCK_STATE_EVENT, json!({ "docked": docked }));
            }
        }
        Ok(())
    }

    fn install_watcher(&mut self) -> Result<(), String> {
        if self.watcher.is_none() {
            let main = self.inner.main_hwnd.load(Ordering::Acquire);
            if main == 0 {
                return Err("the player window is not attached yet".to_owned());
            }
            let controller = Controller(Arc::clone(&self.inner));
            self.watcher = Some(Watcher::spawn(
                main,
                Box::new(move |event| controller.on_hook(event)),
            )?);
        }
        if let Some(watcher) = &self.watcher {
            watcher.enable();
        }
        Ok(())
    }

    /// The watcher thread removes its WinEvent hooks, the UI Automation
    /// handlers and the hidden window on its own thread, in order with any
    /// later enable; dropping the probe thread's sender ends that thread.
    fn uninstall_watcher(&mut self) {
        if let Some(watcher) = &self.watcher {
            watcher.disable();
        }
        self.probe = ProbeState::default();
        self.settle = None;
    }

    fn request_probe(&mut self) {
        if self.probe.wedged {
            let _ = self.inbox.send(Message::Event(Event::Taskbars(None)));
            return;
        }
        if self.probe.running.is_some() {
            self.probe.rerun = true;
            return;
        }
        if self.probe.thread.is_none() {
            let report = self.inbox.clone();
            self.probe_generation += 1;
            match ProbeThread::spawn(
                self.probe_generation,
                move |generation, serial, measurement| {
                    let _ = report.send(Message::Probed {
                        generation,
                        serial,
                        measurement,
                    });
                },
            ) {
                Ok(thread) => self.probe.thread = Some(thread),
                Err(error) => {
                    eprintln!("[taskbar-docking] {error}");
                    let _ = self.inbox.send(Message::Event(Event::Taskbars(None)));
                    return;
                }
            }
        }
        self.probe_serial += 1;
        let serial = self.probe_serial;
        let sent = self
            .probe
            .thread
            .as_ref()
            .map_or(Err("no probe thread".to_owned()), |thread| {
                thread.request(serial)
            });
        match sent {
            Ok(()) => self.probe.running = Some((serial, Instant::now())),
            Err(error) => {
                eprintln!("[taskbar-docking] {error}");
                self.probe.thread = None;
                let _ = self.inbox.send(Message::Event(Event::Taskbars(None)));
            }
        }
    }

    fn raise(&mut self) {
        let recent = self
            .last_raise
            .filter(|last| last.elapsed() < RAISE_INTERVAL);
        match recent {
            Some(last) if self.raise_due.is_none() => self.raise_due = Some(last + RAISE_INTERVAL),
            Some(_) => {}
            None => self.raise_now(),
        }
    }

    fn raise_now(&mut self) {
        self.last_raise = Some(Instant::now());
        if !lock(&self.inner.state).strip_visible() {
            return;
        }
        if let Ok(overlay) = self.overlay_hwnd() {
            if let Err(error) = attach::raise(overlay) {
                eprintln!("[taskbar-docking] {error}");
            }
        }
    }

    fn ensure_overlay(&mut self) -> Result<usize, String> {
        if self.inner.stopping.load(Ordering::Acquire) {
            return Err("the app is shutting down".to_owned());
        }
        if let Some(overlay) = self.inner.app.get_webview_window(OVERLAY_LABEL) {
            return crate::win32::hwnd_of(&overlay);
        }
        self.overlay_listening = false;
        let overlay =
            (self.inner.overlay_factory)(&self.inner.app).map_err(|error| error.to_string())?;
        let inner = Arc::clone(&self.inner);
        overlay.on_window_event(move |event| {
            // Windows die on exit too; by then nobody may bring the player back.
            if matches!(event, WindowEvent::Destroyed) && !inner.stopping.load(Ordering::Acquire) {
                let _ = inner.inbox.send(Message::OverlayGone);
            }
        });
        crate::win32::hwnd_of(&overlay)
    }

    fn overlay_hwnd(&self) -> Result<usize, String> {
        self.inner
            .app
            .get_webview_window(OVERLAY_LABEL)
            .ok_or_else(|| "the taskbar strip is gone".to_owned())
            .and_then(|overlay| crate::win32::hwnd_of(&overlay))
    }

    /// Restores a hidden player at its home; a player that is still showing
    /// (the hide was refused) is left where the user has it.
    fn show_player(&self, home: Rect) {
        if self.inner.stopping.load(Ordering::Acquire) {
            return;
        }
        let Some(player) = self.inner.app.get_webview_window(MAIN_LABEL) else {
            return;
        };
        if player.is_visible().unwrap_or(false) && !player.is_minimized().unwrap_or(false) {
            return;
        }
        let _ = player.set_position(PhysicalPosition::new(home.x, home.y));
        if player.is_minimized().unwrap_or(false) {
            let _ = player.unminimize();
        }
        if let Err(error) = player.show() {
            eprintln!("[taskbar-docking] could not show the player: {error}");
        }
        // The display the bar was docked from may be gone by now.
        let _ = crate::ensure_main_window_reachable(&player);
    }

    fn emit_phase(&self, phase: StripPhase) {
        let (name, motion) = match phase {
            StripPhase::Hidden => ("hidden", None),
            StripPhase::Docking { motion } => ("docking", Some(motion)),
            StripPhase::Docked => ("docked", None),
            StripPhase::Undocking { motion } => ("undocking", Some(motion)),
        };
        self.emit_to_main(
            STRIP_PHASE_EVENT,
            json!({
                "phase": name,
                "motion": motion,
                "durationMs": motion.map_or(0, Motion::duration_ms),
            }),
        );
    }

    fn emit_to_main(&self, event: &str, payload: serde_json::Value) {
        if let Err(error) = self.inner.app.emit_to(MAIN_LABEL, event, payload) {
            eprintln!("[taskbar-docking] could not publish {event}: {error}");
        }
    }
}

/// Places the strip and re-applies once if Tao's DPI handling resized it.
fn settle_rect(overlay: usize, rect: Rect) -> Result<(), String> {
    attach::set_rect(overlay, rect)?;
    if attach::window_rect(overlay) != Some(rect) {
        attach::set_rect(overlay, rect)?;
    }
    attach::redraw(overlay)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
