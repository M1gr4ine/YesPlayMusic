//! Win32 side of docking: strip window operations, and the resident
//! watcher thread that turns our own move loop, foreground changes and
//! Explorer's taskbar changes into controller events.
//!
//! Nothing here polls. Drag boundaries come from `EVENT_SYSTEM_MOVESIZESTART`
//! / `END` on this process, full-screen apps from `EVENT_SYSTEM_FOREGROUND`,
//! taskbar moves and re-ordering from Explorer's taskbar thread, button or
//! tray changes from UI Automation structure events, and Explorer restarts
//! from the `TaskbarCreated` broadcast on a hidden window.
//!
//! The watcher thread lives as long as the app and is told to enable or
//! disable; every hook, handler and window is installed and torn down on
//! that one thread, in order, so an enable can never overlap the previous
//! teardown and nothing on the UI thread ever joins it.
//!
//! Window calls here (`SetWindowPos`, `ShowWindow`, `GetWindowRect`) only
//! ever target windows this process created, whose UI thread never waits
//! on the docking worker, so their synchronous message round trips cannot
//! deadlock; `DwmFlush` waits for one composition frame at most.

use std::{
    sync::{
        atomic::{AtomicU32, AtomicUsize, Ordering},
        mpsc, Mutex,
    },
    thread,
    time::Duration,
};

use windows::{
    core::{w, Result as WinResult},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Dwm::DwmFlush,
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, SAFEARRAY},
            LibraryLoader::GetModuleHandleW,
            Threading::{GetCurrentProcessId, GetCurrentThreadId},
        },
        UI::{
            Accessibility::{
                CUIAutomation8, IUIAutomation2, IUIAutomationElement,
                IUIAutomationStructureChangedEventHandler,
                IUIAutomationStructureChangedEventHandler_Impl, SetWinEventHook,
                StructureChangeType, TreeScope_Subtree, UnhookWinEvent, HWINEVENTHOOK,
            },
            HiDpi::{
                GetDpiForWindow, SetThreadDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Shell::{
                SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE,
                QUNS_RUNNING_D3D_FULL_SCREEN,
            },
            WindowsAndMessaging::{
                ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, GetClassNameW, GetClientRect, GetGUIThreadInfo, GetMessageW,
                GetWindowThreadProcessId, KillTimer, PeekMessageW, PostThreadMessageW,
                RegisterClassW, RegisterWindowMessageW, SetTimer, SetWindowPos, ShowWindow,
                TranslateMessage, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_REORDER,
                EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART,
                GUITHREADINFO, GUI_INMOVESIZE, HWND_TOP, HWND_TOPMOST, MSG, MSGFLT_ALLOW,
                OBJID_WINDOW, PM_NOREMOVE, SET_WINDOW_POS_FLAGS, SPI_SETWORKAREA,
                SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
                SW_HIDE, WINDOW_EX_STYLE, WINEVENT_OUTOFCONTEXT, WM_APP, WM_DISPLAYCHANGE, WM_QUIT,
                WM_SETTINGCHANGE, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
            },
        },
    },
};

use super::{
    model::{Rect, COMPACT_BAR_MAX_HEIGHT},
    probe::{bridge_windows, TIMEOUT_MS},
};
use crate::win32::{self, hwnd, taskbar_windows};

pub fn set_rect(raw: usize, rect: Rect) -> Result<(), String> {
    win32::set_rect(hwnd(raw), rect)
}

pub fn window_rect(raw: usize) -> Option<Rect> {
    win32::window_rect(hwnd(raw))
}

/// Shows above every window without taking focus from the foreground app.
pub fn show_topmost(raw: usize) -> Result<(), String> {
    z_order(raw, HWND_TOPMOST, SWP_SHOWWINDOW)
}

/// Explorer re-asserts the taskbar's top-most position after clicks. The
/// strip is already top-most, so `HWND_TOP` is what moves it to the front
/// of that band; `HWND_TOPMOST` would leave its order untouched. Posted,
/// not sent: nothing waits on the new order.
pub fn raise(raw: usize) -> Result<(), String> {
    z_order(raw, HWND_TOP, SWP_NOMOVE | SWP_ASYNCWINDOWPOS)
}

fn z_order(raw: usize, after: HWND, extra: SET_WINDOW_POS_FLAGS) -> Result<(), String> {
    unsafe {
        SetWindowPos(
            hwnd(raw),
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | extra,
        )
    }
    .map_err(|error| format!("could not raise the taskbar strip: {error}"))
}

pub fn hide(raw: usize) {
    let _ = unsafe { ShowWindow(hwnd(raw), SW_HIDE) };
}

/// Paces flight frames with composition; falls back to a short sleep when
/// DWM is unavailable (remote sessions).
pub fn wait_frame() {
    if unsafe { DwmFlush() }.is_err() {
        thread::sleep(Duration::from_millis(8));
    }
}

/// The window's thread is inside a move or size loop right now.
pub fn in_move_loop(raw: usize) -> bool {
    let thread = unsafe { GetWindowThreadProcessId(hwnd(raw), None) };
    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(std::mem::size_of::<GUITHREADINFO>()).unwrap_or(0),
        ..Default::default()
    };
    unsafe { GetGUIThreadInfo(thread, &mut info) }.is_ok() && info.flags.contains(GUI_INMOVESIZE)
}

/// The frame and bar-ness of a window as of now, read directly rather than
/// through window events that may still be queued.
pub fn frame_now(raw: usize) -> (Option<Rect>, Option<bool>) {
    let window = hwnd(raw);
    let frame = win32::window_rect(window);
    let mut client = RECT::default();
    let dpi = unsafe { GetDpiForWindow(window) };
    let compact = (unsafe { GetClientRect(window, &mut client) }.is_ok() && dpi > 0).then(|| {
        f64::from(client.bottom - client.top) * 96.0 / f64::from(dpi) < COMPACT_BAR_MAX_HEIGHT
    });
    (frame, compact)
}

/// A full-screen app or presentation owns the display.
pub fn fullscreen_foreground() -> bool {
    matches!(
        unsafe { SHQueryUserNotificationState() },
        Ok(QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookEvent {
    DragStarted,
    DragEnded {
        frame: Option<Rect>,
        compact: Option<bool>,
    },
    TaskbarChanged,
    TaskbarRaised,
    Foreground {
        fullscreen: bool,
    },
    ExplorerRestarted,
}

pub type Sink = Box<dyn Fn(HookEvent) + Send + Sync>;

// Hook procedures are plain function pointers, so the one controller per
// process reaches them through statics.
static SINK: Mutex<Option<Sink>> = Mutex::new(None);
static MAIN_HWND: AtomicUsize = AtomicUsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

/// `wParam` 1 enables, 0 disables.
const WM_WATCH: u32 = WM_APP + 1;
/// Re-target the Explorer hooks; `wParam` 1 forces it even when nothing changed.
const WM_REHOOK: u32 = WM_APP + 2;
const REHOOK_TIMER: usize = 1;
const REHOOK_RETRY: Duration = Duration::from_secs(1);
const REHOOK_ATTEMPTS: u32 = 5;
const TASKBAR_CLASSES: [&str; 4] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "TrayNotifyWnd",
    "ReBarWindow32",
];

fn notify(event: HookEvent) {
    if let Some(sink) = SINK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
    {
        sink(event);
    }
}

/// Handle to the resident watcher thread. It starts disabled; dropping it
/// ends the thread after a final teardown.
pub struct Watcher {
    thread: u32,
}

impl Watcher {
    pub fn spawn(main: usize, sink: Sink) -> Result<Self, String> {
        *SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
        MAIN_HWND.store(main, Ordering::Release);
        let (ready, started) = mpsc::channel();
        thread::Builder::new()
            .name("taskbar-watch".into())
            .spawn(move || watch_loop(&ready))
            .map_err(|error| error.to_string())?;
        // Nothing but the thread start gates this; no hook is set yet.
        let thread = started
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| "the taskbar watcher did not start".to_owned())?;
        Ok(Self { thread })
    }

    pub fn enable(&self) {
        self.post(WM_WATCH, 1);
    }

    pub fn disable(&self) {
        self.post(WM_WATCH, 0);
    }

    /// Re-targets the Explorer hooks if its process or its taskbar windows
    /// changed since they were set (restart whose broadcast was missed, a
    /// monitor plugged in).
    pub fn refresh_explorer(&self) {
        self.post(WM_REHOOK, 0);
    }

    fn post(&self, message: u32, wparam: usize) {
        let _ = unsafe { PostThreadMessageW(self.thread, message, WPARAM(wparam), LPARAM(0)) };
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.post(WM_QUIT, 0);
    }
}

fn watch_loop(ready: &mpsc::Sender<u32>) {
    let _apartment = win32::Apartment::enter();
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let mut message = MSG::default();
    // Creates the thread's message queue so posted messages are not lost.
    let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
    if ready.send(unsafe { GetCurrentThreadId() }).is_err() {
        return;
    }

    let mut installed: Option<Installed> = None;
    let mut attempts = 0;
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
        match message.message {
            WM_WATCH => {
                if message.wParam.0 == 0 {
                    installed = None;
                } else if installed.is_none() {
                    installed = Some(Installed::new());
                    attempts = 0;
                }
            }
            WM_REHOOK => {
                if let Some(installed) = &mut installed {
                    if message.wParam.0 != 0 {
                        installed.explorer.rehook();
                    } else {
                        installed.explorer.rehook_if_changed();
                    }
                    // Explorer's island may not answer UI Automation yet; try again.
                    if installed.explorer.incomplete() && attempts < REHOOK_ATTEMPTS {
                        attempts += 1;
                        if let Some(window) = installed.window {
                            let elapse =
                                u32::try_from(REHOOK_RETRY.as_millis()).unwrap_or(u32::MAX);
                            unsafe { SetTimer(Some(window), REHOOK_TIMER, elapse, None) };
                        }
                    } else if !installed.explorer.incomplete() {
                        attempts = 0;
                    }
                }
            }
            _ => {
                let _ = unsafe { TranslateMessage(&message) };
                unsafe { DispatchMessageW(&message) };
            }
        }
    }
    drop(installed);
}

/// Everything one enable sets up; dropping it removes all of it.
struct Installed {
    own: HWINEVENTHOOK,
    foreground: HWINEVENTHOOK,
    window: Option<HWND>,
    explorer: ExplorerWatch,
}

impl Installed {
    fn new() -> Self {
        let own = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_MOVESIZESTART,
                EVENT_SYSTEM_MOVESIZEEND,
                None,
                Some(move_size_proc),
                GetCurrentProcessId(),
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        let foreground = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(foreground_proc),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        let window = create_watch_window();
        let explorer = ExplorerWatch::hook();
        Self {
            own,
            foreground,
            window,
            explorer,
        }
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        self.explorer.unhook();
        if let Some(window) = self.window.take() {
            let _ = unsafe { KillTimer(Some(window), REHOOK_TIMER) };
            let _ = unsafe { DestroyWindow(window) };
        }
        unhook(self.own);
        unhook(self.foreground);
    }
}

fn unhook(hook: HWINEVENTHOOK) {
    if !hook.0.is_null() {
        let _ = unsafe { UnhookWinEvent(hook) };
    }
}

/// A hidden top-level window: `TaskbarCreated` is broadcast to those only,
/// never to message-only windows.
fn create_watch_window() -> Option<HWND> {
    let class = w!("YesPlayMusicTaskbarWatch");
    let instance = unsafe { GetModuleHandleW(None) }.ok()?;
    let descriptor = WNDCLASSW {
        lpfnWndProc: Some(watch_proc),
        hInstance: instance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    // Registering again after a disable fails harmlessly; the class stays.
    unsafe { RegisterClassW(&descriptor) };
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("YesPlayMusic taskbar watch"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
    .ok()?;
    let created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED.store(created, Ordering::Release);
    // Explorer runs at medium integrity; let its broadcast through if we are elevated.
    let _ = unsafe { ChangeWindowMessageFilterEx(window, created, MSGFLT_ALLOW, None) };
    Some(window)
}

unsafe extern "system" fn watch_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let created = TASKBAR_CREATED.load(Ordering::Acquire);
    match message {
        WM_DISPLAYCHANGE => {
            notify(HookEvent::TaskbarChanged);
            let _ = unsafe {
                PostThreadMessageW(GetCurrentThreadId(), WM_REHOOK, WPARAM(0), LPARAM(0))
            };
        }
        WM_SETTINGCHANGE if wparam.0 == SPI_SETWORKAREA.0 as usize => {
            notify(HookEvent::TaskbarChanged);
        }
        WM_TIMER if wparam.0 == REHOOK_TIMER => {
            let _ = unsafe { KillTimer(Some(window), REHOOK_TIMER) };
            let _ = unsafe {
                PostThreadMessageW(GetCurrentThreadId(), WM_REHOOK, WPARAM(1), LPARAM(0))
            };
        }
        _ if created != 0 && message == created => {
            notify(HookEvent::ExplorerRestarted);
            let _ = unsafe {
                PostThreadMessageW(GetCurrentThreadId(), WM_REHOOK, WPARAM(1), LPARAM(0))
            };
        }
        _ => return unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
    LRESULT(0)
}

unsafe extern "system" fn move_size_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let main = MAIN_HWND.load(Ordering::Acquire);
    if window.0 as usize != main {
        return;
    }
    match event {
        EVENT_SYSTEM_MOVESIZESTART => notify(HookEvent::DragStarted),
        EVENT_SYSTEM_MOVESIZEEND => {
            let (frame, compact) = frame_now(main);
            notify(HookEvent::DragEnded { frame, compact });
        }
        _ => {}
    }
}

unsafe extern "system" fn foreground_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    notify(HookEvent::Foreground {
        fullscreen: fullscreen_foreground(),
    });
}

unsafe extern "system" fn taskbar_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if object != OBJID_WINDOW.0 {
        return;
    }
    match event {
        // The desktop re-ordered its children: Explorer raised the taskbar.
        EVENT_OBJECT_REORDER => notify(HookEvent::TaskbarRaised),
        EVENT_OBJECT_LOCATIONCHANGE if is_taskbar_class(window) => {
            notify(HookEvent::TaskbarChanged);
        }
        _ => {}
    }
}

fn is_taskbar_class(window: HWND) -> bool {
    let mut buffer = [0u16; 64];
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    let Ok(length) = usize::try_from(length) else {
        return false;
    };
    let class = &buffer[..length.min(buffer.len())];
    TASKBAR_CLASSES
        .iter()
        .any(|known| known.encode_utf16().eq(class.iter().copied()))
}

#[windows_core::implement(IUIAutomationStructureChangedEventHandler)]
struct StructureHandler;

impl IUIAutomationStructureChangedEventHandler_Impl for StructureHandler_Impl {
    fn HandleStructureChangedEvent(
        &self,
        _sender: windows_core::Ref<'_, IUIAutomationElement>,
        _change: StructureChangeType,
        _runtime_id: *const SAFEARRAY,
    ) -> WinResult<()> {
        // Single, bulk, invalidated or reordered: any of them can move the
        // free band, and the controller debounces the burst.
        notify(HookEvent::TaskbarChanged);
        Ok(())
    }
}

/// Hooks bound to the running Explorer: a WinEvent hook on its taskbar
/// thread and structure-changed handlers on each taskbar's button frame
/// and island.
struct ExplorerWatch {
    pid: u32,
    /// The taskbar and island windows the handlers were registered on.
    roots: Vec<usize>,
    events: HWINEVENTHOOK,
    automation: Option<IUIAutomation2>,
}

impl ExplorerWatch {
    fn hook() -> Self {
        let (pid, thread) = explorer_taskbar_thread();
        let roots = structure_roots();
        let events = if pid == 0 {
            HWINEVENTHOOK::default()
        } else {
            unsafe {
                SetWinEventHook(
                    EVENT_OBJECT_REORDER,
                    EVENT_OBJECT_LOCATIONCHANGE,
                    None,
                    Some(taskbar_proc),
                    pid,
                    thread,
                    WINEVENT_OUTOFCONTEXT,
                )
            }
        };
        let automation = (pid != 0).then(|| watch_structure(&roots)).flatten();
        Self {
            pid,
            roots,
            events,
            automation,
        }
    }

    /// Explorer is there but its automation tree was not: worth retrying.
    fn incomplete(&self) -> bool {
        self.pid != 0 && self.automation.is_none()
    }

    fn rehook(&mut self) {
        self.unhook();
        *self = Self::hook();
    }

    fn rehook_if_changed(&mut self) {
        if explorer_taskbar_thread().0 != self.pid
            || structure_roots() != self.roots
            || self.incomplete()
        {
            self.rehook();
        }
    }

    fn unhook(&mut self) {
        unhook(self.events);
        self.events = HWINEVENTHOOK::default();
        if let Some(automation) = self.automation.take() {
            let _ = unsafe { automation.RemoveAllEventHandlers() };
        }
    }
}

/// `(process, thread)` owning the primary taskbar, or zeros without one.
fn explorer_taskbar_thread() -> (u32, u32) {
    let Some(taskbar) = taskbar_windows().into_iter().next() else {
        return (0, 0);
    };
    let mut pid = 0;
    let thread = unsafe { GetWindowThreadProcessId(taskbar, Some(&mut pid)) };
    (pid, thread)
}

/// The windows whose automation trees hold buttons and tray items: every
/// island of every taskbar, or the taskbar itself without islands.
fn structure_roots() -> Vec<usize> {
    let mut roots = Vec::new();
    for taskbar in taskbar_windows() {
        let bridges = bridge_windows(taskbar);
        if bridges.is_empty() {
            roots.push(taskbar.0 as usize);
        } else {
            roots.extend(bridges.into_iter().map(|bridge| bridge.0 as usize));
        }
    }
    roots
}

/// Subscribes to the whole subtree of every island (and of a legacy
/// taskbar). On Windows 11 the button frame and the tray controls are
/// siblings under the island root, so nothing narrower sees both; the
/// island is small and the controller debounces the bursts anyway.
fn watch_structure(roots: &[usize]) -> Option<IUIAutomation2> {
    let automation: IUIAutomation2 =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.ok()?;
    // Registration calls into Explorer; bound them like the probe does.
    unsafe { automation.SetConnectionTimeout(TIMEOUT_MS) }.ok()?;
    unsafe { automation.SetTransactionTimeout(TIMEOUT_MS) }.ok()?;
    let handler: IUIAutomationStructureChangedEventHandler = StructureHandler.into();
    let mut registered = false;
    for &root in roots {
        let Ok(element) = (unsafe { automation.ElementFromHandle(hwnd(root)) }) else {
            continue;
        };
        registered |= unsafe {
            automation.AddStructureChangedEventHandler(&element, TreeScope_Subtree, None, &handler)
        }
        .is_ok();
    }
    registered.then_some(automation)
}
