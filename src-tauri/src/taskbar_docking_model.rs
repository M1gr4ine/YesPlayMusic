#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
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

pub fn should_persist_frame(compact: bool, docked: bool, transitioning: bool) -> bool {
    !compact && !docked && !transitioning
}

#[cfg(test)]
mod tests {
    use super::{
        compact_mode_is_valid, dock_decision, dock_target, interpolate_rect,
        live_compact_mode_is_valid, near_taskbar, should_persist_frame, taskbar_snap_ease,
        validate_overlay_action, validate_overlay_snapshot, DockMode, ForegroundSafety, Rect,
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
