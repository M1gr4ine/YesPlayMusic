#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForegroundSafety {
    Clear,
    Fullscreen,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockMode {
    Floating,
    Docked,
    Waiting,
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

pub fn should_persist_frame(compact: bool, docked: bool, transitioning: bool) -> bool {
    !compact && !docked && !transitioning
}

#[cfg(test)]
mod tests {
    use super::{
        compact_mode_is_valid, dock_decision, dock_target, live_compact_mode_is_valid,
        near_taskbar, should_persist_frame, DockMode, ForegroundSafety, Rect,
    };

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
