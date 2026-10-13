#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForegroundContext {
    pub hwnd: u64,
    pub bounds: Rect,
}

pub fn foreground_context_changed(
    previous: Option<ForegroundContext>,
    current: Option<ForegroundContext>,
) -> bool {
    !matches!((previous, current), (Some(previous), Some(current)) if previous == current)
}

/// Allows a narrow steady-poll exception for a moving foreground window.
/// Callers must still perform the normal fresh safety probe before showing the overlay.
pub fn can_skip_steady_foreground_retry(
    guard: SteadyPollGuard,
    previous: Option<ForegroundContext>,
    current: Option<ForegroundContext>,
    monitor: Rect,
) -> bool {
    if !guard.selected_bar_matches
        || !guard.attached
        || guard.transitioning
        || !guard.inner_requested_visible
        || !guard.latest_requested_visible
        || !guard.probe_complete
        || guard.safety != ForegroundSafety::Clear
        || !monitor.is_valid()
    {
        return false;
    }

    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    if previous.hwnd == 0
        || previous.hwnd != current.hwnd
        || !previous.bounds.is_valid()
        || !current.bounds.is_valid()
        || previous.bounds.width != current.bounds.width
        || previous.bounds.height != current.bounds.height
        || (previous.bounds.x == current.bounds.x && previous.bounds.y == current.bounds.y)
    {
        return false;
    }

    current.bounds.width < monitor.width || current.bounds.height < monitor.height
}

pub fn taskbar_snap_ease(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    let axis = |t: f64, first: f64, second: f64| {
        let inverse = 1.0 - t;
        3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if axis(middle, 0.77, 0.175) < progress {
            low = middle;
        } else {
            high = middle;
        }
    }
    axis((low + high) / 2.0, 0.0, 1.0)
}

pub fn interpolate_rect(from: Rect, to: Rect, progress: f64) -> Option<Rect> {
    if !from.is_valid() || !to.is_valid() || !progress.is_finite() {
        return None;
    }
    let eased = taskbar_snap_ease(progress);
    let interpolate = |start: i32, end: i32| {
        (f64::from(start) + (f64::from(end) - f64::from(start)) * eased).round() as i32
    };
    let rect = Rect {
        x: interpolate(from.x, to.x),
        y: interpolate(from.y, to.y),
        width: interpolate(from.width, to.width),
        height: interpolate(from.height, to.height),
    };
    rect.is_valid().then_some(rect)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForegroundSafety {
    Clear,
    Fullscreen,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SteadyPollGuard {
    pub selected_bar_matches: bool,
    pub attached: bool,
    pub transitioning: bool,
    pub inner_requested_visible: bool,
    pub latest_requested_visible: bool,
    pub probe_complete: bool,
    pub safety: ForegroundSafety,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockMode {
    Floating,
    Docking,
    Docked,
    Undocking,
    Waiting,
}

pub const OVERLAY_SNAPSHOT_MAX_BYTES: usize = 16_384;

pub fn validate_overlay_snapshot(snapshot: &serde_json::Value) -> Result<(), &'static str> {
    let object = snapshot.as_object().ok_or("snapshot must be an object")?;
    if serde_json::to_vec(snapshot)
        .map_err(|_| "snapshot is not serializable")?
        .len()
        > OVERLAY_SNAPSHOT_MAX_BYTES
    {
        return Err("snapshot is too large");
    }
    const STRINGS: &[(&str, usize)] = &[
        ("artwork", 4_096),
        ("trackTitle", 512),
        ("artistName", 512),
        ("lyric", 2_048),
        ("emptyKind", 16),
        ("emptyLabel", 256),
        ("elapsedLabel", 32),
        ("durationLabel", 32),
        ("locale", 32),
    ];
    const NUMBERS: &[&str] = &["progress", "duration"];
    const BOOLEANS: &[&str] = &["playing", "shuffle", "reducedMotion"];
    for key in object.keys() {
        if !STRINGS.iter().any(|(allowed, _)| allowed == key)
            && !NUMBERS.contains(&key.as_str())
            && !BOOLEANS.contains(&key.as_str())
        {
            return Err("snapshot contains an unsupported field");
        }
    }
    for (key, max_len) in STRINGS {
        let value = object.get(*key).ok_or("snapshot is missing a field")?;
        if *key == "emptyKind" && value.is_null() {
            continue;
        }
        let text = value.as_str().ok_or("snapshot string field is invalid")?;
        if text.len() > *max_len
            || text.chars().any(|character| {
                character.is_control()
                    && !(*key == "lyric" && matches!(character, '\n' | '\r' | '\t'))
            })
            || (*key == "emptyKind" && !matches!(text, "loading" | "track"))
        {
            return Err("snapshot string field is invalid");
        }
    }
    for key in NUMBERS {
        let value = object.get(*key).ok_or("snapshot is missing a field")?;
        let number = value.as_f64().ok_or("snapshot number field is invalid")?;
        if !number.is_finite() || !(0.0..=86_400.0).contains(&number) {
            return Err("snapshot number field is outside the supported range");
        }
    }
    for key in BOOLEANS {
        if !object
            .get(*key)
            .ok_or("snapshot is missing a field")?
            .is_boolean()
        {
            return Err("snapshot boolean field is invalid");
        }
    }
    Ok(())
}

pub fn validate_overlay_action(action: &str, value: Option<f64>) -> Result<(), &'static str> {
    match action {
        "previous" | "play" | "next" | "shuffle" | "undock" | "close" if value.is_none() => Ok(()),
        "seek"
            if value
                .is_some_and(|value| value.is_finite() && (0.0..=86_400.0).contains(&value)) =>
        {
            Ok(())
        }
        _ => Err("overlay action is invalid"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DockDecision {
    pub mode: DockMode,
    pub show: bool,
    pub attach: bool,
    pub suspended: bool,
}

impl Rect {
    pub fn is_valid(self) -> bool {
        self.width > 0 && self.height > 0 && self.width <= 100_000 && self.height <= 100_000
    }
}

pub fn dock_decision(
    enabled: bool,
    compact: bool,
    requested_visible: bool,
    safety: ForegroundSafety,
) -> DockDecision {
    let active = enabled && compact;
    let suspended = active && safety != ForegroundSafety::Clear;
    let mode = if !active {
        DockMode::Floating
    } else if suspended {
        DockMode::Waiting
    } else {
        DockMode::Docked
    };
    DockDecision {
        mode,
        show: requested_visible && !suspended,
        attach: active && !suspended,
        suspended,
    }
}

pub fn compact_mode_is_valid(compact: bool, width: u32, height: u32) -> bool {
    (300..=8_192).contains(&width) && height > 0 && height <= 8_192 && (!compact || height < 340)
}

pub fn live_compact_mode_is_valid(
    width: u32,
    height: u32,
    maximized: bool,
    fullscreen: bool,
) -> bool {
    !maximized && !fullscreen && compact_mode_is_valid(true, width, height)
}

pub fn dock_target(free: Rect, dpi: u32) -> Option<Rect> {
    if !free.is_valid() || !(48..=768).contains(&dpi) {
        return None;
    }
    let width = (380_u64 * u64::from(dpi)).div_ceil(96);
    let width = i32::try_from(width).ok()?;
    if free.width < width {
        return None;
    }
    let x = i64::from(free.x) + i64::from(free.width) - i64::from(width);
    Some(Rect {
        x: i32::try_from(x).ok()?,
        y: free.y,
        width,
        height: free.height,
    })
}

pub fn near_taskbar(bounds: Rect, taskbar: Rect, tolerance: i32) -> bool {
    if !bounds.is_valid() || !taskbar.is_valid() || !(0..=1_000).contains(&tolerance) {
        return false;
    }
    let bounds_right = i64::from(bounds.x) + i64::from(bounds.width);
    let taskbar_right = i64::from(taskbar.x) + i64::from(taskbar.width);
    let bounds_bottom = i64::from(bounds.y) + i64::from(bounds.height);
    let taskbar_bottom = i64::from(taskbar.y) + i64::from(taskbar.height);
    let horizontal_overlap =
        i64::from(bounds.x) < taskbar_right && bounds_right > i64::from(taskbar.x);
    horizontal_overlap
        && i64::from(bounds.y) <= taskbar_bottom + i64::from(tolerance)
        && bounds_bottom >= i64::from(taskbar.y) - i64::from(tolerance)
}

/// Distance above or below a taskbar, in DIPs, that still offers docking.
pub const SNAP_TOLERANCE_DIP: u64 = 40;

pub fn snap_tolerance(dpi: u32) -> i32 {
    let dpi = u64::from(dpi.clamp(48, 768));
    i32::try_from((SNAP_TOLERANCE_DIP * dpi).div_ceil(96)).unwrap_or(40)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DragDockHint {
    None,
    Ready,
    Unavailable,
}

impl DragDockHint {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ready => "ready",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Feedback for a compact player being dragged near `taskbar`. `safe` means
/// the taskbar is visible and its monitor has no fullscreen foreground app.
pub fn drag_dock_hint(
    bounds: Rect,
    taskbar: Rect,
    free: Option<Rect>,
    dpi: u32,
    safe: bool,
) -> DragDockHint {
    let horizontal = taskbar.width >= taskbar.height;
    if !horizontal || !near_taskbar(bounds, taskbar, snap_tolerance(dpi)) {
        return DragDockHint::None;
    }
    if safe && free.and_then(|free| dock_target(free, dpi)).is_some() {
        DragDockHint::Ready
    } else {
        DragDockHint::Unavailable
    }
}

pub fn should_persist_frame(compact: bool, docked: bool, transitioning: bool) -> bool {
    !compact && !docked && !transitioning
}

#[cfg(test)]
mod tests {
    use super::{
        can_skip_steady_foreground_retry, compact_mode_is_valid, dock_decision, dock_target,
        drag_dock_hint, foreground_context_changed, interpolate_rect, live_compact_mode_is_valid,
        near_taskbar, should_persist_frame, snap_tolerance, taskbar_snap_ease,
        validate_overlay_action, validate_overlay_snapshot, DockMode, DragDockHint,
        ForegroundContext, ForegroundSafety, Rect, SteadyPollGuard,
    };

    const BOTTOM_TASKBAR: Rect = Rect {
        x: 0,
        y: 1040,
        width: 1920,
        height: 40,
    };
    const WIDE_FREE_BAND: Rect = Rect {
        x: 800,
        y: 1040,
        width: 700,
        height: 40,
    };

    fn compact_player_with_bottom(bottom: i32) -> Rect {
        Rect {
            x: 820,
            y: bottom - 60,
            width: 593,
            height: 60,
        }
    }

    #[test]
    fn foreground_context_changes_require_a_fresh_full_safety_probe() {
        let before = ForegroundContext {
            hwnd: 0x1234,
            bounds: Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        };
        let different_window = ForegroundContext {
            hwnd: 0x5678,
            ..before
        };
        let changed_bounds = ForegroundContext {
            bounds: Rect {
                width: 1918,
                ..before.bounds
            },
            ..before
        };

        assert!(!foreground_context_changed(Some(before), Some(before)));
        assert!(foreground_context_changed(
            Some(before),
            Some(different_window)
        ));
        assert!(foreground_context_changed(
            Some(before),
            Some(changed_bounds)
        ));
        assert!(foreground_context_changed(Some(before), None));
        assert!(foreground_context_changed(None, Some(before)));
        assert!(foreground_context_changed(None, None));
    }

    fn eligible_steady_poll_guard() -> SteadyPollGuard {
        SteadyPollGuard {
            selected_bar_matches: true,
            attached: true,
            transitioning: false,
            inner_requested_visible: true,
            latest_requested_visible: true,
            probe_complete: true,
            safety: ForegroundSafety::Clear,
        }
    }

    fn test_rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn test_foreground(hwnd: u64, bounds: Rect) -> ForegroundContext {
        ForegroundContext { hwnd, bounds }
    }

    #[test]
    fn steady_attached_poll_keeps_showing_during_two_hundred_safe_translations() {
        let monitor = test_rect(0, 0, 1920, 1080);
        let guard = eligible_steady_poll_guard();
        let mut previous = Some(test_foreground(0x1234, test_rect(100, 200, 380, 40)));
        let mut visible = true;

        for step in 1..=200 {
            let current = test_foreground(0x1234, test_rect(100 + step, 200 + step, 380, 40));
            if foreground_context_changed(previous, Some(current))
                && !can_skip_steady_foreground_retry(guard, previous, Some(current), monitor)
            {
                visible = false;
            }
            assert!(visible, "overlay hidden on translation {step}");
            assert!(dock_decision(true, true, true, ForegroundSafety::Clear).show);
            previous = Some(current);
        }
    }

    #[test]
    fn steady_translation_exception_rejects_unsafe_or_unverifiable_contexts() {
        let monitor = test_rect(0, 0, 1920, 1080);
        let guard = eligible_steady_poll_guard();
        let before = test_foreground(0x1234, test_rect(100, 200, 380, 40));
        let moved = test_foreground(0x1234, test_rect(101, 200, 380, 40));
        assert!(can_skip_steady_foreground_retry(
            guard,
            Some(before),
            Some(moved),
            monitor
        ));

        let guard_cases = [
            (
                "selected bar changed",
                SteadyPollGuard {
                    selected_bar_matches: false,
                    ..guard
                },
            ),
            (
                "detached",
                SteadyPollGuard {
                    attached: false,
                    ..guard
                },
            ),
            (
                "transitioning",
                SteadyPollGuard {
                    transitioning: true,
                    ..guard
                },
            ),
            (
                "inner hidden",
                SteadyPollGuard {
                    inner_requested_visible: false,
                    ..guard
                },
            ),
            (
                "latest hidden",
                SteadyPollGuard {
                    latest_requested_visible: false,
                    ..guard
                },
            ),
            (
                "incomplete",
                SteadyPollGuard {
                    probe_complete: false,
                    ..guard
                },
            ),
            (
                "fullscreen",
                SteadyPollGuard {
                    safety: ForegroundSafety::Fullscreen,
                    ..guard
                },
            ),
            (
                "unknown",
                SteadyPollGuard {
                    safety: ForegroundSafety::Unknown,
                    ..guard
                },
            ),
        ];
        for (label, state) in guard_cases {
            assert!(
                !can_skip_steady_foreground_retry(state, Some(before), Some(moved), monitor),
                "allowed {label}"
            );
        }

        let full_monitor_before = test_foreground(0x1234, test_rect(100, 200, 1920, 1080));
        let full_monitor_moved = test_foreground(0x1234, test_rect(101, 201, 1920, 1080));
        let geometry_cases: [(
            &str,
            Option<ForegroundContext>,
            Option<ForegroundContext>,
            Rect,
        ); 9] = [
            (
                "size changed",
                Some(before),
                Some(test_foreground(0x1234, test_rect(101, 200, 381, 40))),
                monitor,
            ),
            (
                "different HWND",
                Some(before),
                Some(test_foreground(0x5678, moved.bounds)),
                monitor,
            ),
            (
                "zero HWND",
                Some(before),
                Some(test_foreground(0, moved.bounds)),
                monitor,
            ),
            ("missing previous", None, Some(moved), monitor),
            ("missing current", Some(before), None, monitor),
            (
                "invalid foreground rect",
                Some(before),
                Some(test_foreground(0x1234, test_rect(101, 200, 0, 40))),
                monitor,
            ),
            (
                "invalid selected monitor",
                Some(before),
                Some(moved),
                test_rect(0, 0, 0, 1080),
            ),
            (
                "may cover entire monitor",
                Some(full_monitor_before),
                Some(full_monitor_moved),
                monitor,
            ),
            ("no translation", Some(before), Some(before), monitor),
        ];
        for (label, previous, current, selected_monitor) in geometry_cases {
            assert!(
                !can_skip_steady_foreground_retry(guard, previous, current, selected_monitor),
                "allowed {label}"
            );
        }

        let wide_before = test_foreground(0x1234, test_rect(100, 200, 1920, 40));
        let wide_moved = test_foreground(0x1234, test_rect(101, 200, 1920, 40));
        assert!(can_skip_steady_foreground_retry(
            guard,
            Some(wide_before),
            Some(wide_moved),
            monitor
        ));
    }

    #[test]
    fn stable_clear_context_recovers_visibility_after_unknown_probe() {
        let hidden = dock_decision(true, true, true, ForegroundSafety::Unknown);
        assert!(hidden.suspended && !hidden.show);
        let stable = test_foreground(0x1234, test_rect(100, 200, 380, 40));
        assert!(!foreground_context_changed(Some(stable), Some(stable)));
        let recovered = dock_decision(true, true, true, ForegroundSafety::Clear);
        assert!(!recovered.suspended && recovered.show);
    }

    #[test]
    fn snap_tolerance_scales_forty_dips_with_the_taskbar_dpi() {
        assert_eq!(snap_tolerance(96), 40);
        assert_eq!(snap_tolerance(144), 60);
        assert_eq!(snap_tolerance(120), 50);
        assert_eq!(snap_tolerance(0), snap_tolerance(48));
    }

    #[test]
    fn dragging_within_the_snap_band_offers_docking_when_space_is_free() {
        for bottom in [1000, 1040, 1080] {
            assert_eq!(
                drag_dock_hint(
                    compact_player_with_bottom(bottom),
                    BOTTOM_TASKBAR,
                    Some(WIDE_FREE_BAND),
                    96,
                    true
                ),
                DragDockHint::Ready,
                "bottom edge at {bottom}"
            );
        }
        assert_eq!(
            drag_dock_hint(
                compact_player_with_bottom(999),
                BOTTOM_TASKBAR,
                Some(WIDE_FREE_BAND),
                96,
                true
            ),
            DragDockHint::None
        );
    }

    #[test]
    fn high_dpi_taskbars_keep_the_same_logical_snap_band() {
        let taskbar = Rect {
            x: 0,
            y: 1380,
            width: 2560,
            height: 60,
        };
        let free = Rect {
            x: 1000,
            y: 1380,
            width: 900,
            height: 60,
        };
        let player = |bottom: i32| Rect {
            x: 1200,
            y: bottom - 90,
            width: 890,
            height: 90,
        };
        assert_eq!(
            drag_dock_hint(player(1321), taskbar, Some(free), 144, true),
            DragDockHint::Ready
        );
        assert_eq!(
            drag_dock_hint(player(1319), taskbar, Some(free), 144, true),
            DragDockHint::None
        );
    }

    #[test]
    fn near_but_undockable_taskbars_explain_instead_of_offering_a_snap() {
        let player = compact_player_with_bottom(1030);
        let narrow = Rect {
            width: 379,
            ..WIDE_FREE_BAND
        };
        for (free, safe) in [
            (Some(narrow), true),
            (None, true),
            (Some(WIDE_FREE_BAND), false),
        ] {
            assert_eq!(
                drag_dock_hint(player, BOTTOM_TASKBAR, free, 96, safe),
                DragDockHint::Unavailable
            );
        }
    }

    #[test]
    fn vertical_taskbars_never_offer_the_horizontal_strip() {
        let vertical = Rect {
            x: 0,
            y: 0,
            width: 62,
            height: 1080,
        };
        let player = Rect {
            x: 40,
            y: 500,
            width: 593,
            height: 60,
        };
        assert_eq!(
            drag_dock_hint(player, vertical, Some(WIDE_FREE_BAND), 96, true),
            DragDockHint::None
        );
        assert_eq!(DragDockHint::Ready.as_str(), "ready");
        assert_eq!(DragDockHint::Unavailable.as_str(), "unavailable");
        assert_eq!(DragDockHint::None.as_str(), "none");
    }

    #[test]
    fn fullscreen_hides_compact_window_and_retains_enabled_dock_intent_for_resume() {
        let hidden = dock_decision(true, true, true, ForegroundSafety::Fullscreen);
        assert_eq!(hidden.mode, DockMode::Waiting);
        assert!(!hidden.show);
        assert!(!hidden.attach);
        assert!(hidden.suspended);

        let resumed = dock_decision(true, true, true, ForegroundSafety::Clear);
        assert_eq!(resumed.mode, DockMode::Docked);
        assert!(resumed.show);
        assert!(resumed.attach);
        assert!(!resumed.suspended);
    }

    #[test]
    fn unknown_foreground_probe_is_fail_closed_for_active_compact_docking() {
        let decision = dock_decision(true, true, true, ForegroundSafety::Unknown);
        assert_eq!(decision.mode, DockMode::Waiting);
        assert!(!decision.show);
        assert!(!decision.attach);
        assert!(decision.suspended);
    }

    #[test]
    fn fullscreen_does_not_hide_regular_browsing_or_disabled_compact_mode() {
        for (enabled, compact) in [(false, true), (true, false), (false, false)] {
            let decision = dock_decision(enabled, compact, true, ForegroundSafety::Fullscreen);
            assert_eq!(decision.mode, DockMode::Floating);
            assert!(decision.show);
            assert!(!decision.attach);
            assert!(!decision.suspended);
        }
    }

    #[test]
    fn user_hidden_window_is_not_resurfaced_after_guard_clears() {
        let decision = dock_decision(true, true, false, ForegroundSafety::Clear);
        assert_eq!(decision.mode, DockMode::Docked);
        assert!(!decision.show);
        assert!(decision.attach);
        assert!(!decision.suspended);
    }

    #[test]
    fn compact_command_flag_requires_compact_inner_height() {
        assert!(compact_mode_is_valid(true, 900, 150));
        assert!(compact_mode_is_valid(true, 380, 64));
        assert!(!compact_mode_is_valid(true, 900, 340));
        assert!(!compact_mode_is_valid(true, 299, 150));
        assert!(!compact_mode_is_valid(true, 900, 0));
        assert!(compact_mode_is_valid(false, 900, 150));
        assert!(compact_mode_is_valid(false, 920, 620));
        assert!(!compact_mode_is_valid(true, 900, 340));
    }

    #[test]
    fn live_compact_eligibility_checks_size_boundaries_and_window_state() {
        assert!(live_compact_mode_is_valid(300, 1, false, false));
        assert!(live_compact_mode_is_valid(8_192, 339, false, false));

        for (width, height) in [(299, 150), (8_193, 150), (900, 0), (900, 340)] {
            assert!(
                !live_compact_mode_is_valid(width, height, false, false),
                "unexpectedly accepted {width}x{height} as a live compact window"
            );
        }

        assert!(!live_compact_mode_is_valid(900, 150, true, false));
        assert!(!live_compact_mode_is_valid(900, 150, false, true));
        assert!(!live_compact_mode_is_valid(900, 150, true, true));
    }

    #[test]
    fn dock_target_is_380_dip_by_real_taskbar_height() {
        let target = dock_target(
            Rect {
                x: 100,
                y: 1040,
                width: 800,
                height: 48,
            },
            144,
        )
        .unwrap();
        assert_eq!(
            target,
            Rect {
                x: 330,
                y: 1040,
                width: 570,
                height: 48
            }
        );
    }

    #[test]
    fn docking_rejects_invalid_or_insufficient_free_geometry() {
        assert_eq!(
            dock_target(
                Rect {
                    x: 0,
                    y: 0,
                    width: 569,
                    height: 48
                },
                144
            ),
            None
        );
        assert_eq!(
            dock_target(
                Rect {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 0
                },
                96
            ),
            None
        );
        assert_eq!(
            dock_target(
                Rect {
                    x: 0,
                    y: 0,
                    width: 800,
                    height: 48
                },
                0
            ),
            None
        );
    }

    #[test]
    fn compact_window_snaps_only_inside_near_taskbar_band() {
        let taskbar = Rect {
            x: 0,
            y: 1000,
            width: 1920,
            height: 80,
        };
        assert!(near_taskbar(
            Rect {
                x: 900,
                y: 1000,
                width: 380,
                height: 150
            },
            taskbar,
            28
        ));
        assert!(near_taskbar(
            Rect {
                x: 900,
                y: 850,
                width: 380,
                height: 150
            },
            taskbar,
            28
        ));
        assert!(!near_taskbar(
            Rect {
                x: 900,
                y: 800,
                width: 380,
                height: 150
            },
            taskbar,
            28
        ));
        assert!(!near_taskbar(
            Rect {
                x: 1920,
                y: 1000,
                width: 380,
                height: 150
            },
            taskbar,
            28
        ));
    }

    #[test]
    fn main_window_frame_persistence_is_disabled_while_compact_docked_or_transitioning() {
        assert!(!should_persist_frame(true, false, false));
        assert!(!should_persist_frame(false, true, false));
        assert!(!should_persist_frame(false, false, true));
        assert!(should_persist_frame(false, false, false));
    }

    #[test]
    fn overlay_snapshot_accepts_only_bounded_presentation_fields() {
        let valid = serde_json::json!({
            "artwork": "https://example.test/cover.jpg",
            "trackTitle": "Song",
            "artistName": "Artist",
            "lyric": "Line",
            "emptyKind": null,
            "emptyLabel": "",
            "progress": 10.5,
            "duration": 180.0,
            "elapsedLabel": "0:10",
            "durationLabel": "3:00",
            "playing": true,
            "shuffle": false,
            "locale": "zh-CN",
            "reducedMotion": false
        });
        assert!(validate_overlay_snapshot(&valid).is_ok());
        assert!(validate_overlay_snapshot(&serde_json::json!({"accountToken":"secret"})).is_err());
        assert!(validate_overlay_snapshot(&serde_json::json!({"trackTitle": 42})).is_err());
        assert!(validate_overlay_snapshot(&serde_json::json!({"progress": -1})).is_err());
        assert!(validate_overlay_snapshot(&serde_json::json!({})).is_err());
        assert!(validate_overlay_snapshot(&serde_json::json!({
            "artwork": "", "trackTitle": "", "artistName": "", "lyric": "",
            "emptyKind": "unexpected", "emptyLabel": "", "progress": 0,
            "duration": 0, "elapsedLabel": "", "durationLabel": "",
            "playing": false, "shuffle": false, "locale": "en-US",
            "reducedMotion": false
        }))
        .is_err());
        assert!(
            validate_overlay_snapshot(&serde_json::json!({"lyric": "x".repeat(2_049)})).is_err()
        );
    }

    #[test]
    fn overlay_actions_are_allowlisted_and_seek_is_bounded() {
        for action in ["previous", "play", "next", "shuffle", "undock", "close"] {
            assert!(validate_overlay_action(action, None).is_ok(), "{action}");
        }
        assert!(validate_overlay_action("seek", Some(30.0)).is_ok());
        assert!(validate_overlay_action("seek", Some(f64::NAN)).is_err());
        assert!(validate_overlay_action("seek", Some(-1.0)).is_err());
        assert!(validate_overlay_action("close", Some(1.0)).is_err());
        assert!(validate_overlay_action("eval", None).is_err());
    }

    #[test]
    fn overlay_animation_uses_the_electron_cubic_bezier_and_exact_endpoints() {
        let from = Rect {
            x: 100,
            y: 200,
            width: 900,
            height: 150,
        };
        let to = Rect {
            x: 700,
            y: 1_000,
            width: 380,
            height: 48,
        };
        assert_eq!(interpolate_rect(from, to, 0.0), Some(from));
        assert_eq!(interpolate_rect(from, to, 1.0), Some(to));
        let middle = interpolate_rect(from, to, 0.5).unwrap();
        assert!(middle.x > from.x && middle.x < to.x);
        assert!(middle.y > from.y && middle.y < to.y);
        assert!(taskbar_snap_ease(0.5) > 0.5);
        assert_eq!(interpolate_rect(from, to, f64::NAN), None);
    }

    #[test]
    fn docking_preserves_arbitrary_compact_frame_for_restore() {
        let original = Rect {
            x: 100,
            y: 300,
            width: 900,
            height: 150,
        };
        let docked = dock_target(
            Rect {
                x: 0,
                y: 1040,
                width: 1000,
                height: 48,
            },
            96,
        )
        .unwrap();
        assert_eq!(docked.width, 380);
        assert_eq!(docked.height, 48);
        assert_eq!(
            original,
            Rect {
                x: 100,
                y: 300,
                width: 900,
                height: 150
            }
        );
    }
}
