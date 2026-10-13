//! Platform-independent geometry, probe and payload rules for Windows taskbar
//! docking, kept apart from Win32 so they are tested on every CI platform.

use serde::{Deserialize, Serialize};

/// A rectangle in physical screen pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn is_valid(self) -> bool {
        (1..=100_000).contains(&self.width) && (1..=100_000).contains(&self.height)
    }

    fn contains(self, x: i64, y: i64) -> bool {
        (i64::from(self.x)..i64::from(self.x) + i64::from(self.width)).contains(&x)
            && (i64::from(self.y)..i64::from(self.y) + i64::from(self.height)).contains(&y)
    }
}

/// A logical client size the renderer lays out as the compact bar.
pub fn is_compact_size(width: u32, height: u32) -> bool {
    (300..=8_192).contains(&width) && (1..340).contains(&height)
}

/// The native helper's view of every taskbar.
#[derive(Debug, Deserialize)]
pub struct Probe {
    /// False when some taskbar could not be read; such a probe proves nothing.
    pub complete: bool,
    pub bars: Vec<Taskbar>,
}

#[derive(Debug, Deserialize)]
pub struct Taskbar {
    pub handle: String,
    pub taskbar: Rect,
    pub monitor: Rect,
    /// The empty band between the task buttons and the tray, when known.
    pub free: Option<Rect>,
    pub dpi: u32,
    /// Hidden taskbar, or a foreground window that is fullscreen or unreadable.
    pub suspended: bool,
    pub fullscreen: Option<bool>,
}

impl Probe {
    /// The taskbar on the monitor that contains the centre of `window`.
    pub fn bar_for(&self, window: Rect) -> Option<&Taskbar> {
        if !window.is_valid() {
            return None;
        }
        let x = i64::from(window.x) + i64::from(window.width) / 2;
        let y = i64::from(window.y) + i64::from(window.height) / 2;
        self.bars
            .iter()
            .find(|bar| bar.monitor.is_valid() && bar.monitor.contains(x, y))
    }

    /// Whether the strip may be shown over the taskbar under `window`.
    pub fn is_clear_for(&self, window: Rect) -> bool {
        self.complete && self.bar_for(window).is_some_and(Taskbar::is_clear)
    }
}

impl Taskbar {
    /// Visible, with a foreground window known not to be fullscreen.
    pub fn is_clear(&self) -> bool {
        !self.suspended && self.fullscreen == Some(false)
    }

    fn is_horizontal(&self) -> bool {
        self.taskbar.width >= self.taskbar.height
    }

    /// Where the 380-DIP strip lands: the right end of the free band.
    pub fn dock_target(&self) -> Option<Rect> {
        let free = self.free.filter(|free| free.is_valid())?;
        if !self.is_horizontal() || !(48..=768).contains(&self.dpi) {
            return None;
        }
        let width = i32::try_from((380 * u64::from(self.dpi)).div_ceil(96)).ok()?;
        (free.width >= width).then(|| Rect {
            x: free.x + free.width - width,
            width,
            ..free
        })
    }

    /// Within 40 DIPs above or below the taskbar, overlapping it horizontally.
    fn is_near(&self, window: Rect) -> bool {
        let tolerance = (40 * u64::from(self.dpi.clamp(48, 768))).div_ceil(96) as i64;
        let bar = self.taskbar;
        let (left, right) = (
            i64::from(window.x),
            i64::from(window.x) + i64::from(window.width),
        );
        let (top, bottom) = (
            i64::from(window.y),
            i64::from(window.y) + i64::from(window.height),
        );
        window.is_valid()
            && bar.is_valid()
            && left < i64::from(bar.x) + i64::from(bar.width)
            && right > i64::from(bar.x)
            && top <= i64::from(bar.y) + i64::from(bar.height) + tolerance
            && bottom >= i64::from(bar.y) - tolerance
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DragHint {
    None,
    Ready,
    Unavailable,
}

/// Feedback for a compact player dragged to `window`.
pub fn drag_hint(probe: &Probe, window: Rect) -> DragHint {
    let Some(bar) = probe
        .bar_for(window)
        .filter(|bar| bar.is_horizontal() && bar.is_near(window))
    else {
        return DragHint::None;
    };
    if probe.complete && bar.is_clear() && bar.dock_target().is_some() {
        DragHint::Ready
    } else {
        DragHint::Unavailable
    }
}

/// The strip's flight curve: CSS `cubic-bezier(.77, 0, .175, 1)`.
pub fn snap_ease(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    let bezier = |t: f64, first: f64, second: f64| {
        let inverse = 1.0 - t;
        3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
    };
    let (mut low, mut high) = (0.0, 1.0);
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

pub fn interpolate_rect(from: Rect, to: Rect, progress: f64) -> Rect {
    let eased = snap_ease(progress);
    let lerp = |start: i32, end: i32| {
        (f64::from(start) + (f64::from(end) - f64::from(start)) * eased).round() as i32
    };
    Rect {
        x: lerp(from.x, to.x),
        y: lerp(from.y, to.y),
        width: lerp(from.width, to.width),
        height: lerp(from.height, to.height),
    }
}

/// Presentation-only playback state the player publishes for the strip.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverlaySnapshot {
    artwork: String,
    track_title: String,
    artist_name: String,
    lyric: String,
    empty_kind: Option<EmptyKind>,
    empty_label: String,
    progress: f64,
    duration: f64,
    playing: bool,
    shuffle: bool,
    locale: String,
    pub reduced_motion: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum EmptyKind {
    Loading,
    Track,
}

impl OverlaySnapshot {
    /// Bounds every field, since the overlay renders them verbatim.
    pub fn validate(&self) -> Result<(), &'static str> {
        let text = |value: &str, max_len: usize, multiline: bool| {
            value.len() <= max_len
                && !value
                    .chars()
                    .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
        };
        let valid = text(&self.artwork, 4_096, false)
            && text(&self.track_title, 512, false)
            && text(&self.artist_name, 512, false)
            && text(&self.lyric, 2_048, true)
            && text(&self.empty_label, 256, false)
            && text(&self.locale, 32, false)
            && is_track_time(self.progress)
            && is_track_time(self.duration);
        valid
            .then_some(())
            .ok_or("taskbar overlay snapshot is invalid")
    }
}

pub fn is_track_time(seconds: f64) -> bool {
    seconds.is_finite() && (0.0..=86_400.0).contains(&seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn bottom_bar(free: Option<Rect>, dpi: u32) -> Taskbar {
        Taskbar {
            handle: "65552".to_owned(),
            taskbar: rect(0, 1040, 1920, 40),
            monitor: rect(0, 0, 1920, 1080),
            free,
            dpi,
            suspended: false,
            fullscreen: Some(false),
        }
    }

    fn probe(bars: Vec<Taskbar>) -> Probe {
        Probe {
            complete: true,
            bars,
        }
    }

    fn player_with_bottom(bottom: i32) -> Rect {
        rect(820, bottom - 60, 593, 60)
    }

    #[test]
    fn the_strip_is_380_dips_at_the_right_end_of_the_free_band() {
        let bar = Taskbar {
            free: Some(rect(100, 1040, 800, 48)),
            ..bottom_bar(None, 144)
        };
        assert_eq!(bar.dock_target(), Some(rect(330, 1040, 570, 48)));

        let narrow = Taskbar {
            free: Some(rect(0, 1040, 569, 48)),
            ..bottom_bar(None, 144)
        };
        assert_eq!(narrow.dock_target(), None);
        assert_eq!(bottom_bar(None, 96).dock_target(), None);
    }

    #[test]
    fn dragging_within_forty_dips_of_the_taskbar_offers_docking() {
        let free = Some(rect(800, 1040, 700, 40));
        let probe = probe(vec![bottom_bar(free, 96)]);
        for bottom in [1000, 1040, 1080] {
            assert_eq!(
                drag_hint(&probe, player_with_bottom(bottom)),
                DragHint::Ready,
                "bottom edge at {bottom}"
            );
        }
        assert_eq!(drag_hint(&probe, player_with_bottom(999)), DragHint::None);

        let high_dpi = probe_with_dpi(144);
        assert_eq!(
            drag_hint(&high_dpi, rect(820, 1321 - 90, 593, 90)),
            DragHint::Ready
        );
        assert_eq!(
            drag_hint(&high_dpi, rect(820, 1319 - 90, 593, 90)),
            DragHint::None
        );
    }

    fn probe_with_dpi(dpi: u32) -> Probe {
        probe(vec![Taskbar {
            taskbar: rect(0, 1380, 2560, 60),
            monitor: rect(0, 0, 2560, 1440),
            free: Some(rect(1000, 1380, 900, 60)),
            ..bottom_bar(None, dpi)
        }])
    }

    #[test]
    fn near_but_undockable_taskbars_explain_instead_of_snapping() {
        let player = player_with_bottom(1030);
        let fullscreen = Taskbar {
            fullscreen: Some(true),
            ..bottom_bar(Some(rect(800, 1040, 700, 40)), 96)
        };
        for bar in [
            bottom_bar(Some(rect(800, 1040, 379, 40)), 96),
            bottom_bar(None, 96),
            fullscreen,
        ] {
            assert_eq!(drag_hint(&probe(vec![bar]), player), DragHint::Unavailable);
        }
    }

    #[test]
    fn vertical_taskbars_never_offer_the_strip() {
        let vertical = Taskbar {
            taskbar: rect(0, 0, 62, 1080),
            ..bottom_bar(Some(rect(0, 0, 62, 1080)), 96)
        };
        assert_eq!(
            drag_hint(&probe(vec![vertical]), rect(40, 500, 593, 60)),
            DragHint::None
        );
    }

    #[test]
    fn safety_uses_the_window_monitor_and_fails_closed() {
        let secondary = Taskbar {
            handle: "131088".to_owned(),
            monitor: rect(1920, 0, 1920, 1080),
            fullscreen: Some(true),
            ..bottom_bar(None, 96)
        };
        let both = probe(vec![bottom_bar(None, 96), secondary]);
        assert!(both.is_clear_for(rect(100, 100, 500, 60)));
        assert!(!both.is_clear_for(rect(2000, 100, 500, 60)));
        assert!(!both.is_clear_for(rect(5000, 100, 500, 60)));

        let unknown = Taskbar {
            fullscreen: None,
            ..bottom_bar(None, 96)
        };
        assert!(!probe(vec![unknown]).is_clear_for(rect(100, 100, 500, 60)));
        let incomplete = Probe {
            complete: false,
            bars: vec![bottom_bar(None, 96)],
        };
        assert!(!incomplete.is_clear_for(rect(100, 100, 500, 60)));
    }

    #[test]
    fn compact_size_matches_the_renderer_bar_breakpoint() {
        assert!(is_compact_size(300, 1));
        assert!(is_compact_size(8_192, 339));
        for (width, height) in [(299, 150), (8_193, 150), (900, 0), (900, 340)] {
            assert!(!is_compact_size(width, height), "{width}x{height}");
        }
    }

    #[test]
    fn flights_follow_the_curve_and_land_exactly() {
        let from = rect(100, 200, 900, 150);
        let to = rect(700, 1_000, 380, 48);
        assert_eq!(interpolate_rect(from, to, 0.0), from);
        assert_eq!(interpolate_rect(from, to, 1.0), to);
        assert!(snap_ease(0.25) < 0.25 && snap_ease(0.75) > 0.75);
    }

    #[test]
    fn snapshots_reject_unknown_or_unbounded_fields() {
        let valid = serde_json::json!({
            "artwork": "https://example.test/cover.jpg",
            "trackTitle": "Song",
            "artistName": "Artist",
            "lyric": "Line",
            "emptyKind": null,
            "emptyLabel": "",
            "progress": 10.5,
            "duration": 180.0,
            "playing": true,
            "shuffle": false,
            "locale": "zh-CN",
            "reducedMotion": false
        });
        let parse = |value: serde_json::Value| {
            serde_json::from_value::<OverlaySnapshot>(value)
                .map_err(|_| "unparsable")
                .and_then(|snapshot| snapshot.validate())
        };
        assert!(parse(valid.clone()).is_ok());

        let with = |key: &str, value: serde_json::Value| {
            let mut snapshot = valid.clone();
            snapshot[key] = value;
            snapshot
        };
        assert!(parse(with("accountToken", "secret".into())).is_err());
        assert!(parse(with("emptyKind", "unexpected".into())).is_err());
        assert!(parse(with("progress", (-1).into())).is_err());
        assert!(parse(with("lyric", "x".repeat(2_049).into())).is_err());
        assert!(parse(with("trackTitle", "a\u{7}b".into())).is_err());
    }
}
