//! QQ Music quick login (Windows): the official login page runs in its own
//! WebView2 window, and the session it leaves behind becomes the UNM QQ
//! cookie. The login window loads a remote origin with no capability, so
//! Tauri rejects every IPC call from it.

use tauri::WebviewWindow;

/// Resolves with the cookie for `unmQQCookie`, or `None` when the user closes
/// the login window or one is already open (it is focused instead).
#[tauri::command]
pub async fn qq_music_login(window: WebviewWindow) -> Result<Option<String>, String> {
    // The Last.fm callback window also runs local app code and could call this.
    if window.label() != "main" {
        return Err("QQ Music login can only be opened from the main window".to_owned());
    }
    platform::login(window).await
}

#[cfg(not(windows))]
mod platform {
    pub async fn login(_owner: tauri::WebviewWindow) -> Result<Option<String>, String> {
        Err("QQ Music quick login is only available on Windows".to_owned())
    }
}

#[cfg(windows)]
mod platform {
    use std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc::{self, Receiver, RecvTimeoutError, Sender},
        },
        time::Duration,
    };

    use tauri::{
        webview::{NewWindowResponse, PageLoadEvent, PlatformWebview},
        Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
    };
    use webview2_com::{
        take_pwstr, GetCookiesCompletedHandler,
        Microsoft::Web::WebView2::Win32::{ICoreWebView2CookieList, ICoreWebView2_2},
    };
    use windows_core::{w, Interface, PWSTR};

    use super::flow::{is_allowed_url, step, BrowserCookie, Effect, Event, State};

    const WINDOW_LABEL: &str = "qq-music-login";
    const LOGIN_URL: &str = "https://y.qq.com/n/ryqq/profile";
    const DATA_DIRECTORY: &str = "qq-music-login";
    // WebView2 has no cookie-change event, and QQ completes the login inside a
    // cross-origin iframe, so no top-level page load is guaranteed to follow
    // it. Re-read the cookie store whenever the window has been quiet this long.
    const RECHECK_AFTER: Duration = Duration::from_secs(1);
    // The profile page only links to the login dialog; open it for the user.
    const OPEN_LOGIN_DIALOG: &str = "setTimeout(()=>{if(location.hostname!=='y.qq.com'||document.querySelector('#login_frame'))return;document.querySelector('a.top_login__link')?.click();},700)";

    /// Held from the start of a login until its window is destroyed, so a
    /// re-click never races a closing window for the label or data folder.
    static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

    fn release() {
        IN_FLIGHT.store(false, Ordering::Release);
    }

    pub async fn login(owner: WebviewWindow) -> Result<Option<String>, String> {
        if IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            // The pending call delivers the cookie; this one just surfaces it.
            if let Some(window) = owner.get_webview_window(WINDOW_LABEL) {
                let _ = window.set_focus();
            }
            return Ok(None);
        }
        let (events, inbox) = mpsc::channel();
        let window = open_window(&owner, events.clone())?;
        tauri::async_runtime::spawn_blocking(move || drive(&window, &events, &inbox))
            .await
            .map_err(|_| "QQ Music login stopped unexpectedly".to_owned())
    }

    /// Releases the in-flight slot itself: directly if no window was built,
    /// otherwise when that window is destroyed.
    fn open_window(owner: &WebviewWindow, events: Sender<Event>) -> Result<WebviewWindow, String> {
        let window = build_window(owner).inspect_err(|_| release())?;
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                let _ = events.send(Event::Destroyed);
                release();
            }
        });
        // Shown only now so the Destroyed handler cannot miss an early close.
        if let Err(error) = window.show() {
            let _ = window.close();
            return Err(window_error(error));
        }
        let _ = window.set_focus();
        Ok(window)
    }

    fn build_window(owner: &WebviewWindow) -> Result<WebviewWindow, String> {
        let app = owner.app_handle().clone();
        let data_directory = owner
            .path()
            .app_local_data_dir()
            .map_err(window_error)?
            .join(DATA_DIRECTORY);
        WebviewWindowBuilder::new(
            owner,
            WINDOW_LABEL,
            WebviewUrl::External(LOGIN_URL.parse().map_err(window_error)?),
        )
        .parent(owner)
        .map_err(window_error)?
        .title("QQ Music")
        .inner_size(1_040.0, 760.0)
        .center()
        .visible(false)
        // WebView2 refuses a second environment on one data folder with
        // different options, and the main window's options vary with the proxy
        // relay. InPrivate keeps the session out of this folder.
        .data_directory(data_directory)
        .incognito(true)
        .on_navigation(is_allowed_url)
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Finished
                && payload.url().host_str() == Some("y.qq.com")
            {
                let _ = webview.eval(OPEN_LOGIN_DIALOG);
            }
        })
        .on_new_window(move |url, _features| {
            // Keep QQ's login popups inside this window.
            if is_allowed_url(&url) {
                if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
                    let _ = window.navigate(url);
                }
            }
            NewWindowResponse::Deny
        })
        .build()
        .map_err(window_error)
    }

    fn window_error<E>(_: E) -> String {
        "Unable to open the QQ Music login window".to_owned()
    }

    fn drive(
        window: &WebviewWindow,
        events: &Sender<Event>,
        inbox: &Receiver<Event>,
    ) -> Option<String> {
        let mut state = State::Idle;
        loop {
            let event = match inbox.recv_timeout(RECHECK_AFTER) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => Event::Tick,
                // Unreachable while `events` is alive; ending is the safe reading.
                Err(RecvTimeoutError::Disconnected) => Event::Destroyed,
            };
            let (next, effect) = step(state, event);
            match effect {
                Some(Effect::ReadCookies) => read_cookies(window, events.clone()),
                Some(Effect::Close) => {
                    let _ = window.close();
                }
                None => {}
            }
            match next {
                State::Done(cookie) => return Some(cookie),
                State::Cancelled => return None,
                live => state = live,
            }
        }
    }

    /// Never waits on the UI thread. Unlike `cookies_for_url`, which unwraps a
    /// reply the runtime drops once the window is gone, `with_webview` only
    /// queues the closure: a closed window drops it unrun, and the Destroyed
    /// event ends the login.
    fn read_cookies(window: &WebviewWindow, events: Sender<Event>) {
        let dispatch_failed = events.clone();
        let queued = window.with_webview(move |webview| {
            let failed = events.clone();
            if start_cookie_read(&webview, events).is_err() {
                let _ = failed.send(Event::ReadFailed);
            }
        });
        if queued.is_err() {
            let _ = dispatch_failed.send(Event::ReadFailed);
        }
    }

    /// Runs on the UI thread. `GetCookies` returns at once and calls the
    /// handler later from the message loop; the result includes HttpOnly
    /// cookies.
    fn start_cookie_read(
        webview: &PlatformWebview,
        events: Sender<Event>,
    ) -> windows_core::Result<()> {
        let handler = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
            let event = result
                .and_then(|()| collect_cookies(list))
                .map_or(Event::ReadFailed, Event::Cookies);
            let _ = events.send(event);
            Ok(())
        }));
        unsafe {
            webview
                .controller()
                .CoreWebView2()?
                .cast::<ICoreWebView2_2>()?
                .CookieManager()?
                .GetCookies(w!("https://y.qq.com/"), &handler)
        }
    }

    fn collect_cookies(
        list: Option<ICoreWebView2CookieList>,
    ) -> windows_core::Result<Vec<BrowserCookie>> {
        let Some(list) = list else {
            return Ok(Vec::new());
        };
        let mut count = 0;
        unsafe { list.Count(&mut count)? };
        (0..count)
            .map(|index| unsafe {
                let cookie = list.GetValueAtIndex(index)?;
                Ok(BrowserCookie {
                    name: read_string(|out| cookie.Name(out))?,
                    domain: read_string(|out| cookie.Domain(out))?,
                    value: read_string(|out| cookie.Value(out))?,
                })
            })
            .collect()
    }

    fn read_string(
        read: impl FnOnce(*mut PWSTR) -> windows_core::Result<()>,
    ) -> windows_core::Result<String> {
        let mut value = PWSTR::null();
        read(&mut value)?;
        Ok(take_pwstr(value))
    }
}

/// Platform-independent rules of the login, unit-tested on every OS.
#[cfg_attr(not(windows), allow(dead_code))]
mod flow {
    use tauri::Url;

    /// Cookies copied into the UNM setting. Refresh and access tokens stay
    /// behind; UNM needs only the account and its music key. WeChat sign-ins
    /// keep the account in `wxuin`, and the login type tells QQ which kind of
    /// key it is reading.
    const EXPORTED_COOKIES: [&str; 6] = [
        "uin",
        "wxuin",
        "qqmusic_key",
        "qm_keyst",
        "login_type",
        "tmeLoginType",
    ];
    /// Ticks a cookie read may stay unanswered before it is issued again.
    const STALLED_READ_TICKS: u8 = 5;

    /// Pages the login window may show: QQ's own HTTPS hosts, plus the blank
    /// page WebView2 uses between navigations.
    pub fn is_allowed_url(url: &Url) -> bool {
        url.as_str() == "about:blank"
            || (url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| host == "qq.com" || host.ends_with(".qq.com")))
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct BrowserCookie {
        pub name: String,
        pub domain: String,
        pub value: String,
    }

    /// The UNM cookie, once QQ has issued both an account and a music key.
    /// When a name is set on several domains, the most specific one wins, as
    /// it would for a request to y.qq.com.
    fn session_cookie(cookies: &[BrowserCookie]) -> Option<String> {
        let value = |name: &str| {
            cookies
                .iter()
                .filter(|cookie| cookie.name == name && !cookie.value.is_empty())
                .max_by_key(|cookie| cookie.domain.trim_start_matches('.').len())
                .map(|cookie| cookie.value.as_str())
        };
        value("uin").or_else(|| value("wxuin"))?;
        value("qqmusic_key").or_else(|| value("qm_keyst"))?;
        let fields: Vec<String> = EXPORTED_COOKIES
            .iter()
            .filter_map(|name| value(name).map(|value| format!("{name}={value}")))
            .collect();
        Some(fields.join("; "))
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum State {
        Idle,
        /// A cookie read is in flight; `ticks` counts how long it has gone
        /// unanswered.
        Reading {
            ticks: u8,
        },
        Done(String),
        Cancelled,
    }

    #[derive(Debug)]
    pub enum Event {
        Tick,
        Cookies(Vec<BrowserCookie>),
        /// Transient: a closing window can fail a read before Destroyed arrives.
        ReadFailed,
        Destroyed,
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Effect {
        ReadCookies,
        Close,
    }

    pub fn step(state: State, event: Event) -> (State, Option<Effect>) {
        const READ: (State, Option<Effect>) =
            (State::Reading { ticks: 0 }, Some(Effect::ReadCookies));
        match (state, event) {
            (state @ (State::Done(_) | State::Cancelled), _) => (state, None),
            (_, Event::Destroyed) => (State::Cancelled, None),
            (State::Idle, Event::Tick) => READ,
            // A read whose callback never fires must not stop the login.
            (State::Reading { ticks }, Event::Tick) if ticks + 1 >= STALLED_READ_TICKS => READ,
            (State::Reading { ticks }, Event::Tick) => (State::Reading { ticks: ticks + 1 }, None),
            (_, Event::Cookies(cookies)) => match session_cookie(&cookies) {
                Some(cookie) => (State::Done(cookie), Some(Effect::Close)),
                None => (State::Idle, None),
            },
            (_, Event::ReadFailed) => (State::Idle, None),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn cookie(name: &str, value: &str, domain: &str) -> BrowserCookie {
            BrowserCookie {
                name: name.to_owned(),
                domain: domain.to_owned(),
                value: value.to_owned(),
            }
        }

        fn qq_login() -> Vec<BrowserCookie> {
            vec![
                cookie("uin", "o0123456", ".qq.com"),
                cookie("uin", "123456", ".y.qq.com"),
                cookie("qm_keyst", "KEY", ".y.qq.com"),
                cookie("psrf_qqrefresh_token", "REFRESH", ".qq.com"),
                cookie("psrf_qqaccess_token", "ACCESS", ".qq.com"),
                cookie("tmeLoginType", "2", ".y.qq.com"),
            ]
        }

        #[test]
        fn only_https_qq_hosts_and_blank_are_allowed() {
            for allowed in [
                "https://y.qq.com/n/ryqq/profile",
                "https://xui.ptlogin2.qq.com/cgi-bin/xlogin",
                "https://open.weixin.qq.com/connect/qrconnect",
                "https://qq.com/",
                "about:blank",
            ] {
                assert!(is_allowed_url(&allowed.parse().unwrap()), "{allowed}");
            }
            for rejected in [
                "http://y.qq.com/",
                "https://evilqq.com/",
                "https://qq.com.evil.test/",
                "https://qq.com@evil.test/",
                "https://evil.test/?next=.qq.com",
                "about:srcdoc",
                "file:///C:/qq.com",
            ] {
                assert!(!is_allowed_url(&rejected.parse().unwrap()), "{rejected}");
            }
        }

        #[test]
        fn qq_session_exports_account_key_and_login_type_only() {
            assert_eq!(
                session_cookie(&qq_login()).as_deref(),
                Some("uin=123456; qm_keyst=KEY; tmeLoginType=2")
            );
        }

        #[test]
        fn wechat_session_is_found_by_wxuin() {
            let cookies = [
                cookie("wxuin", "o0777", ".qq.com"),
                cookie("qqmusic_key", "W_X_KEY", ".qq.com"),
                cookie("wxrefresh_token", "REFRESH", ".qq.com"),
                cookie("login_type", "2", ".qq.com"),
            ];
            assert_eq!(
                session_cookie(&cookies).as_deref(),
                Some("wxuin=o0777; qqmusic_key=W_X_KEY; login_type=2")
            );
        }

        #[test]
        fn no_session_without_both_account_and_key() {
            assert_eq!(session_cookie(&[cookie("uin", "1", ".qq.com")]), None);
            assert_eq!(
                session_cookie(&[cookie("qqmusic_key", "KEY", ".qq.com")]),
                None
            );
            assert_eq!(
                session_cookie(&[
                    cookie("uin", "1", ".qq.com"),
                    cookie("qm_keyst", "", ".qq.com")
                ]),
                None
            );
        }

        #[test]
        fn ticks_read_until_a_session_appears() {
            let (state, effect) = step(State::Idle, Event::Tick);
            assert_eq!(
                (&state, effect),
                (&State::Reading { ticks: 0 }, Some(Effect::ReadCookies))
            );

            let (state, effect) = step(state, Event::Cookies(Vec::new()));
            assert_eq!((&state, effect), (&State::Idle, None));

            let (state, _) = step(state, Event::Tick);
            let (state, effect) = step(state, Event::Cookies(qq_login()));
            assert_eq!(
                (state, effect),
                (
                    State::Done("uin=123456; qm_keyst=KEY; tmeLoginType=2".to_owned()),
                    Some(Effect::Close)
                )
            );
        }

        #[test]
        fn a_stalled_read_is_issued_again() {
            let mut state = State::Reading { ticks: 0 };
            for _ in 1..STALLED_READ_TICKS {
                let (next, effect) = step(state, Event::Tick);
                assert_eq!(effect, None);
                state = next;
            }
            assert_eq!(
                step(state, Event::Tick),
                (State::Reading { ticks: 0 }, Some(Effect::ReadCookies))
            );
        }

        #[test]
        fn a_failed_read_is_retried_on_the_next_tick() {
            let (state, effect) = step(State::Reading { ticks: 2 }, Event::ReadFailed);
            assert_eq!((&state, effect), (&State::Idle, None));
            assert_eq!(step(state, Event::Tick).1, Some(Effect::ReadCookies));
        }

        #[test]
        fn closing_the_window_cancels_and_finished_logins_stay_finished() {
            for state in [State::Idle, State::Reading { ticks: 3 }] {
                assert_eq!(step(state, Event::Destroyed), (State::Cancelled, None));
            }
            let done = State::Done("uin=1".to_owned());
            assert_eq!(
                step(done, Event::Destroyed).0,
                State::Done("uin=1".to_owned())
            );
        }
    }
}
