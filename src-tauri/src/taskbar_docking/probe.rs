//! Measures every taskbar with UI Automation on one long-lived MTA thread.
//!
//! Windows 11 keeps its buttons in XAML islands
//! (`Windows.UI.Composition.DesktopWindowContentBridge`); the classic
//! `MSTaskListWClass` under `Shell_TrayWnd` is a hidden leftover and is
//! never consulted once an island exists. Older or third-party taskbars
//! still use `ReBarWindow32 > MSTaskSwWClass > MSTaskListWClass`.
//!
//! Every read is a `Result`: a taskbar that cannot be measured fails the
//! whole probe rather than passing as empty, so the strip never lands on
//! buttons the read missed.

use std::{sync::mpsc, thread};

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::HWND,
        System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
        UI::{
            Accessibility::{
                AutomationElementMode_None, CUIAutomation8, IUIAutomation2,
                IUIAutomationCacheRequest, IUIAutomationCondition, TreeScope_Descendants,
                UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
                UIA_ClassNamePropertyId,
            },
            HiDpi::{
                GetDpiForWindow, SetThreadDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Shell::{SHAppBarMessage, ABM_GETSTATE, ABS_AUTOHIDE, APPBARDATA},
            WindowsAndMessaging::{FindWindowExW, IsWindowVisible},
        },
    },
};

use super::model::{Rect, Taskbar};
use crate::win32::{rect_from, taskbar_windows, window_rect, Apartment};

/// Upper bound on any single call into Explorer's automation provider.
pub const TIMEOUT_MS: u32 = 1500;

const BRIDGE_CLASS: PCWSTR = w!("Windows.UI.Composition.DesktopWindowContentBridge");
const TASK_LIST_BUTTON_CLASS: &str = "Taskbar.TaskListButtonAutomationPeer";
const CONTENT_IDS: [&str; 5] = [
    "StartButton",
    "SearchButton",
    "SearchBoxTextBlock",
    "TaskViewButton",
    "ChatButton",
];
const WIDGETS_ID: &str = "WidgetsButton";
const TRAY_IDS: [&str; 3] = ["SystemTrayIcon", "NotifyItemIcon", "ClockButton"];

pub type Measurement = Result<Vec<Taskbar>, String>;

/// One probe thread; every answer carries the thread's generation and the
/// request serial, so an answer from a thread that was already retired
/// (docking turned off and on again) can never pass for a current one.
pub struct ProbeThread {
    requests: mpsc::Sender<u64>,
}

impl ProbeThread {
    pub fn spawn(
        generation: u64,
        report: impl Fn(u64, u64, Measurement) + Send + 'static,
    ) -> Result<Self, String> {
        let (requests, inbox) = mpsc::channel::<u64>();
        thread::Builder::new()
            .name("taskbar-probe".into())
            .spawn(move || {
                let prober = Prober::new();
                for serial in inbox {
                    let measurement = match &prober {
                        Ok(prober) => prober.measure(),
                        Err(error) => Err(error.clone()),
                    };
                    report(generation, serial, measurement);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self { requests })
    }

    pub fn request(&self, serial: u64) -> Result<(), String> {
        self.requests
            .send(serial)
            .map_err(|_| "the taskbar probe thread is gone".to_owned())
    }
}

struct Prober {
    automation: IUIAutomation2,
    any: IUIAutomationCondition,
    cache: IUIAutomationCacheRequest,
    /// Last field on purpose: `CoUninitialize` must follow every release above.
    _apartment: Apartment,
}

/// One element as read from the cache in a single round trip.
struct Item {
    rect: Rect,
    id: String,
    class: String,
}

impl Prober {
    fn new() -> Result<Self, String> {
        let apartment = Apartment::enter().ok_or("could not join the COM apartment")?;
        // Without this, coordinates on scaled displays come back virtualised.
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        let build = || -> windows::core::Result<(IUIAutomation2, IUIAutomationCondition, IUIAutomationCacheRequest)> {
            let automation: IUIAutomation2 =
                unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }?;
            // A wedged provider returns an error instead of hanging the thread.
            unsafe { automation.SetConnectionTimeout(TIMEOUT_MS) }?;
            unsafe { automation.SetTransactionTimeout(TIMEOUT_MS) }?;
            let any = unsafe { automation.CreateTrueCondition() }?;
            let cache = unsafe { automation.CreateCacheRequest() }?;
            unsafe { cache.AddProperty(UIA_BoundingRectanglePropertyId) }?;
            unsafe { cache.AddProperty(UIA_AutomationIdPropertyId) }?;
            unsafe { cache.AddProperty(UIA_ClassNamePropertyId) }?;
            // Found elements carry cached data only, no live reference each;
            // the roots they are searched from are fetched live.
            unsafe { cache.SetAutomationElementMode(AutomationElementMode_None) }?;
            Ok((automation, any, cache))
        };
        let (automation, any, cache) =
            build().map_err(|error| format!("UI Automation unavailable: {error}"))?;
        Ok(Self {
            automation,
            any,
            cache,
            _apartment: apartment,
        })
    }

    fn measure(&self) -> Measurement {
        let auto_hide = auto_hide_enabled();
        taskbar_windows()
            .into_iter()
            .map(|taskbar| self.measure_taskbar(taskbar, auto_hide))
            .collect()
    }

    fn measure_taskbar(&self, taskbar: HWND, auto_hide: bool) -> Result<Taskbar, String> {
        let bounds = window_rect(taskbar).ok_or("the taskbar has no bounds")?;
        let dpi = unsafe { GetDpiForWindow(taskbar) };
        if dpi == 0 {
            return Err("the taskbar reports no DPI".to_owned());
        }
        let bridges = child_windows(taskbar, BRIDGE_CLASS);
        let (container, content, widgets, tray) = if bridges.is_empty() {
            self.measure_legacy(taskbar, bounds)?
        } else {
            let mut items = Vec::new();
            for bridge in bridges {
                items.extend(self.items_under(bridge, bounds)?);
            }
            Self::classify_island(&items)?
        };
        Ok(Taskbar {
            id: taskbar.0 as usize,
            bounds,
            dpi,
            container: container.unwrap_or(bounds),
            content,
            widgets,
            tray,
            auto_hide,
        })
    }

    /// Windows 11: every button and tray item is an island element. The
    /// automation ids are hints; anything right of the buttons is tray.
    fn classify_island(items: &[Item]) -> Result<Layout, String> {
        if items.is_empty() {
            return Err("the taskbar island reported no elements".to_owned());
        }
        let mut content = None;
        let mut widgets = None;
        let mut tray = None;
        for item in items {
            if item.id == WIDGETS_ID {
                widgets = Some(union(widgets, item.rect));
            } else if TRAY_IDS.contains(&item.id.as_str()) {
                tray = Some(union(tray, item.rect));
            } else if CONTENT_IDS.contains(&item.id.as_str())
                || item.class == TASK_LIST_BUTTON_CLASS
            {
                content = Some(union(content, item.rect));
            }
        }
        let content = content.ok_or("no taskbar buttons were found")?;
        for item in items {
            if item.rect.x >= content.right() {
                tray = Some(union(tray, item.rect));
            }
        }
        Ok((None, Some(content), widgets, tray))
    }

    /// Windows 10 and third-party shells: the task list is a real window and
    /// its buttons are its automation descendants; none running is normal.
    fn measure_legacy(&self, taskbar: HWND, bounds: Rect) -> Result<Layout, String> {
        let list = legacy_task_list(taskbar)
            .filter(|list| unsafe { IsWindowVisible(*list) }.as_bool())
            .ok_or("no visible task list window")?;
        let container = window_rect(list).ok_or("the task list has no bounds")?;
        let content = self
            .items_under(list, bounds)?
            .iter()
            .fold(None, |content, item| Some(union(content, item.rect)));
        let tray = find_child(taskbar, w!("TrayNotifyWnd"))
            .filter(|tray| unsafe { IsWindowVisible(*tray) }.as_bool())
            .and_then(window_rect);
        Ok((Some(container), content, None, tray))
    }

    /// Every automation descendant of `window`, clipped to the taskbar;
    /// elements outside it (flyouts, hidden rows) do not count.
    fn items_under(&self, window: HWND, bounds: Rect) -> Result<Vec<Item>, String> {
        let read = || -> windows::core::Result<Vec<Item>> {
            // A live root: a cache-only element has nothing to search from.
            let root = unsafe { self.automation.ElementFromHandle(window) }?;
            let found =
                unsafe { root.FindAllBuildCache(TreeScope_Descendants, &self.any, &self.cache) }?;
            let count = unsafe { found.Length() }?;
            let mut items = Vec::new();
            for index in 0..count {
                let element = unsafe { found.GetElement(index) }?;
                let rect = unsafe { element.CachedBoundingRectangle() }?;
                let id = unsafe { element.CachedAutomationId() }?.to_string();
                let class = unsafe { element.CachedClassName() }?.to_string();
                if let Some(rect) = rect_from(rect).and_then(|rect| rect.clip(bounds)) {
                    items.push(Item { rect, id, class });
                }
            }
            Ok(items)
        };
        read().map_err(|error| format!("could not read the taskbar elements: {error}"))
    }
}

/// `(container, content, widgets, tray)`; `container` defaults to the bar.
type Layout = (Option<Rect>, Option<Rect>, Option<Rect>, Option<Rect>);

fn union(current: Option<Rect>, rect: Rect) -> Rect {
    current.map_or(rect, |current| current.union(rect))
}

/// `ReBarWindow32 > MSTaskSwWClass > MSTaskListWClass`, with each level
/// optional on patched shells.
fn legacy_task_list(taskbar: HWND) -> Option<HWND> {
    let rebar =
        find_child(taskbar, w!("ReBarWindow32")).or_else(|| find_child(taskbar, w!("WorkerW")))?;
    let Some(switcher) = find_child(rebar, w!("MSTaskSwWClass")) else {
        return find_child(rebar, w!("MSTaskListWClass"));
    };
    find_child(switcher, w!("MSTaskListWClass")).or(Some(switcher))
}

fn find_child(parent: HWND, class: PCWSTR) -> Option<HWND> {
    child_windows(parent, class).into_iter().next()
}

pub fn child_windows(parent: HWND, class: PCWSTR) -> Vec<HWND> {
    let mut children = Vec::new();
    let mut cursor = HWND::default();
    while let Ok(child) = unsafe { FindWindowExW(Some(parent), Some(cursor), class, None) } {
        if child.0.is_null() {
            break;
        }
        children.push(child);
        cursor = child;
    }
    children
}

pub fn bridge_windows(taskbar: HWND) -> Vec<HWND> {
    child_windows(taskbar, BRIDGE_CLASS)
}

fn auto_hide_enabled() -> bool {
    let mut data = APPBARDATA {
        cbSize: u32::try_from(std::mem::size_of::<APPBARDATA>()).unwrap_or(0),
        ..Default::default()
    };
    let state = unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) };
    u32::try_from(state).is_ok_and(|state| state & ABS_AUTOHIDE != 0)
}
