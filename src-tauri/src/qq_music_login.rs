//! QQ Music quick login (Windows): the official login page runs in a
//! throwaway WebView2 profile, and the session cookies it leaves behind
//! become the UNM QQ cookie.

use tauri::{AppHandle, WebviewWindow};

/// Resolves with the UNM cookie, or `None` when the user closes the window.
#[tauri::command]
pub async fn qq_music_login(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<Option<String>, String> {
    if window.label() != "main" {
        return Err("QQ Music login can only be opened from the main window".to_owned());
    }

    #[cfg(target_os = "windows")]
    {
        windows::login(app, window).await
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Err("QQ Music quick login is only available on Windows".to_owned())
    }
}

/// Cookies UNM forwards to QQ, in the order they are written.
#[cfg(any(target_os = "windows", test))]
const SESSION_COOKIE_NAMES: &[&str] = &[
    "uin",
    "qqmusic_key",
    "qm_keyst",
    "psrf_qqrefresh_token",
    "qqrefresh_token",
    "psrf_musickey_createtime",
    "psrf_qqaccess_token",
    "psrf_access_token",
    "qqaccess_token",
    "psrf_access_token_expiresAt",
    "psrf_qqopenid",
    "psrf_qqunionid",
    "login_type",
    "tmeLoginType",
    "euin",
    "music_ignore_pskey",
    "wxopenid",
    "wxunionid",
    "wxrefresh_token",
];

/// A login without a refresh token settles over several redirects.
#[cfg(any(target_os = "windows", test))]
const SETTLE_TIME: std::time::Duration = std::time::Duration::from_millis(1_500);

#[cfg(any(target_os = "windows", test))]
struct Session {
    cookie: String,
    can_refresh: bool,
}

#[cfg(any(target_os = "windows", test))]
fn is_qq_url(url: &tauri::webview::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            let host = host.to_ascii_lowercase();
            host == "qq.com" || host.ends_with(".qq.com")
        })
}

/// The session in the given cookies, once QQ has issued an account and a
/// music key. Only first-party QQ Music cookies are exported.
#[cfg(any(target_os = "windows", test))]
fn session_from_cookies(cookies: &[tauri::webview::Cookie<'_>]) -> Option<Session> {
    let mut fields = std::collections::HashMap::new();
    for cookie in cookies {
        let domain = cookie.domain().unwrap_or_default().trim_start_matches('.');
        let value = cookie.value();
        if SESSION_COOKIE_NAMES.contains(&cookie.name())
            && (domain.eq_ignore_ascii_case("qq.com") || domain.eq_ignore_ascii_case("y.qq.com"))
            && !value.is_empty()
            && !value.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            fields.insert(cookie.name(), value);
        }
    }

    if !fields.get("uin")?.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // QQ issues one of the two key names depending on the login type.
    let key = fields
        .get("qqmusic_key")
        .or_else(|| fields.get("qm_keyst"))
        .copied()?;
    fields.insert("qqmusic_key", key);
    fields.insert("qm_keyst", key);

    let cookie = SESSION_COOKIE_NAMES
        .iter()
        .filter_map(|name| fields.get(name).map(|value| format!("{name}={value}")))
        .collect::<Vec<_>>()
        .join("; ");
    let can_refresh =
        fields.contains_key("psrf_qqrefresh_token") || fields.contains_key("qqrefresh_token");
    Some(Session {
        cookie,
        can_refresh,
    })
}

/// Accepts a session with a refresh token at once, and any other session
/// after it has stayed unchanged for [`SETTLE_TIME`].
#[cfg(any(target_os = "windows", test))]
fn settled_cookie(
    pending: &mut Option<(String, std::time::Instant)>,
    session: Option<Session>,
    now: std::time::Instant,
) -> Option<String> {
    let Some(session) = session else {
        *pending = None;
        return None;
    };
    if session.can_refresh {
        return Some(session.cookie);
    }
    match pending {
        Some((cookie, since)) if *cookie == session.cookie => {
            (now.duration_since(*since) >= SETTLE_TIME).then_some(session.cookie)
        }
        _ => {
            *pending = Some((session.cookie, now));
            None
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::{
        path::Path,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };

    use tauri::{
        webview::Url, AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
        WindowEvent,
    };

    use super::{is_qq_url, session_from_cookies, settled_cookie};

    const WINDOW_LABEL: &str = "qq-music-login";
    const LOGIN_URL: &str = "https://y.qq.com/n/ryqq/profile";
    const COOKIE_URL: &str = "https://y.qq.com/";
    const POLL_INTERVAL: Duration = Duration::from_millis(400);
    // The profile page only links to the login dialog; open it for the user.
    const OPEN_LOGIN_DIALOG: &str = "setTimeout(()=>{if(location.hostname!=='y.qq.com'||document.querySelector('#login_frame'))return;document.querySelector('a.top_login__link')?.click();},700)";

    pub async fn login(app: AppHandle, owner: WebviewWindow) -> Result<Option<String>, String> {
        if let Some(existing) = app.get_webview_window(WINDOW_LABEL) {
            let _ = existing.set_focus();
            return Err("QQ Music login is already open".to_owned());
        }

        let profile = tempfile::Builder::new()
            .prefix("yesplaymusic-qq-login-")
            .tempdir()
            .map_err(|_| "Unable to prepare the QQ Music login profile".to_owned())?
            .keep();
        let window = match build_window(&app, &owner, &profile) {
            Ok(window) => window,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(error);
            }
        };

        let closed = Arc::new(AtomicBool::new(false));
        let closed_flag = closed.clone();
        window.on_window_event(move |event| {
            if matches!(event, WindowEvent::Destroyed) {
                closed_flag.store(true, Ordering::Release);
                let profile = profile.clone();
                std::thread::spawn(move || remove_profile(&profile));
            }
        });

        // Deny permission prompts before any QQ page loads.
        let page = window.clone();
        let opened = window.with_webview(move |webview| {
            let opened = deny_permission_requests(webview).is_ok()
                && LOGIN_URL
                    .parse()
                    .is_ok_and(|url| page.navigate(url).is_ok())
                && page.show().is_ok();
            if opened {
                let _ = page.set_focus();
            } else {
                let _ = page.close();
            }
        });
        if opened.is_err() {
            let _ = window.close();
            return Err("Unable to open the QQ Music login page".to_owned());
        }

        tauri::async_runtime::spawn_blocking(move || {
            let cookie = wait_for_session(&window, &closed);
            let _ = window.close();
            cookie
        })
        .await
        .map_err(|_| "QQ Music login stopped unexpectedly".to_owned())
    }

    fn build_window(
        app: &AppHandle,
        owner: &WebviewWindow,
        profile: &Path,
    ) -> Result<WebviewWindow, String> {
        let popup_app = app.clone();
        WebviewWindowBuilder::new(
            app,
            WINDOW_LABEL,
            WebviewUrl::External("about:blank".parse().map_err(window_error)?),
        )
        .parent(owner)
        .map_err(window_error)?
        .title("Connect QQ Music")
        .inner_size(1_040.0, 760.0)
        .min_inner_size(800.0, 600.0)
        .center()
        .visible(false)
        .data_directory(profile.to_path_buf())
        .incognito(true)
        .on_navigation(|url| url.as_str() == "about:blank" || is_qq_url(url))
        .on_page_load(|webview, payload| {
            if payload.event() == tauri::webview::PageLoadEvent::Finished
                && payload.url().host_str() == Some("y.qq.com")
            {
                let _ = webview.eval(OPEN_LOGIN_DIALOG);
            }
        })
        .on_new_window(move |url, _features| {
            // Keep QQ's login popups inside the isolated window.
            if is_qq_url(&url) {
                if let Some(window) = popup_app.get_webview_window(WINDOW_LABEL) {
                    let _ = window.navigate(url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .build()
        .map_err(window_error)
    }

    fn window_error<E>(_: E) -> String {
        "Unable to create the QQ Music login window".to_owned()
    }

    fn wait_for_session(window: &WebviewWindow, closed: &AtomicBool) -> Option<String> {
        let cookie_url: Url = COOKIE_URL.parse().ok()?;
        let mut pending = None;
        while !closed.load(Ordering::Acquire) {
            // Cookie reads include the HttpOnly session cookies.
            let cookies = window
                .cookies_for_url(cookie_url.clone())
                .unwrap_or_default();
            if let Some(cookie) =
                settled_cookie(&mut pending, session_from_cookies(&cookies), Instant::now())
            {
                return Some(cookie);
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        None
    }

    /// WebView2 releases the profile shortly after its window is destroyed.
    fn remove_profile(profile: &Path) {
        for _ in 0..12 {
            match std::fs::remove_dir_all(profile) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        eprintln!("QQ Music login profile could not be removed");
    }

    fn deny_permission_requests(
        webview: tauri::webview::PlatformWebview,
    ) -> windows_core::Result<()> {
        use webview2_com::{
            Microsoft::Web::WebView2::Win32::{
                ICoreWebView2_12, COREWEBVIEW2_PERMISSION_STATE_DENY,
            },
            PermissionRequestedEventHandler,
        };
        use windows_core::Interface;

        let core = unsafe { webview.controller().CoreWebView2() }?.cast::<ICoreWebView2_12>()?;
        let handler = PermissionRequestedEventHandler::create(Box::new(|_sender, args| {
            if let Some(args) = args {
                unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
            }
            Ok(())
        }));
        let mut token = 0i64;
        unsafe { core.add_PermissionRequested(&handler, &mut token) }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use tauri::webview::Cookie;

    use super::{is_qq_url, session_from_cookies, settled_cookie, Session, SETTLE_TIME};

    fn cookie(name: &'static str, value: &'static str, domain: &'static str) -> Cookie<'static> {
        Cookie::build((name, value)).domain(domain).build()
    }

    fn session(cookie: &str, can_refresh: bool) -> Option<Session> {
        Some(Session {
            cookie: cookie.to_owned(),
            can_refresh,
        })
    }

    #[test]
    fn only_https_qq_hosts_are_navigable() {
        assert!(is_qq_url(
            &"https://y.qq.com/n/ryqq/profile".parse().unwrap()
        ));
        assert!(is_qq_url(&"https://graph.qq.com/".parse().unwrap()));
        assert!(!is_qq_url(&"http://y.qq.com/".parse().unwrap()));
        assert!(!is_qq_url(&"https://qq.com.evil.test/".parse().unwrap()));
    }

    #[test]
    fn sessions_export_first_party_music_cookies_with_both_key_names() {
        let session = session_from_cookies(&[
            cookie("uin", "123456789", ".qq.com"),
            cookie("qm_keyst", "key", ".y.qq.com"),
            cookie("psrf_qqrefresh_token", "refresh", ".qq.com"),
            cookie("sessionid", "unrelated", ".qq.com"),
            cookie("qqmusic_key", "foreign", ".evil.qq.com"),
        ])
        .unwrap();

        assert_eq!(
            session.cookie,
            "uin=123456789; qqmusic_key=key; qm_keyst=key; psrf_qqrefresh_token=refresh"
        );
        assert!(session.can_refresh);
    }

    #[test]
    fn incomplete_or_malformed_logins_are_not_sessions() {
        assert!(session_from_cookies(&[cookie("uin", "123", ".qq.com")]).is_none());
        assert!(session_from_cookies(&[
            cookie("uin", "o123", ".qq.com"),
            cookie("qm_keyst", "key", ".qq.com")
        ])
        .is_none());
        assert!(session_from_cookies(&[
            cookie("uin", "123", ".qq.com"),
            cookie("qm_keyst", "a key", ".qq.com")
        ])
        .is_none());
    }

    #[test]
    fn sessions_without_refresh_tokens_must_settle() {
        let start = Instant::now();
        let mut pending = None;

        assert_eq!(
            settled_cookie(&mut pending, session("a", false), start),
            None
        );
        assert_eq!(
            settled_cookie(&mut pending, session("b", false), start + SETTLE_TIME),
            None
        );
        assert_eq!(
            settled_cookie(
                &mut pending,
                session("b", false),
                start + SETTLE_TIME * 2 - Duration::from_millis(1)
            ),
            None
        );
        assert_eq!(
            settled_cookie(&mut pending, session("b", false), start + SETTLE_TIME * 2).as_deref(),
            Some("b")
        );
        assert_eq!(
            settled_cookie(&mut pending, session("c", true), start).as_deref(),
            Some("c")
        );
    }
}
