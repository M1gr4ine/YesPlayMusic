use super::hide;
use windows::{
    core::w,
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP},
    },
};

struct WindowFixture {
    parent: HWND,
}

impl WindowFixture {
    fn new() -> Self {
        let parent = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Taskbar fullscreen test"),
                WS_POPUP,
                -32000,
                -32000,
                80,
                40,
                None,
                None,
                None,
                None,
            )
        }
        .expect("create fullscreen fixture");
        Self { parent }
    }
}

impl Drop for WindowFixture {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.parent) };
    }
}

#[test]
fn unknown_fullscreen_samples_keep_the_last_known_value() {
    for known in [false, true] {
        let mut previous = Some(known);
        assert_eq!(
            super::remember_fullscreen_sample(&mut previous, None),
            (known, false)
        );
        assert_eq!(previous, Some(known));
    }

    let mut previous = None;
    assert_eq!(
        super::remember_fullscreen_sample(&mut previous, None),
        (false, false),
        "an unknown initial sample matches State::default"
    );
    assert_eq!(previous, None);
}

#[test]
fn fullscreen_sample_changes_publish_only_valid_transitions() {
    let mut previous = None;
    assert_eq!(
        super::remember_fullscreen_sample(&mut previous, Some(true)),
        (true, true)
    );
    assert_eq!(
        super::remember_fullscreen_sample(&mut previous, None),
        (true, false)
    );
    assert_eq!(
        super::remember_fullscreen_sample(&mut previous, Some(false)),
        (false, true)
    );
}

#[test]
#[ignore = "uses the foreground desktop; run explicitly with one test thread"]
fn watcher_tracks_same_window_fullscreen_and_timer_only_recovery() {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    use windows::Win32::{
        Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        },
        System::Threading::{AttachThreadInput, GetCurrentThreadId},
        UI::WindowsAndMessaging::{
            DispatchMessageW, GetForegroundWindow, GetWindowThreadProcessId, PeekMessageW,
            SetForegroundWindow, ShowWindow, TranslateMessage, MSG, PM_REMOVE, SW_SHOW,
        },
    };

    struct RestoreForeground {
        original: HWND,
        fixture: HWND,
    }
    impl Drop for RestoreForeground {
        fn drop(&mut self) {
            if unsafe { GetForegroundWindow() } == self.fixture {
                let _ = unsafe { SetForegroundWindow(self.original) };
            }
        }
    }
    struct RestoreSink(Option<super::Sink>);
    impl Drop for RestoreSink {
        fn drop(&mut self) {
            *super::SINK.lock().unwrap() = self.0.take();
        }
    }
    fn wait_for(rx: &mpsc::Receiver<super::HookEvent>, expected: bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut message = MSG::default();
        loop {
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                let _ = unsafe { TranslateMessage(&message) };
                unsafe { DispatchMessageW(&message) };
            }
            while let Ok(event) = rx.try_recv() {
                if event
                    == (super::HookEvent::Foreground {
                        fullscreen: expected,
                    })
                {
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "fullscreen event {expected} was not delivered"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn assert_no_foreground(rx: &mpsc::Receiver<super::HookEvent>, duration: Duration) {
        let deadline = Instant::now() + duration;
        let mut message = MSG::default();
        loop {
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                let _ = unsafe { TranslateMessage(&message) };
                unsafe { DispatchMessageW(&message) };
            }
            while let Ok(event) = rx.try_recv() {
                assert!(
                    !matches!(event, super::HookEvent::Foreground { .. }),
                    "fullscreen polling remained active while undocked"
                );
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    let _apartment = crate::win32::Apartment::enter().expect("enter COM apartment");
    let original = unsafe { GetForegroundWindow() };
    let monitor = unsafe { MonitorFromWindow(original, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap(),
        ..Default::default()
    };
    assert!(unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool());
    let screen = crate::win32::rect_from(info.rcMonitor).unwrap();
    let normal = super::Rect::new(screen.x + 80, screen.y + 80, 400, 240);
    let windows = WindowFixture::new();
    let _foreground = RestoreForeground {
        original,
        fixture: windows.parent,
    };
    let raw = windows.parent.0 as usize;
    super::set_rect(raw, normal).unwrap();
    let _ = unsafe { ShowWindow(windows.parent, SW_SHOW) };
    // Share the active input queue for this explicit foreground desktop test.
    let current_thread = unsafe { GetCurrentThreadId() };
    let foreground_thread = unsafe { GetWindowThreadProcessId(original, None) };
    let attached = foreground_thread != 0
        && unsafe { AttachThreadInput(current_thread, foreground_thread, true) }.as_bool();
    let _ = unsafe { SetForegroundWindow(windows.parent) };
    if attached {
        let _ = unsafe { AttachThreadInput(current_thread, foreground_thread, false) };
    }
    assert_eq!(
        unsafe { GetForegroundWindow() },
        windows.parent,
        "fixture must really own foreground"
    );

    let (tx, rx) = mpsc::channel();
    let _sink = RestoreSink(super::SINK.lock().unwrap().replace(Box::new(move |event| {
        let _ = tx.send(event);
    })));
    *super::LAST_FULLSCREEN.lock().unwrap() = None;
    let mut installed = super::Installed::new();
    assert!(!installed.polling, "watcher starts idle until docking");
    installed.set_docked(true);
    assert!(installed.polling);
    wait_for(&rx, false);
    super::set_rect(raw, screen).unwrap();
    assert_eq!(unsafe { GetForegroundWindow() }, windows.parent);
    wait_for(&rx, true);
    super::set_rect(raw, normal).unwrap();
    wait_for(&rx, false);

    // The timer must recover same-window transitions without a global location hook.
    super::unhook(installed.foreground);
    installed.foreground = Default::default();
    super::set_rect(raw, screen).unwrap();
    wait_for(&rx, true);
    super::set_rect(raw, normal).unwrap();
    wait_for(&rx, false);

    // Polling stops when docking ends and resumes when it starts again.
    installed.set_docked(false);
    assert!(!installed.polling);
    assert_no_foreground(&rx, Duration::from_millis(50));
    super::set_rect(raw, screen).unwrap();
    assert_eq!(super::fullscreen_foreground(), Some(true));
    assert_no_foreground(&rx, Duration::from_millis(400));
    installed.set_docked(true);
    assert!(installed.polling);
    wait_for(&rx, true);
    drop(installed);
}

#[test]
fn fullscreen_client_bounds_cover_the_monitor_but_work_area_does_not() {
    use super::{set_rect, window_covers_monitor, Rect};
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let foreground = unsafe { GetForegroundWindow() };
    let windows = WindowFixture::new();
    hide(windows.parent.0 as usize);
    let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap(),
        ..Default::default()
    };
    assert!(unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool());
    let screen = crate::win32::rect_from(info.rcMonitor).unwrap();
    let raw = windows.parent.0 as usize;
    set_rect(raw, screen).expect("place fullscreen fixture");
    assert_eq!(window_covers_monitor(windows.parent), Some(true));

    set_rect(raw, Rect::new(screen.x + 20, screen.y + 20, 100, 80))
        .expect("place windowed fixture");
    assert_eq!(window_covers_monitor(windows.parent), Some(false));

    let work = crate::win32::rect_from(info.rcWork).unwrap();
    if work != screen {
        set_rect(raw, work).expect("place work-area fixture");
        assert_eq!(window_covers_monitor(windows.parent), Some(false));
    }
    assert_eq!(
        unsafe { GetForegroundWindow() },
        foreground,
        "never take focus"
    );
}

#[test]
fn unreadable_fullscreen_bounds_are_unknown() {
    assert_eq!(super::window_covers_monitor(HWND::default()), None);
    assert_eq!(super::window_covers_monitor(super::hwnd(usize::MAX)), None);
}
