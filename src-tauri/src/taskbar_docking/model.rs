//! Platform-independent half of Windows taskbar docking: taskbar geometry
//! and the docking state machine. Nothing here touches Win32, so every CI
//! platform runs these tests.

/// A rectangle in physical screen pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> i32 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.height)
    }

    pub fn is_empty(self) -> bool {
        self.width <= 0 || self.height <= 0
    }

    pub fn union(self, other: Rect) -> Rect {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }

    /// The part of `self` inside `bounds`, or `None` when nothing is.
    pub fn clip(self, bounds: Rect) -> Option<Rect> {
        let x = self.x.max(bounds.x);
        let y = self.y.max(bounds.y);
        let right = self.right().min(bounds.right());
        let bottom = self.bottom().min(bounds.bottom());
        (right > x && bottom > y).then(|| Rect::new(x, y, right - x, bottom - y))
    }

    fn horizontal_overlap(self, other: Rect) -> i32 {
        (self.right().min(other.right()) - self.x.max(other.x)).max(0)
    }

    fn vertical_gap(self, other: Rect) -> i32 {
        (other.y - self.bottom())
            .max(self.y - other.bottom())
            .max(0)
    }
}

/// Device-independent pixels to physical pixels, rounded up.
pub fn dip(value: i32, dpi: u32) -> i32 {
    let scaled = (i64::from(value) * i64::from(dpi) + 95) / 96;
    i32::try_from(scaled).unwrap_or(i32::MAX)
}

/// Logical width of the strip; the overlay page lays out for exactly this.
pub const STRIP_WIDTH_DIP: i32 = 380;
const STRIP_MARGIN_DIP: i32 = 4;
/// How close to the taskbar the bar has to be released to dock.
const SNAP_DISTANCE_DIP: i32 = 40;
/// `isBarWindowSize` in the renderer: shorter windows lay out as the bar.
pub const COMPACT_BAR_MAX_HEIGHT: f64 = 340.0;
pub const FLIGHT_MS: u32 = 240;
pub const FADE_MS: u32 = 120;
/// Waits before re-measuring after a failed probe while docked; the last
/// value repeats.
const RETRY_MS: [u32; 3] = [150, 1000, 5000];

/// One taskbar as the probe measured it. `container` is where task buttons
/// may live (the whole bar on Windows 11, `MSTaskListWClass` on the legacy
/// taskbar); `content` is the span they actually occupy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Taskbar {
    /// The taskbar window handle, used only as an identity across probes.
    pub id: usize,
    pub bounds: Rect,
    pub dpi: u32,
    pub container: Rect,
    pub content: Option<Rect>,
    pub widgets: Option<Rect>,
    pub tray: Option<Rect>,
    pub auto_hide: bool,
}

impl Taskbar {
    pub fn is_horizontal(&self) -> bool {
        self.bounds.width >= self.bounds.height
    }

    /// The empty band between the last task button and the tray.
    pub fn free_band(&self) -> Option<Rect> {
        if !self.is_horizontal() {
            return None;
        }
        let mut low = self.container.x.max(self.bounds.x);
        let mut high = self.container.right().min(self.bounds.right());
        let content_high = self.content.map_or(low, Rect::right);
        // The widgets button sits before the buttons on a centred bar and
        // after them on a left-aligned one; with no buttons, go by its side.
        if let Some(widgets) = self.widgets {
            let before = match self.content {
                Some(content) => widgets.x < content.x,
                None => widgets.x + widgets.width / 2 < (low + high) / 2,
            };
            if before {
                low = low.max(widgets.right());
            } else {
                high = high.min(widgets.x);
            }
        }
        if let Some(tray) = self.tray {
            high = high.min(tray.x);
        }
        let start = content_high.clamp(low, high.max(low));
        (high > start).then(|| Rect::new(start, self.bounds.y, high - start, self.bounds.height))
    }

    /// Where the strip lands: the tray end of the free band, or nothing when
    /// the band is too narrow, the bar auto-hides or is vertical.
    pub fn strip_rect(&self) -> Option<Rect> {
        if self.auto_hide || !(48..=768).contains(&self.dpi) {
            return None;
        }
        let band = self.free_band()?;
        let width = dip(STRIP_WIDTH_DIP, self.dpi);
        let margin = dip(STRIP_MARGIN_DIP, self.dpi);
        (band.width >= width + margin)
            .then(|| Rect::new(band.right() - margin - width, band.y, width, band.height))
    }

    /// Overlapping the bar horizontally and within reach of it vertically.
    pub fn is_near(&self, frame: Rect) -> bool {
        let reach = dip(SNAP_DISTANCE_DIP, self.dpi.clamp(48, 768));
        frame.horizontal_overlap(self.bounds) > 0 && frame.vertical_gap(self.bounds) <= reach
    }
}

/// The horizontal taskbar the bar is closest to: most overlap, then the
/// smallest gap, so a bar across a monitor seam docks to the side it is on.
pub fn nearest(taskbars: &[Taskbar], frame: Rect) -> Option<&Taskbar> {
    taskbars
        .iter()
        .filter(|taskbar| taskbar.is_horizontal() && taskbar.is_near(frame))
        .max_by_key(|taskbar| {
            (
                frame.horizontal_overlap(taskbar.bounds),
                -frame.vertical_gap(taskbar.bounds),
            )
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DragHint {
    None,
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub docking: bool,
    pub animations: bool,
    /// The system asks for reduced motion: no flight, no fade.
    pub reduced_motion: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    Fly,
    Fade,
    Instant,
}

impl Motion {
    pub fn duration_ms(self) -> u32 {
        match self {
            Motion::Fly => FLIGHT_MS,
            Motion::Fade => FADE_MS,
            Motion::Instant => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flight {
    pub from: Rect,
    pub to: Rect,
    pub motion: Motion,
}

impl Flight {
    /// Where the strip sits before the flight starts.
    pub fn start(self) -> Rect {
        match self.motion {
            Motion::Fly => self.from,
            Motion::Fade | Motion::Instant => self.to,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub taskbar: usize,
    pub strip: Rect,
}

/// A pointer move or resize loop of the compact bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drag {
    pub frame: Rect,
    /// The frame when the loop started; a changed size means a resize.
    pub started: Rect,
    pub taskbars: Option<Vec<Taskbar>>,
    pub hint: DragHint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Undocked {
        drag: Option<Drag>,
    },
    Docking {
        placement: Placement,
        home: Rect,
        /// The strip page is listening and the player has been hidden.
        prepared: bool,
    },
    Docked {
        placement: Placement,
        /// Where the bar was released; undocking returns there.
        home: Rect,
        /// The user wants the strip shown (as opposed to hidden to the tray).
        visible: bool,
        /// Hidden under a full-screen foreground app, to return afterwards.
        covered: bool,
        /// A show request waiting for the probe that confirms the strip fits.
        show_pending: bool,
        /// Consecutive failed probes; only drives the retry back-off.
        failures: u32,
    },
    Undocking {
        home: Rect,
        show_player: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub settings: Settings,
    pub frame: Option<Rect>,
    /// The player window currently lays out as the compact bar.
    pub compact: bool,
    /// A full-screen app owns the foreground.
    pub fullscreen: bool,
    /// The last successful measurement; seeds a drag until the probe answers.
    pub taskbars: Option<Vec<Taskbar>>,
    /// The Explorer-side watcher (hooks, UI Automation handlers, probe
    /// thread) is installed; it exists only while docking is enabled.
    pub watching: bool,
    pub phase: Phase,
}

impl Default for State {
    fn default() -> Self {
        Self {
            settings: Settings {
                docking: true,
                animations: true,
                reduced_motion: false,
            },
            frame: None,
            compact: false,
            fullscreen: false,
            taskbars: None,
            watching: false,
            phase: Phase::Undocked { drag: None },
        }
    }
}

impl State {
    pub fn is_docked(&self) -> bool {
        !matches!(self.phase, Phase::Undocked { .. })
    }

    pub fn strip_visible(&self) -> bool {
        matches!(
            self.phase,
            Phase::Docked {
                visible: true,
                covered: false,
                ..
            }
        )
    }

    pub fn strip_phase(&self) -> StripPhase {
        match self.phase {
            Phase::Undocked { .. } => StripPhase::Hidden,
            Phase::Docking { .. } => StripPhase::Docking {
                motion: self.motion(),
            },
            Phase::Docked { .. } => StripPhase::Docked,
            Phase::Undocking { .. } => StripPhase::Undocking {
                motion: self.motion(),
            },
        }
    }

    fn motion(&self) -> Motion {
        if self.settings.reduced_motion {
            Motion::Instant
        } else if self.settings.animations {
            Motion::Fly
        } else {
            Motion::Fade
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Settings(Settings),
    PlayerFrame {
        frame: Rect,
        compact: bool,
    },
    DragStarted,
    /// The move or resize loop ended; the hook reads the final frame itself
    /// because the window events may still be in flight.
    DragEnded {
        frame: Option<Rect>,
        compact: Option<bool>,
    },
    /// A probe result; `None` means the probe failed or timed out.
    Taskbars(Option<Vec<Taskbar>>),
    /// Explorer moved or resized a taskbar; the controller coalesces these.
    TaskbarChanged,
    /// Explorer re-asserted the taskbar's z-order.
    TaskbarRaised,
    Foreground {
        fullscreen: bool,
    },
    /// Explorer restarted: every measurement and handle is stale.
    ExplorerRestarted,
    /// The strip page is listening for its phase.
    StripReady,
    FlightFinished,
    ShowRequested,
    HideRequested,
    UndockRequested,
    /// Leave the strip at once, without animation: an effect failed, the
    /// overlay died, or the player is about to be resized.
    Abort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripPhase {
    Hidden,
    Docking { motion: Motion },
    Docked,
    Undocking { motion: Motion },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Hook the player's move loop and watch Explorer; docking is enabled.
    InstallWatcher,
    /// Remove every hook and handler and stop the probe thread.
    UninstallWatcher,
    Probe,
    /// Probe again after this many milliseconds.
    RetryProbe(u32),
    Hint(DragHint),
    /// Create the strip if needed and park it, hidden, at this rect;
    /// `StripReady` follows once its page listens.
    PrepareStrip(Rect),
    ShowStrip,
    /// An explicit tray show bypasses the automatic fullscreen guard.
    ShowStripRequested,
    HidePlayer,
    /// Fly the strip between two rects; `FlightFinished` follows.
    StartFlight(Flight),
    PlaceStrip(Rect),
    /// Put the strip back above the taskbar after Explorer re-ordered it.
    Raise,
    HideStrip,
    ShowPlayer(Rect),
    StripPhase(StripPhase),
    DockedChanged(bool),
}

pub fn step(mut state: State, event: Event) -> (State, Vec<Effect>) {
    let effects = match event {
        Event::Settings(settings) => {
            state.settings = settings;
            let mut effects = Vec::new();
            if settings.docking {
                if !state.watching {
                    state.watching = true;
                    effects.push(Effect::InstallWatcher);
                }
            } else {
                effects = leave(&mut state);
                if state.watching {
                    state.watching = false;
                    state.taskbars = None;
                    effects.push(Effect::UninstallWatcher);
                }
            }
            effects
        }
        Event::PlayerFrame { frame, compact } => {
            state.frame = Some(frame);
            state.compact = compact;
            match &mut state.phase {
                Phase::Undocked { drag: Some(drag) } => {
                    drag.frame = frame;
                    refresh_hint(drag, compact)
                }
                _ => Vec::new(),
            }
        }
        Event::DragStarted => match (&state.phase, state.frame) {
            (Phase::Undocked { drag: None }, Some(frame))
                if state.settings.docking && state.compact =>
            {
                let mut drag = Drag {
                    frame,
                    started: frame,
                    taskbars: state.taskbars.clone(),
                    hint: DragHint::None,
                };
                let mut effects = vec![Effect::Probe];
                effects.extend(refresh_hint(&mut drag, true));
                state.phase = Phase::Undocked { drag: Some(drag) };
                effects
            }
            _ => Vec::new(),
        },
        Event::DragEnded { frame, compact } => {
            if let Some(compact) = compact {
                state.compact = compact;
            }
            if let Some(frame) = frame {
                state.frame = Some(frame);
            }
            match std::mem::replace(&mut state.phase, Phase::Undocked { drag: None }) {
                Phase::Undocked {
                    drag: Some(mut drag),
                } => {
                    drag.frame = frame.unwrap_or(drag.frame);
                    let mut effects = Vec::new();
                    if drag.hint != DragHint::None {
                        effects.push(Effect::Hint(DragHint::None));
                    }
                    // A resize loop ends with a different size; only moves dock.
                    let moved = drag.frame.width == drag.started.width
                        && drag.frame.height == drag.started.height;
                    let placement = (state.compact && moved)
                        .then(|| ready_placement(&drag))
                        .flatten();
                    if let Some(placement) = placement {
                        let motion = state.motion();
                        let flight = Flight {
                            from: drag.frame,
                            to: placement.strip,
                            motion,
                        };
                        // The player page feeds the strip from the first
                        // flight frame, not only once it has landed.
                        effects.push(Effect::DockedChanged(true));
                        effects.push(Effect::StripPhase(StripPhase::Docking { motion }));
                        effects.push(Effect::PrepareStrip(flight.start()));
                        state.phase = Phase::Docking {
                            placement,
                            home: drag.frame,
                            prepared: false,
                        };
                    }
                    effects
                }
                other => {
                    state.phase = other;
                    Vec::new()
                }
            }
        }
        Event::Taskbars(Some(taskbars)) => {
            state.taskbars = Some(taskbars.clone());
            let compact = state.compact;
            match &mut state.phase {
                Phase::Undocked { drag: Some(drag) } => {
                    drag.taskbars = Some(taskbars);
                    refresh_hint(drag, compact)
                }
                Phase::Docked {
                    placement,
                    visible,
                    covered,
                    show_pending,
                    failures,
                    ..
                } => {
                    *failures = 0;
                    let strip = taskbars
                        .iter()
                        .find(|taskbar| taskbar.id == placement.taskbar)
                        .and_then(Taskbar::strip_rect);
                    match strip {
                        Some(strip) => {
                            let moved = strip != placement.strip;
                            placement.strip = strip;
                            if *show_pending {
                                *show_pending = false;
                                *visible = true;
                                // A tray show is an explicit override, even
                                // while the last fullscreen sample is true.
                                *covered = false;
                                vec![
                                    Effect::PlaceStrip(strip),
                                    Effect::ShowStripRequested,
                                    Effect::StripPhase(StripPhase::Docked),
                                ]
                            } else if moved && *visible && !*covered {
                                vec![Effect::PlaceStrip(strip)]
                            } else {
                                Vec::new()
                            }
                        }
                        None => leave(&mut state),
                    }
                }
                _ => Vec::new(),
            }
        }
        // A failed probe proves nothing: keep the last placement and retry.
        Event::Taskbars(None) => {
            let compact = state.compact;
            // Only a dock in progress keeps trusting what it measured before.
            if !matches!(state.phase, Phase::Docked { .. }) {
                state.taskbars = None;
            }
            match &mut state.phase {
                Phase::Docked { failures, .. } => {
                    *failures = failures.saturating_add(1);
                    let index = usize::try_from(*failures - 1).unwrap_or(usize::MAX);
                    vec![Effect::RetryProbe(RETRY_MS[index.min(RETRY_MS.len() - 1)])]
                }
                // The cached geometry the drag started with is now suspect:
                // the bar may dock only on a measurement that succeeded.
                Phase::Undocked { drag: Some(drag) } => {
                    drag.taskbars = None;
                    refresh_hint(drag, compact)
                }
                _ => Vec::new(),
            }
        }
        Event::TaskbarChanged => match &state.phase {
            Phase::Docked {
                visible: true,
                covered: false,
                ..
            } => vec![Effect::Raise, Effect::Probe],
            Phase::Docked { .. } | Phase::Undocked { drag: Some(_) } => vec![Effect::Probe],
            // Keep the cache warm only while a dock is plausible.
            Phase::Undocked { drag: None }
                if state.settings.docking && state.compact && state.taskbars.is_some() =>
            {
                vec![Effect::Probe]
            }
            _ => Vec::new(),
        },
        Event::TaskbarRaised => {
            if state.strip_visible() {
                vec![Effect::Raise]
            } else {
                Vec::new()
            }
        }
        Event::Foreground { fullscreen } => {
            let changed = state.fullscreen != fullscreen;
            state.fullscreen = fullscreen;
            if changed {
                foreground(&mut state, fullscreen)
            } else {
                Vec::new()
            }
        }
        Event::ExplorerRestarted => {
            state.taskbars = None;
            match &mut state.phase {
                Phase::Undocked { drag: Some(drag) } => {
                    drag.taskbars = None;
                    let mut effects = refresh_hint(drag, false);
                    effects.push(Effect::Probe);
                    effects
                }
                Phase::Docked { failures, .. } => {
                    *failures = 0;
                    vec![Effect::Probe]
                }
                _ => Vec::new(),
            }
        }
        Event::StripReady => {
            let mut effects = vec![Effect::StripPhase(state.strip_phase())];
            let motion = state.motion();
            let fullscreen = state.fullscreen;
            if let Phase::Docking {
                placement,
                home,
                prepared: prepared @ false,
            } = &mut state.phase
            {
                *prepared = true;
                // Under a full-screen app the flight runs unseen and lands covered.
                if !fullscreen {
                    effects.push(Effect::ShowStrip);
                }
                effects.push(Effect::HidePlayer);
                effects.push(Effect::StartFlight(Flight {
                    from: *home,
                    to: placement.strip,
                    motion,
                }));
            }
            effects
        }
        Event::FlightFinished => {
            match std::mem::replace(&mut state.phase, Phase::Undocked { drag: None }) {
                Phase::Docking {
                    placement,
                    home,
                    prepared: true,
                } => {
                    let covered = state.fullscreen;
                    state.phase = Phase::Docked {
                        placement,
                        home,
                        visible: true,
                        covered,
                        show_pending: false,
                        failures: 0,
                    };
                    // The strip may have been kept hidden for a full-screen app
                    // that has since left, or still be up: match it to `covered`.
                    let mut effects = vec![Effect::StripPhase(StripPhase::Docked)];
                    effects.push(if covered {
                        Effect::HideStrip
                    } else {
                        Effect::ShowStrip
                    });
                    // The flight aimed at a cached measurement; confirm it.
                    effects.push(Effect::Probe);
                    effects
                }
                Phase::Undocking { home, show_player } => {
                    let mut effects = Vec::new();
                    if show_player {
                        effects.push(Effect::ShowPlayer(home));
                    }
                    effects.push(Effect::HideStrip);
                    effects.push(Effect::StripPhase(StripPhase::Hidden));
                    effects.push(Effect::DockedChanged(false));
                    effects
                }
                other => {
                    state.phase = other;
                    Vec::new()
                }
            }
        }
        Event::ShowRequested => match &mut state.phase {
            // Mid-flight the player is the only thing we can show for sure.
            Phase::Docking { .. } => abort(&mut state),
            Phase::Docked {
                visible: false,
                covered,
                show_pending,
                ..
            } => {
                *show_pending = true;
                *covered = false;
                vec![Effect::Probe]
            }
            Phase::Docked {
                visible: true,
                covered,
                ..
            } => {
                *covered = false;
                vec![Effect::ShowStripRequested]
            }
            Phase::Undocking { show_player, .. } => {
                *show_player = true;
                Vec::new()
            }
            Phase::Undocked { .. } => Vec::new(),
        },
        Event::HideRequested => hide(&mut state),
        Event::UndockRequested => leave(&mut state),
        Event::Abort => abort(&mut state),
    };
    (state, effects)
}

fn ready_placement(drag: &Drag) -> Option<Placement> {
    let taskbar = nearest(drag.taskbars.as_deref()?, drag.frame)?;
    taskbar.strip_rect().map(|strip| Placement {
        taskbar: taskbar.id,
        strip,
    })
}

fn refresh_hint(drag: &mut Drag, compact: bool) -> Vec<Effect> {
    let near = drag
        .taskbars
        .as_deref()
        .filter(|_| compact)
        .and_then(|taskbars| nearest(taskbars, drag.frame));
    let hint = match near {
        None => DragHint::None,
        Some(taskbar) if taskbar.strip_rect().is_some() => DragHint::Ready,
        Some(_) => DragHint::Unavailable,
    };
    if hint == drag.hint {
        return Vec::new();
    }
    drag.hint = hint;
    vec![Effect::Hint(hint)]
}

/// A full-screen app took or released the foreground: make the strip's
/// actual visibility match, in whatever phase the dock is.
fn foreground(state: &mut State, fullscreen: bool) -> Vec<Effect> {
    match std::mem::replace(&mut state.phase, Phase::Undocked { drag: None }) {
        Phase::Docked {
            placement,
            home,
            visible: true,
            covered,
            show_pending,
            failures,
        } => {
            let effects = if fullscreen && !covered {
                vec![Effect::HideStrip]
            } else if !fullscreen && covered {
                vec![
                    Effect::PlaceStrip(placement.strip),
                    Effect::ShowStrip,
                    Effect::StripPhase(StripPhase::Docked),
                ]
            } else if !fullscreen {
                vec![Effect::Raise]
            } else {
                Vec::new()
            };
            state.phase = Phase::Docked {
                placement,
                home,
                visible: true,
                covered: fullscreen,
                show_pending,
                failures,
            };
            effects
        }
        // Flying in: a full-screen app cuts the flight short and the strip
        // waits covered; one leaving lets the flight be seen from here on.
        Phase::Docking {
            placement,
            home,
            prepared: true,
        } => {
            if fullscreen {
                state.phase = Phase::Docked {
                    placement,
                    home,
                    visible: true,
                    covered: true,
                    show_pending: false,
                    failures: 0,
                };
                vec![Effect::HideStrip, Effect::StripPhase(StripPhase::Docked)]
            } else {
                state.phase = Phase::Docking {
                    placement,
                    home,
                    prepared: true,
                };
                vec![Effect::ShowStrip]
            }
        }
        // Flying out under a full-screen app: finish at once.
        Phase::Undocking { home, show_player } if fullscreen => {
            let mut effects = Vec::new();
            if show_player {
                effects.push(Effect::ShowPlayer(home));
            }
            effects.push(Effect::HideStrip);
            effects.push(Effect::StripPhase(StripPhase::Hidden));
            effects.push(Effect::DockedChanged(false));
            effects
        }
        other => {
            state.phase = other;
            Vec::new()
        }
    }
}

/// The user hid or minimized the player: whatever phase the dock is in,
/// nothing of it stays on screen and nothing comes back on its own.
fn hide(state: &mut State) -> Vec<Effect> {
    match std::mem::replace(&mut state.phase, Phase::Undocked { drag: None }) {
        Phase::Docked {
            placement,
            home,
            visible,
            covered,
            failures,
            ..
        } => {
            state.phase = Phase::Docked {
                placement,
                home,
                visible: false,
                covered: false,
                show_pending: false,
                failures,
            };
            if visible && !covered {
                vec![Effect::HideStrip]
            } else {
                Vec::new()
            }
        }
        // Mid flight-in: settle as docked-but-hidden; cancelling the flight
        // is the worker's side of `HideStrip`.
        Phase::Docking {
            placement,
            home,
            prepared,
        } => {
            state.phase = Phase::Docked {
                placement,
                home,
                visible: false,
                covered: false,
                show_pending: false,
                failures: 0,
            };
            if prepared {
                // The cancelled flight never lands, so settle the page here.
                vec![Effect::HideStrip, Effect::StripPhase(StripPhase::Docked)]
            } else {
                vec![
                    Effect::HidePlayer,
                    Effect::HideStrip,
                    Effect::StripPhase(StripPhase::Docked),
                ]
            }
        }
        // Mid flight-out: the player stays hidden instead of returning.
        Phase::Undocking { .. } => vec![
            Effect::HideStrip,
            Effect::StripPhase(StripPhase::Hidden),
            Effect::DockedChanged(false),
        ],
        other => {
            state.phase = other;
            Vec::new()
        }
    }
}

/// Leave the taskbar gracefully: fly back when the strip is visible, or just
/// drop out when it is hidden.
fn leave(state: &mut State) -> Vec<Effect> {
    match std::mem::replace(&mut state.phase, Phase::Undocked { drag: None }) {
        Phase::Docked {
            placement,
            home,
            visible: true,
            covered: false,
            ..
        } => {
            let motion = state.motion();
            state.phase = Phase::Undocking {
                home,
                show_player: true,
            };
            vec![
                Effect::StripPhase(StripPhase::Undocking { motion }),
                Effect::StartFlight(Flight {
                    from: placement.strip,
                    to: home,
                    motion,
                }),
            ]
        }
        Phase::Undocking { home, show_player } => {
            state.phase = Phase::Undocking { home, show_player };
            Vec::new()
        }
        // Hidden or covered strips have nothing to fly; a user who hid the
        // player while docked keeps it hidden unless a show was waiting.
        other => abort_effects(other),
    }
}

/// Drop the strip immediately and put the player back where it was.
fn abort(state: &mut State) -> Vec<Effect> {
    abort_effects(std::mem::replace(
        &mut state.phase,
        Phase::Undocked { drag: None },
    ))
}

fn abort_effects(phase: Phase) -> Vec<Effect> {
    match phase {
        Phase::Undocked { drag: Some(drag) } if drag.hint != DragHint::None => {
            vec![Effect::Hint(DragHint::None)]
        }
        Phase::Undocked { .. } => Vec::new(),
        Phase::Docking {
            home,
            prepared: true,
            ..
        }
        | Phase::Docked {
            home,
            visible: true,
            ..
        }
        | Phase::Docked {
            home,
            show_pending: true,
            ..
        }
        | Phase::Undocking {
            home,
            show_player: true,
        } => vec![
            Effect::ShowPlayer(home),
            Effect::HideStrip,
            Effect::StripPhase(StripPhase::Hidden),
            Effect::DockedChanged(false),
        ],
        Phase::Docking { .. } | Phase::Docked { .. } | Phase::Undocking { .. } => vec![
            Effect::HideStrip,
            Effect::StripPhase(StripPhase::Hidden),
            Effect::DockedChanged(false),
        ],
    }
}

/// CSS `cubic-bezier(.77, 0, .175, 1)` for the flight.
pub fn ease(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    let bezier = |t: f64, first: f64, second: f64| {
        let inverse = 1.0 - t;
        3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
    };
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if bezier(middle, 0.77, 0.175) < progress {
            low = middle;
        } else {
            high = middle;
        }
    }
    bezier((low + high) / 2.0, 0.0, 1.0)
}

pub fn interpolate(from: Rect, to: Rect, progress: f64) -> Rect {
    let eased = ease(progress);
    let lerp = |start: i32, end: i32| {
        (f64::from(start) + (f64::from(end) - f64::from(start)) * eased).round() as i32
    };
    Rect::new(
        lerp(from.x, to.x),
        lerp(from.y, to.y),
        lerp(from.width, to.width),
        lerp(from.height, to.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR_ID: usize = 0x1_0000;
    const STRIP: Rect = Rect::new(1256, 1032, 380, 48);

    /// A 1920x1080 display with a 48px Windows 11 taskbar: centred buttons,
    /// widgets at the far left, tray on the right.
    fn win11_bar() -> Taskbar {
        Taskbar {
            id: BAR_ID,
            bounds: Rect::new(0, 1032, 1920, 48),
            dpi: 96,
            container: Rect::new(0, 1032, 1920, 48),
            content: Some(Rect::new(760, 1032, 400, 48)),
            widgets: Some(Rect::new(0, 1032, 160, 48)),
            tray: Some(Rect::new(1640, 1032, 280, 48)),
            auto_hide: false,
        }
    }

    fn bar_frame(y: i32) -> Rect {
        Rect::new(600, y, 593, 60)
    }

    fn frame(frame: Rect, compact: bool) -> Event {
        Event::PlayerFrame { frame, compact }
    }

    fn released(frame: Rect) -> Event {
        Event::DragEnded {
            frame: Some(frame),
            compact: Some(true),
        }
    }

    fn run(mut state: State, events: Vec<Event>) -> (State, Vec<Vec<Effect>>) {
        let mut trace = Vec::new();
        for event in events {
            let (next, effects) = step(state, event);
            trace.push(effects);
            state = next;
        }
        (state, trace)
    }

    fn docking_state() -> State {
        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(900), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                released(bar_frame(1000)),
            ],
        );
        assert!(matches!(
            state.phase,
            Phase::Docking {
                prepared: false,
                ..
            }
        ));
        assert_eq!(
            trace[3].last(),
            Some(&Effect::PrepareStrip(bar_frame(1000)))
        );
        state
    }

    fn docked_state() -> State {
        let (state, trace) = run(
            docking_state(),
            vec![Event::StripReady, Event::FlightFinished],
        );
        assert_eq!(
            trace[1],
            vec![
                Effect::StripPhase(StripPhase::Docked),
                Effect::ShowStrip,
                Effect::Probe
            ]
        );
        assert!(state.strip_visible());
        state
    }

    fn abort_with_player(home: Rect) -> Vec<Effect> {
        vec![
            Effect::ShowPlayer(home),
            Effect::HideStrip,
            Effect::StripPhase(StripPhase::Hidden),
            Effect::DockedChanged(false),
        ]
    }

    #[test]
    fn the_strip_sits_at_the_tray_end_of_the_free_band() {
        let bar = win11_bar();
        assert_eq!(bar.free_band(), Some(Rect::new(1160, 1032, 480, 48)));
        assert_eq!(bar.strip_rect(), Some(STRIP));

        let scaled = Taskbar {
            dpi: 144,
            ..win11_bar()
        };
        assert_eq!(scaled.strip_rect(), None, "570px does not fit in 480px");
        let roomy = Taskbar {
            content: Some(Rect::new(500, 1032, 400, 48)),
            ..scaled
        };
        assert_eq!(roomy.strip_rect(), Some(Rect::new(1064, 1032, 570, 48)));
    }

    #[test]
    fn widgets_only_shrink_the_side_they_are_on() {
        let right_widgets = Taskbar {
            widgets: Some(Rect::new(1500, 1032, 48, 48)),
            ..win11_bar()
        };
        assert_eq!(
            right_widgets.free_band(),
            Some(Rect::new(1160, 1032, 340, 48))
        );

        let legacy = Taskbar {
            container: Rect::new(300, 1032, 1340, 48),
            content: Some(Rect::new(300, 1032, 200, 48)),
            widgets: None,
            ..win11_bar()
        };
        assert_eq!(legacy.free_band(), Some(Rect::new(500, 1032, 1140, 48)));

        let empty = Taskbar {
            content: None,
            ..win11_bar()
        };
        assert_eq!(empty.free_band(), Some(Rect::new(160, 1032, 1480, 48)));
    }

    #[test]
    fn vertical_and_auto_hidden_taskbars_never_take_the_strip() {
        let vertical = Taskbar {
            bounds: Rect::new(0, 0, 62, 1080),
            container: Rect::new(0, 0, 62, 1080),
            ..win11_bar()
        };
        assert_eq!(vertical.strip_rect(), None);
        let hidden = Taskbar {
            auto_hide: true,
            ..win11_bar()
        };
        assert_eq!(hidden.strip_rect(), None);
        assert!(hidden.free_band().is_some(), "only the strip is refused");
    }

    #[test]
    fn nearness_scales_with_the_taskbar_dpi() {
        let bar = win11_bar();
        assert!(bar.is_near(bar_frame(1032 - 60 - 40)));
        assert!(!bar.is_near(bar_frame(1032 - 60 - 41)));
        assert!(!bar.is_near(Rect::new(2000, 1000, 593, 60)), "no overlap");

        let scaled = Taskbar {
            dpi: 144,
            ..win11_bar()
        };
        assert!(scaled.is_near(bar_frame(1032 - 60 - 60)));
        assert!(!scaled.is_near(bar_frame(1032 - 60 - 61)));
    }

    #[test]
    fn the_nearest_taskbar_wins_by_overlap_then_by_gap() {
        let left = win11_bar();
        let right = Taskbar {
            id: BAR_ID + 1,
            bounds: Rect::new(1920, 1032, 1920, 48),
            container: Rect::new(1920, 1032, 1920, 48),
            content: Some(Rect::new(2680, 1032, 400, 48)),
            widgets: None,
            tray: None,
            ..win11_bar()
        };
        let bars = vec![left.clone(), right.clone()];
        let across = Rect::new(1700, 1000, 593, 60);
        assert_eq!(nearest(&bars, across).map(|bar| bar.id), Some(right.id));
        let mostly_left = Rect::new(1500, 1000, 593, 60);
        assert_eq!(nearest(&bars, mostly_left).map(|bar| bar.id), Some(left.id));

        // Same overlap: the closer bar.
        let stacked = Taskbar {
            id: BAR_ID + 2,
            bounds: Rect::new(0, 1100, 1920, 48),
            container: Rect::new(0, 1100, 1920, 48),
            ..win11_bar()
        };
        let bars = vec![stacked.clone(), left.clone()];
        assert_eq!(
            nearest(&bars, bar_frame(1090)).map(|bar| bar.id),
            Some(stacked.id)
        );
        assert_eq!(
            nearest(&bars, bar_frame(960)).map(|bar| bar.id),
            Some(left.id)
        );
    }

    #[test]
    fn releasing_near_the_taskbar_prepares_the_strip_then_flies_on_ready() {
        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(500), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                frame(bar_frame(1000), true),
                released(bar_frame(1000)),
                Event::StripReady,
                Event::FlightFinished,
            ],
        );
        assert_eq!(trace[1], vec![Effect::Probe]);
        assert_eq!(trace[2], vec![], "far from the bar: no hint");
        assert_eq!(trace[3], vec![Effect::Hint(DragHint::Ready)]);
        assert_eq!(
            trace[4],
            vec![
                Effect::Hint(DragHint::None),
                Effect::DockedChanged(true),
                Effect::StripPhase(StripPhase::Docking {
                    motion: Motion::Fly
                }),
                Effect::PrepareStrip(bar_frame(1000)),
            ]
        );
        assert_eq!(
            trace[5],
            vec![
                Effect::StripPhase(StripPhase::Docking {
                    motion: Motion::Fly
                }),
                Effect::ShowStrip,
                Effect::HidePlayer,
                Effect::StartFlight(Flight {
                    from: bar_frame(1000),
                    to: STRIP,
                    motion: Motion::Fly,
                }),
            ]
        );
        assert_eq!(
            trace[6],
            vec![
                Effect::StripPhase(StripPhase::Docked),
                Effect::ShowStrip,
                Effect::Probe
            ]
        );
        assert!(matches!(state.phase, Phase::Docked { home, .. } if home == bar_frame(1000)));
    }

    #[test]
    fn the_release_frame_from_the_hook_wins_over_stale_window_events() {
        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(500), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                released(bar_frame(1000)),
            ],
        );
        assert!(matches!(state.phase, Phase::Docking { home, .. } if home == bar_frame(1000)));
        assert_eq!(trace[3][0], Effect::DockedChanged(true));

        let (state, _) = run(
            State::default(),
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                Event::DragEnded {
                    frame: None,
                    compact: None,
                },
            ],
        );
        assert!(
            matches!(state.phase, Phase::Docking { .. }),
            "falls back to the last frame"
        );
    }

    #[test]
    fn releasing_away_from_the_taskbar_keeps_the_bar_floating() {
        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(500), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                released(bar_frame(500)),
            ],
        );
        assert_eq!(trace[3], vec![]);
        assert_eq!(state.phase, Phase::Undocked { drag: None });
    }

    #[test]
    fn resizing_or_losing_the_bar_layout_never_docks() {
        // Dragging the top edge up: the window ends taller and non-compact.
        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                frame(Rect::new(600, 600, 593, 460), false),
                Event::DragEnded {
                    frame: Some(Rect::new(600, 600, 593, 460)),
                    compact: Some(false),
                },
            ],
        );
        assert_eq!(trace[2], vec![Effect::Hint(DragHint::Ready)]);
        assert_eq!(trace[3], vec![Effect::Hint(DragHint::None)]);
        assert_eq!(trace[4], vec![]);
        assert_eq!(state.phase, Phase::Undocked { drag: None });

        // Still compact, still near, but a different size: a resize loop.
        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                released(Rect::new(600, 1000, 700, 60)),
            ],
        );
        assert_eq!(trace[3], vec![Effect::Hint(DragHint::None)]);
        assert_eq!(state.phase, Phase::Undocked { drag: None });
    }

    #[test]
    fn drags_need_the_setting_and_a_compact_window() {
        let off = State {
            settings: Settings {
                docking: false,
                animations: true,
                reduced_motion: false,
            },
            ..State::default()
        };
        let (_, trace) = run(off, vec![frame(bar_frame(1000), true), Event::DragStarted]);
        assert_eq!(trace[1], vec![]);

        let (_, trace) = run(
            State::default(),
            vec![frame(Rect::new(0, 0, 920, 620), false), Event::DragStarted],
        );
        assert_eq!(trace[1], vec![]);
    }

    #[test]
    fn a_narrow_or_failed_probe_explains_instead_of_docking() {
        let narrow = Taskbar {
            tray: Some(Rect::new(1300, 1032, 620, 48)),
            ..win11_bar()
        };
        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![narrow])),
                released(bar_frame(1000)),
            ],
        );
        assert_eq!(trace[2], vec![Effect::Hint(DragHint::Unavailable)]);
        assert_eq!(trace[3], vec![Effect::Hint(DragHint::None)]);
        assert_eq!(state.phase, Phase::Undocked { drag: None });

        let (_, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(None),
                released(bar_frame(1000)),
            ],
        );
        assert_eq!(trace[2], vec![], "a failed probe proves nothing");
        assert_eq!(trace[3], vec![]);
    }

    #[test]
    fn the_last_measurement_seeds_the_next_drag() {
        let (undocked, _) = run(docked_state(), vec![Event::Abort]);
        assert!(undocked.taskbars.is_some());
        let (_, trace) = run(
            undocked,
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                released(bar_frame(1000)),
            ],
        );
        assert_eq!(
            trace[1],
            vec![Effect::Probe, Effect::Hint(DragHint::Ready)],
            "the hint shows before the probe answers"
        );
        assert_eq!(trace[2][0], Effect::Hint(DragHint::None));
        assert!(matches!(trace[2].last(), Some(Effect::PrepareStrip(_))));

        // A probe that fails during the drag withdraws the cached offer.
        let (state, trace) = run(
            undocked_with_cache(),
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(None),
                released(bar_frame(1000)),
            ],
        );
        assert_eq!(trace[2], vec![Effect::Hint(DragHint::None)]);
        assert_eq!(trace[3], vec![]);
        assert_eq!(state.phase, Phase::Undocked { drag: None });
        assert_eq!(state.taskbars, None, "the next drag starts blind too");

        // The cache is refreshed only while the bar is compact and docking is on.
        let (compact, trace) = run(
            undocked_with_cache(),
            vec![frame(bar_frame(500), true), Event::TaskbarChanged],
        );
        assert_eq!(trace[1], vec![Effect::Probe]);
        let (_, trace) = run(
            compact,
            vec![
                frame(Rect::new(0, 0, 920, 620), false),
                Event::TaskbarChanged,
            ],
        );
        assert_eq!(trace[1], vec![]);
        let (_, trace) = run(State::default(), vec![Event::TaskbarChanged]);
        assert_eq!(trace[0], vec![], "never probe for users who do not dock");
    }

    fn undocked_with_cache() -> State {
        let (state, _) = run(docked_state(), vec![Event::Abort]);
        state
    }

    #[test]
    fn animations_off_fades_and_reduced_motion_is_instant() {
        let fade = State {
            settings: Settings {
                docking: true,
                animations: false,
                reduced_motion: false,
            },
            ..State::default()
        };
        let (_, trace) = run(
            fade,
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                released(bar_frame(1000)),
                Event::StripReady,
            ],
        );
        assert_eq!(
            trace[3][2],
            Effect::StripPhase(StripPhase::Docking {
                motion: Motion::Fade
            })
        );
        assert_eq!(
            trace[3][3],
            Effect::PrepareStrip(STRIP),
            "fades start in place"
        );
        assert!(matches!(
            trace[4].last(),
            Some(Effect::StartFlight(Flight {
                motion: Motion::Fade,
                ..
            }))
        ));

        let reduced = State {
            settings: Settings {
                docking: true,
                animations: true,
                reduced_motion: true,
            },
            ..State::default()
        };
        let (_, trace) = run(
            reduced,
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                released(bar_frame(1000)),
            ],
        );
        assert_eq!(
            trace[3][2],
            Effect::StripPhase(StripPhase::Docking {
                motion: Motion::Instant
            })
        );
    }

    #[test]
    fn a_moved_taskbar_moves_the_strip_and_a_lost_one_undocks() {
        let moved = Taskbar {
            bounds: Rect::new(0, 1020, 1920, 60),
            container: Rect::new(0, 1020, 1920, 60),
            ..win11_bar()
        };
        let (state, trace) = run(
            docked_state(),
            vec![
                Event::TaskbarChanged,
                Event::Taskbars(Some(vec![moved.clone()])),
                Event::Taskbars(Some(vec![moved])),
            ],
        );
        assert_eq!(trace[0], vec![Effect::Raise, Effect::Probe]);
        assert_eq!(
            trace[1],
            vec![Effect::PlaceStrip(Rect::new(1256, 1020, 380, 60))]
        );
        assert_eq!(trace[2], vec![], "unchanged geometry is left alone");
        assert!(matches!(state.phase, Phase::Docked { .. }));

        for lost in [
            vec![Taskbar {
                id: BAR_ID + 1,
                ..win11_bar()
            }],
            vec![Taskbar {
                auto_hide: true,
                ..win11_bar()
            }],
        ] {
            let (state, trace) = run(docked_state(), vec![Event::Taskbars(Some(lost))]);
            assert_eq!(
                trace[0],
                vec![
                    Effect::StripPhase(StripPhase::Undocking {
                        motion: Motion::Fly
                    }),
                    Effect::StartFlight(Flight {
                        from: STRIP,
                        to: bar_frame(1000),
                        motion: Motion::Fly,
                    }),
                ]
            );
            let (state, trace) = run(state, vec![Event::FlightFinished]);
            assert_eq!(trace[0], abort_with_player(bar_frame(1000)));
            assert_eq!(state.phase, Phase::Undocked { drag: None });
        }
    }

    #[test]
    fn failed_probes_while_docked_keep_the_strip_and_back_off() {
        let (state, trace) = run(
            docked_state(),
            vec![
                Event::Taskbars(None),
                Event::Taskbars(None),
                Event::Taskbars(None),
                Event::Taskbars(None),
                Event::Taskbars(Some(vec![win11_bar()])),
                Event::Taskbars(None),
            ],
        );
        assert_eq!(
            &trace[..4],
            &[
                vec![Effect::RetryProbe(150)],
                vec![Effect::RetryProbe(1000)],
                vec![Effect::RetryProbe(5000)],
                vec![Effect::RetryProbe(5000)],
            ]
        );
        assert_eq!(trace[4], vec![], "the strip never moved");
        assert_eq!(trace[5], vec![Effect::RetryProbe(150)], "success resets");
        assert!(state.strip_visible());
        assert!(state.taskbars.is_some(), "docked keeps its measurement");

        // Hidden to the tray with a show waiting: the strip is not shown on
        // a failure, and not torn down either.
        let (state, trace) = run(
            docked_state(),
            vec![
                Event::HideRequested,
                Event::ShowRequested,
                Event::Taskbars(None),
            ],
        );
        assert_eq!(trace[2], vec![Effect::RetryProbe(150)]);
        assert!(matches!(
            state.phase,
            Phase::Docked {
                visible: false,
                show_pending: true,
                ..
            }
        ));
    }

    #[test]
    fn the_setting_installs_and_removes_the_watcher() {
        let on = Event::Settings(Settings {
            docking: true,
            animations: true,
            reduced_motion: false,
        });
        let off = Event::Settings(Settings {
            docking: false,
            animations: true,
            reduced_motion: false,
        });
        // Nothing watches until the renderer says docking is on, even
        // though the default setting is on.
        let (state, trace) = run(
            State::default(),
            vec![on.clone(), on.clone(), off.clone(), off.clone(), on.clone()],
        );
        assert_eq!(
            trace,
            vec![
                vec![Effect::InstallWatcher],
                vec![],
                vec![Effect::UninstallWatcher],
                vec![],
                vec![Effect::InstallWatcher],
            ]
        );
        assert!(state.watching);

        // Turning it off while docked flies home first, then tears down
        // and forgets the measurements.
        let (watched, _) = run(docked_state(), vec![on]);
        let (state, trace) = run(watched, vec![off]);
        assert_eq!(
            trace[0],
            vec![
                Effect::StripPhase(StripPhase::Undocking {
                    motion: Motion::Fly
                }),
                Effect::StartFlight(Flight {
                    from: STRIP,
                    to: bar_frame(1000),
                    motion: Motion::Fly,
                }),
                Effect::UninstallWatcher,
            ]
        );
        assert!(!state.watching);
        assert_eq!(state.taskbars, None);
    }

    #[test]
    fn the_return_button_and_the_setting_both_undock() {
        for event in [
            Event::UndockRequested,
            Event::Settings(Settings {
                docking: false,
                animations: true,
                reduced_motion: false,
            }),
        ] {
            let (state, trace) = run(docked_state(), vec![event]);
            assert!(matches!(trace[0][1], Effect::StartFlight(_)));
            assert!(matches!(
                state.phase,
                Phase::Undocking {
                    show_player: true,
                    ..
                }
            ));
        }
    }

    #[test]
    fn hiding_while_docked_hides_the_strip_and_showing_re_probes() {
        let (state, trace) = run(
            docked_state(),
            vec![
                Event::HideRequested,
                Event::HideRequested,
                Event::TaskbarChanged,
                Event::ShowRequested,
                Event::Taskbars(Some(vec![win11_bar()])),
                Event::ShowRequested,
            ],
        );
        assert_eq!(trace[0], vec![Effect::HideStrip]);
        assert_eq!(trace[1], vec![]);
        assert_eq!(
            trace[2],
            vec![Effect::Probe],
            "hidden strips are not raised"
        );
        assert_eq!(trace[3], vec![Effect::Probe]);
        assert_eq!(
            trace[4],
            vec![
                Effect::PlaceStrip(STRIP),
                Effect::ShowStripRequested,
                Effect::StripPhase(StripPhase::Docked)
            ],
            "a strip shown again is told its phase again"
        );
        assert_eq!(trace[5], vec![Effect::ShowStripRequested]);
        assert!(state.strip_visible());
    }

    #[test]
    fn a_show_request_the_strip_cannot_honour_brings_the_player_back() {
        let gone = vec![Taskbar {
            auto_hide: true,
            ..win11_bar()
        }];
        let (state, trace) = run(
            docked_state(),
            vec![
                Event::HideRequested,
                Event::ShowRequested,
                Event::Taskbars(Some(gone.clone())),
            ],
        );
        assert_eq!(trace[2], abort_with_player(bar_frame(1000)));
        assert_eq!(state.phase, Phase::Undocked { drag: None });

        // Hidden by the user and the taskbar vanished: stay hidden.
        let (_, trace) = run(
            docked_state(),
            vec![Event::HideRequested, Event::Taskbars(Some(gone))],
        );
        assert_eq!(
            trace[1],
            vec![
                Effect::HideStrip,
                Effect::StripPhase(StripPhase::Hidden),
                Effect::DockedChanged(false),
            ]
        );

        // Asking for the player mid-flight hands it back at once.
        let (prepared, _) = run(docking_state(), vec![Event::StripReady]);
        let (state, trace) = run(prepared, vec![Event::ShowRequested]);
        assert_eq!(trace[0], abort_with_player(bar_frame(1000)));
        assert_eq!(state.phase, Phase::Undocked { drag: None });
    }

    #[test]
    fn full_screen_apps_cover_the_strip_until_they_leave() {
        let (state, trace) = run(
            docked_state(),
            vec![
                Event::Foreground { fullscreen: true },
                Event::Foreground { fullscreen: true },
                Event::TaskbarChanged,
                Event::TaskbarRaised,
                Event::Foreground { fullscreen: false },
                Event::Foreground { fullscreen: false },
                Event::TaskbarRaised,
            ],
        );
        assert_eq!(trace[0], vec![Effect::HideStrip]);
        assert_eq!(trace[1], vec![]);
        assert_eq!(
            trace[2],
            vec![Effect::Probe],
            "covered strips are not raised"
        );
        assert_eq!(trace[3], vec![]);
        assert_eq!(
            trace[4],
            vec![
                Effect::PlaceStrip(STRIP),
                Effect::ShowStrip,
                Effect::StripPhase(StripPhase::Docked)
            ]
        );
        assert_eq!(trace[5], vec![]);
        assert_eq!(trace[6], vec![Effect::Raise]);
        assert!(state.strip_visible());

        // An explicit show request restores the original tray priority.
        let (covered, _) = run(docked_state(), vec![Event::Foreground { fullscreen: true }]);
        let (shown, trace) = run(covered.clone(), vec![Event::ShowRequested]);
        assert_eq!(trace[0], vec![Effect::ShowStripRequested,]);
        assert!(shown.strip_visible());
        let (shown, trace) = run(shown, vec![Event::ShowRequested]);
        assert_eq!(trace[0], vec![Effect::ShowStripRequested]);
        assert!(shown.strip_visible());
        let (shown, trace) = run(shown, vec![Event::TaskbarRaised]);
        assert_eq!(trace[0], vec![Effect::Raise]);
        assert!(shown.strip_visible());
        let (shown, trace) = run(shown, vec![Event::Foreground { fullscreen: true }]);
        assert_eq!(
            trace[0],
            vec![],
            "duplicate samples preserve the tray override"
        );
        assert!(shown.strip_visible());
        let (shown, trace) = run(
            shown,
            vec![
                Event::Foreground { fullscreen: false },
                Event::Foreground { fullscreen: true },
            ],
        );
        assert_eq!(trace[0], vec![Effect::Raise]);
        assert_eq!(trace[1], vec![Effect::HideStrip]);
        assert!(!shown.strip_visible());
        let (state, trace) = run(covered.clone(), vec![Event::UndockRequested]);
        assert_eq!(trace[0], abort_with_player(bar_frame(1000)));
        assert_eq!(state.phase, Phase::Undocked { drag: None });
        let (_, trace) = run(covered, vec![Event::HideRequested]);
        assert_eq!(trace[0], vec![], "already off screen");
    }

    #[test]
    fn show_requested_while_hidden_waits_for_probe_then_overrides_fullscreen() {
        let (hidden, _) = run(docked_state(), vec![Event::HideRequested]);
        let (pending, trace) = run(
            hidden,
            vec![Event::Foreground { fullscreen: true }, Event::ShowRequested],
        );
        assert_eq!(trace[1], vec![Effect::Probe]);
        assert!(matches!(
            pending.phase,
            Phase::Docked {
                visible: false,
                show_pending: true,
                ..
            }
        ));

        let (shown, trace) = run(pending, vec![Event::Taskbars(Some(vec![win11_bar()]))]);
        assert_eq!(
            trace[0],
            vec![
                Effect::PlaceStrip(STRIP),
                Effect::ShowStripRequested,
                Effect::StripPhase(StripPhase::Docked),
            ]
        );
        assert!(shown.strip_visible());
    }

    #[test]
    fn full_screen_during_a_flight_lands_the_strip_covered() {
        let (state, trace) = run(
            docking_state(),
            vec![
                Event::Foreground { fullscreen: true },
                Event::StripReady,
                Event::FlightFinished,
                Event::Foreground { fullscreen: false },
            ],
        );
        assert_eq!(trace[0], vec![]);
        assert_eq!(
            trace[1],
            vec![
                Effect::StripPhase(StripPhase::Docking {
                    motion: Motion::Fly
                }),
                Effect::HidePlayer,
                Effect::StartFlight(Flight {
                    from: bar_frame(1000),
                    to: STRIP,
                    motion: Motion::Fly,
                }),
            ],
            "no ShowStrip over a full-screen app"
        );
        assert_eq!(
            trace[2],
            vec![
                Effect::StripPhase(StripPhase::Docked),
                Effect::HideStrip,
                Effect::Probe
            ]
        );
        assert_eq!(
            trace[3],
            vec![
                Effect::PlaceStrip(STRIP),
                Effect::ShowStrip,
                Effect::StripPhase(StripPhase::Docked)
            ]
        );
        assert!(state.strip_visible());

        // The app leaves before the flight lands: the strip is shown from
        // there on, and landing shows it again rather than only raising.
        let (state, trace) = run(
            docking_state(),
            vec![
                Event::Foreground { fullscreen: true },
                Event::StripReady,
                Event::Foreground { fullscreen: false },
                Event::FlightFinished,
                Event::ShowRequested,
            ],
        );
        assert_eq!(trace[2], vec![Effect::ShowStrip]);
        assert_eq!(
            trace[3],
            vec![
                Effect::StripPhase(StripPhase::Docked),
                Effect::ShowStrip,
                Effect::Probe
            ]
        );
        assert_eq!(trace[4], vec![Effect::ShowStripRequested]);
        assert!(state.strip_visible());

        // The app arrives mid-flight: the flight is cut short, covered.
        let (prepared, _) = run(docking_state(), vec![Event::StripReady]);
        let (state, trace) = run(prepared, vec![Event::Foreground { fullscreen: true }]);
        assert_eq!(
            trace[0],
            vec![Effect::HideStrip, Effect::StripPhase(StripPhase::Docked)]
        );
        assert!(matches!(
            state.phase,
            Phase::Docked {
                visible: true,
                covered: true,
                ..
            }
        ));

        // Flying out when one arrives: finish right there.
        let (undocking, _) = run(docked_state(), vec![Event::UndockRequested]);
        let (state, trace) = run(undocking, vec![Event::Foreground { fullscreen: true }]);
        assert_eq!(trace[0], abort_with_player(bar_frame(1000)));
        assert_eq!(state.phase, Phase::Undocked { drag: None });

        // A tray show explicitly reveals the strip, even while fullscreen.
        let (state, trace) = run(
            docked_state(),
            vec![
                Event::HideRequested,
                Event::Foreground { fullscreen: true },
                Event::ShowRequested,
                Event::Taskbars(Some(vec![win11_bar()])),
                Event::Foreground { fullscreen: false },
            ],
        );
        assert_eq!(
            trace[3],
            vec![
                Effect::PlaceStrip(STRIP),
                Effect::ShowStripRequested,
                Effect::StripPhase(StripPhase::Docked)
            ]
        );
        assert_eq!(trace[4], vec![Effect::Raise]);
        assert!(state.strip_visible());
    }

    #[test]
    fn hiding_during_a_flight_settles_hidden_instead_of_being_lost() {
        // Waiting for the strip page: the player is still showing.
        let (state, trace) = run(
            docking_state(),
            vec![
                Event::HideRequested,
                Event::StripReady,
                Event::FlightFinished,
            ],
        );
        assert_eq!(
            trace[0],
            vec![
                Effect::HidePlayer,
                Effect::HideStrip,
                Effect::StripPhase(StripPhase::Docked),
            ]
        );
        assert_eq!(trace[1], vec![Effect::StripPhase(StripPhase::Docked)]);
        assert_eq!(trace[2], vec![], "no flight was started");
        assert!(matches!(state.phase, Phase::Docked { visible: false, .. }));

        // Flying in: the strip is cancelled and hidden where it is.
        let (prepared, _) = run(docking_state(), vec![Event::StripReady]);
        let (state, trace) = run(prepared, vec![Event::HideRequested]);
        assert_eq!(
            trace[0],
            vec![Effect::HideStrip, Effect::StripPhase(StripPhase::Docked)],
            "the page must not stay in its docking phase"
        );
        assert!(matches!(state.phase, Phase::Docked { visible: false, .. }));

        // Flying out: the player does not come back after all.
        let (undocking, _) = run(docked_state(), vec![Event::UndockRequested]);
        let (state, trace) = run(undocking, vec![Event::HideRequested, Event::FlightFinished]);
        assert_eq!(
            trace[0],
            vec![
                Effect::HideStrip,
                Effect::StripPhase(StripPhase::Hidden),
                Effect::DockedChanged(false),
            ]
        );
        assert_eq!(trace[1], vec![]);
        assert_eq!(state.phase, Phase::Undocked { drag: None });
    }

    #[test]
    fn explorer_restarts_drop_every_measurement() {
        let (state, trace) = run(docked_state(), vec![Event::ExplorerRestarted]);
        assert_eq!(trace[0], vec![Effect::Probe]);
        assert_eq!(state.taskbars, None);

        let (state, trace) = run(
            State::default(),
            vec![
                frame(bar_frame(1000), true),
                Event::DragStarted,
                Event::Taskbars(Some(vec![win11_bar()])),
                Event::ExplorerRestarted,
                released(bar_frame(1000)),
            ],
        );
        assert_eq!(trace[3], vec![Effect::Hint(DragHint::None), Effect::Probe]);
        assert_eq!(trace[4], vec![], "nothing known: no dock");
        assert_eq!(state.phase, Phase::Undocked { drag: None });
    }

    #[test]
    fn abort_drops_the_strip_from_every_busy_phase() {
        let (prepared, _) = run(docking_state(), vec![Event::StripReady]);
        let (undocking, _) = run(docked_state(), vec![Event::UndockRequested]);
        for busy in [prepared, docked_state(), undocking] {
            let (state, trace) = run(busy, vec![Event::Abort]);
            assert_eq!(trace[0], abort_with_player(bar_frame(1000)));
            assert_eq!(state.phase, Phase::Undocked { drag: None });
        }

        // Before the strip page listened the player was never hidden.
        let (state, trace) = run(docking_state(), vec![Event::Abort]);
        assert_eq!(
            trace[0],
            vec![
                Effect::HideStrip,
                Effect::StripPhase(StripPhase::Hidden),
                Effect::DockedChanged(false),
            ]
        );
        assert_eq!(state.phase, Phase::Undocked { drag: None });

        // A ready or landing signal from a flight that was aborted is noise.
        let (state, trace) = run(state, vec![Event::StripReady, Event::FlightFinished]);
        assert_eq!(trace[0], vec![Effect::StripPhase(StripPhase::Hidden)]);
        assert_eq!(trace[1], vec![]);
        assert_eq!(state.phase, Phase::Undocked { drag: None });
        let (_, trace) = run(State::default(), vec![Event::Abort]);
        assert_eq!(trace[0], vec![]);
    }

    #[test]
    fn a_repeated_ready_signal_only_replays_the_phase() {
        let (state, trace) = run(
            docking_state(),
            vec![Event::StripReady, Event::StripReady, Event::FlightFinished],
        );
        assert_eq!(trace[0].len(), 4);
        assert_eq!(
            trace[1],
            vec![Effect::StripPhase(StripPhase::Docking {
                motion: Motion::Fly
            })]
        );
        assert!(state.strip_visible());
        let (_, trace) = run(state, vec![Event::StripReady]);
        assert_eq!(trace[0], vec![Effect::StripPhase(StripPhase::Docked)]);
    }

    #[test]
    fn flights_follow_the_curve_and_land_exactly() {
        let from = Rect::new(100, 200, 900, 150);
        let to = Rect::new(700, 1_000, 380, 48);
        assert_eq!(interpolate(from, to, 0.0), from);
        assert_eq!(interpolate(from, to, 1.0), to);
        assert!(ease(0.25) < 0.25 && ease(0.75) > 0.75);
        assert_eq!(dip(380, 144), 570);
        assert_eq!(dip(4, 120), 5);
        assert_eq!(
            Rect::new(-10, 1000, 100, 100).clip(Rect::new(0, 1032, 1920, 48)),
            Some(Rect::new(0, 1032, 90, 48))
        );
        assert_eq!(Rect::new(0, 0, 10, 10).clip(Rect::new(20, 20, 5, 5)), None);
    }
}
