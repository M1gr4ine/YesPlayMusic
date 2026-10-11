use serde::Serialize;
#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, State, WebviewWindow};
#[cfg(target_os = "windows")]
use tauri::{Emitter, Manager, WebviewWindowBuilder};

const MAIN_WINDOW_LABEL: &str = "main";
#[cfg(target_os = "windows")]
const AUTH_WINDOW_LABEL: &str = "qq-music-login-auth";
#[cfg(target_os = "windows")]
const LOGIN_URL: &str = "https://y.qq.com/n/ryqq/profile";
#[cfg(target_os = "windows")]
const COOKIE_URL: &str = "https://y.qq.com/";
#[cfg(target_os = "windows")]
const CANDIDATE_EVENT: &str = "qq-music-login://candidate";
#[cfg(any(target_os = "windows", test))]
const COOKIE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(400);
#[cfg(any(target_os = "windows", test))]
const COOKIE_STABILITY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1_500);
#[cfg(any(target_os = "windows", test))]
const PROFILE_CLEANUP_ATTEMPTS: usize = 13;
#[cfg(any(target_os = "windows", test))]
const PROFILE_CLEANUP_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

#[cfg(any(target_os = "windows", test))]
const QQ_COOKIE_NAMES: &[&str] = &[
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

#[derive(Clone, Default)]
pub struct QqMusicLoginState {
    #[cfg(target_os = "windows")]
    inner: Arc<Mutex<Option<LoginSession>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenQqMusicLoginResponse {
    attempt_id: String,
    reused: bool,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct QqMusicLoginCandidateEvent {
    attempt_id: String,
    value: String,
    can_refresh: bool,
}

#[cfg(any(target_os = "windows", test))]
struct CookieField {
    name: String,
    value: String,
    domain: Option<String>,
}

#[cfg(any(target_os = "windows", test))]
struct NormalizedCookie {
    value: String,
    can_refresh: bool,
}

pub fn is_main_window_label(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL
}

#[cfg(any(target_os = "windows", test))]
fn is_allowed_qq_cookie_domain(domain: Option<&str>) -> bool {
    let normalized = domain
        .unwrap_or_default()
        .strip_prefix('.')
        .unwrap_or(domain.unwrap_or_default())
        .to_ascii_lowercase();
    normalized == "qq.com" || normalized == "y.qq.com"
}

#[cfg(any(target_os = "windows", test))]
fn is_official_qq_url(url: &tauri::webview::Url) -> bool {
    if url.scheme() != "https" {
        return false;
    }
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("qq.com") || host.to_ascii_lowercase().ends_with(".qq.com")
    })
}

#[cfg(any(target_os = "windows", test))]
fn is_auth_navigation_url(url: &tauri::webview::Url) -> bool {
    url.as_str() == "about:blank" || is_official_qq_url(url)
}

#[cfg(any(target_os = "windows", test))]
fn normalize_qq_cookies(cookies: &[CookieField]) -> Option<NormalizedCookie> {
    let mut fields = std::collections::HashMap::<String, String>::new();
    for cookie in cookies {
        if !QQ_COOKIE_NAMES.contains(&cookie.name.as_str())
            || !is_allowed_qq_cookie_domain(cookie.domain.as_deref())
            || cookie.value.is_empty()
            || cookie
                .value
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            continue;
        }
        fields.insert(cookie.name.clone(), cookie.value.clone());
    }

    let key = fields
        .get("qqmusic_key")
        .or_else(|| fields.get("qm_keyst"))
        .cloned()?;
    if !fields
        .get("uin")
        .is_some_and(|uin| !uin.is_empty() && uin.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    fields.insert("qqmusic_key".to_owned(), key.clone());
    fields.insert("qm_keyst".to_owned(), key);

    let can_refresh =
        fields.contains_key("psrf_qqrefresh_token") || fields.contains_key("qqrefresh_token");
    let value = QQ_COOKIE_NAMES
        .iter()
        .filter_map(|name| fields.get(*name).map(|value| format!("{name}={value}")))
        .collect::<Vec<_>>()
        .join("; ");
    Some(NormalizedCookie { value, can_refresh })
}

#[tauri::command]
pub async fn open_qq_music_login(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, QqMusicLoginState>,
) -> Result<OpenQqMusicLoginResponse, String> {
    if !is_main_window_label(window.label()) {
        return Err("QQ Music login can only be opened from the main window".to_owned());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (app, state);
        Err("QQ Music quick login is only available on Windows".to_owned())
    }

    #[cfg(target_os = "windows")]
    {
        open_windows_login(window, app, state.inner.clone()).await
    }
}

#[tauri::command]
pub async fn finish_qq_music_login(
    window: WebviewWindow,
    state: State<'_, QqMusicLoginState>,
    attempt_id: String,
    value: Option<String>,
    committed: bool,
) -> Result<(), String> {
    if !is_main_window_label(window.label()) {
        return Err("QQ Music login can only be completed from the main window".to_owned());
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (state, attempt_id, value, committed);
        Err("QQ Music quick login is only available on Windows".to_owned())
    }

    #[cfg(target_os = "windows")]
    {
        let auth_window = {
            let mut active = state
                .inner
                .lock()
                .map_err(|_| "QQ Music login is unavailable".to_owned())?;
            let session = active
                .as_mut()
                .filter(|session| session.attempt_id == attempt_id)
                .ok_or_else(|| "QQ Music login attempt is no longer active".to_owned())?;

            if committed {
                let candidate = session
                    .candidate
                    .as_ref()
                    .ok_or_else(|| "QQ Music login candidate is no longer active".to_owned())?;
                if value.as_deref() != Some(candidate.value.as_str()) {
                    return Err("QQ Music login candidate did not match".to_owned());
                }
            }
            session
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            session.window.clone()
        };
        auth_window
            .close()
            .map_err(|_| "Unable to close the QQ Music login window".to_owned())
    }
}

#[cfg(target_os = "windows")]
struct LoginSession {
    attempt_id: String,
    window: WebviewWindow,
    candidate: Option<NormalizedCookie>,
    ready: bool,
    _temporary_profile: tempfile::TempDir,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    poller: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "windows")]
async fn open_windows_login(
    owner: WebviewWindow,
    app: AppHandle,
    state: Arc<Mutex<Option<LoginSession>>>,
) -> Result<OpenQqMusicLoginResponse, String> {
    let login_url = LOGIN_URL
        .parse()
        .map_err(|_| "Unable to prepare the QQ Music login URL".to_owned())?;
    {
        let active = state
            .lock()
            .map_err(|_| "QQ Music login is unavailable".to_owned())?;
        if let Some(session) = active.as_ref() {
            if !is_login_attempt_active(&session.cancelled)
                || app.get_webview_window(AUTH_WINDOW_LABEL).is_none()
            {
                return Err("The previous QQ Music login window is still closing".to_owned());
            }
            if session.ready {
                session
                    .window
                    .show()
                    .map_err(|_| "Unable to show the QQ Music login window".to_owned())?;
                session
                    .window
                    .set_focus()
                    .map_err(|_| "Unable to focus the QQ Music login window".to_owned())?;
            }
            if let Some(candidate) = session.candidate.as_ref() {
                let _ = app.emit_to(
                    MAIN_WINDOW_LABEL,
                    CANDIDATE_EVENT,
                    QqMusicLoginCandidateEvent {
                        attempt_id: session.attempt_id.clone(),
                        value: candidate.value.clone(),
                        can_refresh: candidate.can_refresh,
                    },
                );
            }
            return Ok(OpenQqMusicLoginResponse {
                attempt_id: session.attempt_id.clone(),
                reused: true,
            });
        }
    }

    let temporary_profile = tempfile::Builder::new()
        .prefix("yesplaymusic-qq-login-")
        .tempdir()
        .map_err(|_| "Unable to prepare the QQ Music login profile".to_owned())?;
    let attempt_id = new_attempt_id()?;
    let blank_url = "about:blank"
        .parse()
        .map_err(|_| "Unable to prepare the QQ Music login URL".to_owned())?;
    let app_for_new_window = app.clone();
    let navigation_window = WebviewWindowBuilder::new(
            &app,
            AUTH_WINDOW_LABEL,
            tauri::WebviewUrl::External(blank_url),
        )
        .parent(&owner)
        .map_err(|_| "Unable to create the QQ Music login window".to_owned())?
        .title("Connect QQ Music")
        .inner_size(1_040.0, 760.0)
        .min_inner_size(800.0, 600.0)
        .center()
        .visible(false)
        .data_directory(temporary_profile.path().to_path_buf())
        .incognito(true)
        .on_navigation(is_auth_navigation_url)
        .on_page_load(|webview, payload| {
            if payload.event() != tauri::webview::PageLoadEvent::Finished
                || !payload
                    .url()
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case("y.qq.com"))
            {
                return;
            }
            let _ = webview.eval(
                "setTimeout(()=>{if(location.hostname!=='y.qq.com'||document.querySelector('#login_frame'))return;const link=document.querySelector('a.top_login__link');if(link)link.click();},700)",
            );
        })
        .on_new_window(move |url, _features| {
            if is_official_qq_url(&url) {
                if let Some(target) = app_for_new_window.get_webview_window(AUTH_WINDOW_LABEL) {
                    let _ = target.navigate(url);
                }
            }
            tauri::webview::NewWindowResponse::Deny
        });

    let auth_window = navigation_window
        .build()
        .map_err(|_| "Unable to create the QQ Music login window".to_owned())?;

    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let session = LoginSession {
        attempt_id: attempt_id.clone(),
        window: auth_window.clone(),
        candidate: None,
        ready: false,
        _temporary_profile: temporary_profile,
        cancelled: cancelled.clone(),
        poller: None,
    };
    {
        let mut active = state
            .lock()
            .map_err(|_| "QQ Music login is unavailable".to_owned())?;
        if let Some(existing) = active.as_ref() {
            let _ = auth_window.close();
            if is_login_attempt_active(&existing.cancelled)
                && app.get_webview_window(AUTH_WINDOW_LABEL).is_some()
            {
                return Ok(OpenQqMusicLoginResponse {
                    attempt_id: existing.attempt_id.clone(),
                    reused: true,
                });
            }
            return Err("The previous QQ Music login window is still closing".to_owned());
        }
        *active = Some(session);
    }

    install_destroy_cleanup(&auth_window, app.clone(), state.clone(), attempt_id.clone());

    let (start_sender, start_receiver) = std::sync::mpsc::sync_channel(0);
    let poller_state = state.clone();
    let poller_app = app.clone();
    let poller_window = auth_window.clone();
    let poller_attempt = attempt_id.clone();
    let poller_cancelled = cancelled.clone();
    let poller = std::thread::spawn(move || {
        if start_receiver.recv().is_ok() {
            poll_qq_music_cookies(
                poller_app,
                poller_window,
                poller_state,
                poller_attempt,
                poller_cancelled,
            );
        }
    });
    if let Ok(mut active) = state.lock() {
        if let Some(session) = active
            .as_mut()
            .filter(|session| session.attempt_id == attempt_id)
        {
            session.poller = Some(poller);
        }
    }
    let _ = start_sender.send(());

    let (hook_sender, hook_receiver) = std::sync::mpsc::sync_channel(1);
    let window_for_hook = auth_window.clone();
    let cancelled_for_hook = cancelled.clone();
    let hook_submission = auth_window
        .with_webview(move |webview| {
            let result = if !is_login_attempt_active(&cancelled_for_hook) {
                Err("QQ Music login setup was cancelled".to_owned())
            } else if deny_webview_permissions(webview).is_ok() {
                if !is_login_attempt_active(&cancelled_for_hook) {
                    Err("QQ Music login setup was cancelled".to_owned())
                } else {
                    window_for_hook
                        .navigate(login_url)
                        .map_err(|_| "Unable to open the QQ Music login page".to_owned())
                }
            } else {
                Err("Unable to restrict QQ Music login permissions".to_owned())
            };
            let result = if is_login_attempt_active(&cancelled_for_hook) {
                result
            } else {
                Err("QQ Music login setup was cancelled".to_owned())
            };
            let _ = hook_sender.send(result);
        })
        .map_err(|_| "Unable to initialize QQ Music login security".to_owned());

    let setup_result = match hook_submission {
        Err(error) => Err(error),
        Ok(()) => match tauri::async_runtime::spawn_blocking(move || {
            hook_receiver.recv_timeout(std::time::Duration::from_secs(8))
        })
        .await
        {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(error),
            Ok(Err(_)) | Err(_) => Err("Unable to initialize QQ Music login security".to_owned()),
        },
    };
    finish_login_setup(setup_result, &cancelled, || {
        let _ = auth_window.close();
    })?;

    let shown = if is_login_attempt_active(&cancelled) {
        auth_window
            .show()
            .and_then(|()| auth_window.set_focus())
            .map_err(|_| "Unable to open the QQ Music login page".to_owned())
    } else {
        Err("QQ Music login setup was cancelled".to_owned())
    };
    finish_login_setup(shown, &cancelled, || {
        let _ = auth_window.close();
    })?;

    let ready = state
        .lock()
        .map_err(|_| "QQ Music login is unavailable".to_owned())
        .and_then(|mut active| {
            active
                .as_mut()
                .filter(|session| session.attempt_id == attempt_id)
                .map(|session| session.ready = true)
                .ok_or_else(|| "QQ Music login attempt is no longer active".to_owned())
        });
    finish_login_setup(ready, &cancelled, || {
        let _ = auth_window.close();
    })?;

    Ok(OpenQqMusicLoginResponse {
        attempt_id,
        reused: false,
    })
}

#[cfg(target_os = "windows")]
fn new_attempt_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| "Unable to start QQ Music login".to_owned())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(target_os = "windows")]
fn install_destroy_cleanup(
    window: &WebviewWindow,
    app: AppHandle,
    state: Arc<Mutex<Option<LoginSession>>>,
    attempt_id: String,
) {
    window.on_window_event(move |event| {
        if !matches!(event, tauri::WindowEvent::Destroyed) {
            return;
        }
        let session = state.lock().ok().and_then(|mut active| {
            if active
                .as_ref()
                .is_some_and(|session| session.attempt_id == attempt_id)
            {
                active.take()
            } else {
                None
            }
        });
        if let Some(mut session) = session {
            session
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            let poller = session.poller.take();
            std::thread::spawn(move || {
                if let Some(poller) = poller {
                    let _ = poller.join();
                }
                let LoginSession {
                    window,
                    _temporary_profile: temporary_profile,
                    ..
                } = session;
                let profile_path = temporary_profile.keep();
                drop(window);
                if remove_profile_with_retry(
                    || std::fs::remove_dir_all(&profile_path),
                    std::thread::sleep,
                )
                .is_err()
                {
                    eprintln!(
                        "QQ Music login temporary profile could not be removed after retries"
                    );
                }
            });
        }
        let _ = app.emit_to(
            MAIN_WINDOW_LABEL,
            CANDIDATE_EVENT,
            QqMusicLoginCandidateEvent {
                attempt_id: attempt_id.clone(),
                value: String::new(),
                can_refresh: false,
            },
        );
    });
}

#[cfg(any(target_os = "windows", test))]
fn remove_profile_with_retry(
    mut remove: impl FnMut() -> std::io::Result<()>,
    mut sleep: impl FnMut(std::time::Duration),
) -> std::io::Result<()> {
    for attempt in 0..PROFILE_CLEANUP_ATTEMPTS {
        match remove() {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) if attempt + 1 == PROFILE_CLEANUP_ATTEMPTS => return Err(error),
            Err(_) => sleep(PROFILE_CLEANUP_RETRY_INTERVAL),
        }
    }

    unreachable!("profile cleanup always attempts removal at least once");
}

#[cfg(target_os = "windows")]
fn poll_qq_music_cookies(
    app: AppHandle,
    window: WebviewWindow,
    state: Arc<Mutex<Option<LoginSession>>>,
    attempt_id: String,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) {
    let cookie_url = match COOKIE_URL.parse::<tauri::webview::Url>() {
        Ok(url) => url,
        Err(_) => return,
    };
    let mut previous: Option<(String, std::time::Instant)> = None;

    while !cancelled.load(std::sync::atomic::Ordering::Acquire) {
        let cookies = window
            .cookies_for_url(cookie_url.clone())
            .unwrap_or_default();
        let fields = cookies
            .iter()
            .map(|cookie| CookieField {
                name: cookie.name().to_owned(),
                value: cookie.value().to_owned(),
                domain: cookie.domain().map(str::to_owned),
            })
            .collect::<Vec<_>>();

        let candidate = normalize_qq_cookies(&fields);
        if let Some(candidate) = candidate {
            if candidate_is_ready(&mut previous, &candidate, std::time::Instant::now()) {
                let emit_candidate = {
                    let mut active = match state.lock() {
                        Ok(active) => active,
                        Err(_) => return,
                    };
                    let Some(session) = active
                        .as_mut()
                        .filter(|session| session.attempt_id == attempt_id)
                    else {
                        return;
                    };
                    if cancelled.load(std::sync::atomic::Ordering::Acquire)
                        || session.candidate.is_some()
                    {
                        false
                    } else {
                        session.candidate = Some(NormalizedCookie {
                            value: candidate.value.clone(),
                            can_refresh: candidate.can_refresh,
                        });
                        true
                    }
                };
                if emit_candidate {
                    if app
                        .emit_to(
                            MAIN_WINDOW_LABEL,
                            CANDIDATE_EVENT,
                            QqMusicLoginCandidateEvent {
                                attempt_id: attempt_id.clone(),
                                value: candidate.value,
                                can_refresh: candidate.can_refresh,
                            },
                        )
                        .is_ok()
                    {
                        return;
                    }
                    if let Ok(mut active) = state.lock() {
                        if let Some(session) = active
                            .as_mut()
                            .filter(|session| session.attempt_id == attempt_id)
                        {
                            session.candidate = None;
                        }
                    }
                }
            }
        } else {
            previous = None;
        }

        std::thread::sleep(COOKIE_POLL_INTERVAL);
    }
}

#[cfg(any(target_os = "windows", test))]
fn candidate_is_ready(
    previous: &mut Option<(String, std::time::Instant)>,
    candidate: &NormalizedCookie,
    now: std::time::Instant,
) -> bool {
    if candidate.can_refresh {
        return true;
    }
    match previous.as_ref() {
        Some((value, since)) if value == &candidate.value => {
            now.duration_since(*since) >= COOKIE_STABILITY_INTERVAL
        }
        _ => {
            *previous = Some((candidate.value.clone(), now));
            false
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn finish_login_setup<T>(
    result: Result<T, String>,
    cancelled: &std::sync::atomic::AtomicBool,
    close_window: impl FnOnce(),
) -> Result<T, String> {
    if result.is_err() {
        cancelled.store(true, std::sync::atomic::Ordering::Release);
        close_window();
    }
    result
}

#[cfg(any(target_os = "windows", test))]
fn is_login_attempt_active(cancelled: &std::sync::atomic::AtomicBool) -> bool {
    !cancelled.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(target_os = "windows")]
fn deny_webview_permissions(webview: tauri::webview::PlatformWebview) -> Result<(), ()> {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{ICoreWebView2_12, COREWEBVIEW2_PERMISSION_STATE_DENY},
        PermissionRequestedEventHandler,
    };
    use windows_core::Interface;

    let controller = webview.controller();
    let core = unsafe { controller.CoreWebView2() }.map_err(|_| ())?;
    let core_with_permissions = core.cast::<ICoreWebView2_12>().map_err(|_| ())?;
    let handler = PermissionRequestedEventHandler::create(Box::new(|_sender, args| {
        if let Some(args) = args {
            unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)? };
        }
        Ok(())
    }));
    let mut token = 0i64;
    unsafe { core_with_permissions.add_PermissionRequested(&handler, &mut token) }.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_is_ready, finish_login_setup, is_allowed_qq_cookie_domain,
        is_auth_navigation_url, is_login_attempt_active, is_main_window_label, is_official_qq_url,
        normalize_qq_cookies, remove_profile_with_retry, CookieField, NormalizedCookie,
        COOKIE_STABILITY_INTERVAL, PROFILE_CLEANUP_ATTEMPTS, PROFILE_CLEANUP_RETRY_INTERVAL,
    };

    fn field(name: &str, value: &str, domain: &str) -> CookieField {
        CookieField {
            name: name.to_owned(),
            value: value.to_owned(),
            domain: Some(domain.to_owned()),
        }
    }

    #[test]
    fn cookie_domain_allowlist_is_exact() {
        assert!(is_allowed_qq_cookie_domain(Some(".qq.com")));
        assert!(is_allowed_qq_cookie_domain(Some("y.qq.com")));
        assert!(!is_allowed_qq_cookie_domain(Some("login.qq.com")));
        assert!(!is_allowed_qq_cookie_domain(Some("qq.com.evil.test")));
    }

    #[test]
    fn navigation_requires_https_and_a_qq_domain() {
        assert!(is_official_qq_url(
            &"https://y.qq.com/n/ryqq/profile".parse().unwrap()
        ));
        assert!(is_official_qq_url(
            &"https://login.qq.com/".parse().unwrap()
        ));
        assert!(!is_official_qq_url(&"http://y.qq.com/".parse().unwrap()));
        assert!(!is_official_qq_url(
            &"https://qq.com.evil.test/".parse().unwrap()
        ));
        assert!(is_auth_navigation_url(&"about:blank".parse().unwrap()));
        assert!(!is_auth_navigation_url(
            &"https://example.com/".parse().unwrap()
        ));
    }

    #[test]
    fn candidate_cookie_names_and_domain_filters_are_applied_before_export() {
        let result = normalize_qq_cookies(&[
            field("uin", "123456789", ".qq.com"),
            field("qm_keyst", "fake-key", ".y.qq.com"),
            field("psrf_qqrefresh_token", "fake-refresh-token", ".qq.com"),
            field("sessionid", "fake-session", ".qq.com"),
            field("qqmusic_key", "discarded-key", ".evil.qq.com"),
        ])
        .expect("valid test candidate");

        assert_eq!(
            result.value,
            "uin=123456789; qqmusic_key=fake-key; qm_keyst=fake-key; psrf_qqrefresh_token=fake-refresh-token"
        );
        assert!(result.can_refresh);
    }

    #[test]
    fn incomplete_or_malformed_cookies_never_become_candidates() {
        assert!(normalize_qq_cookies(&[field("uin", "123", ".qq.com")]).is_none());
        assert!(normalize_qq_cookies(&[
            field("uin", "o123", ".qq.com"),
            field("qm_keyst", "fake-key", ".qq.com"),
        ])
        .is_none());
        assert!(normalize_qq_cookies(&[
            field("uin", "123", ".qq.com"),
            field("qm_keyst", "fake key", ".qq.com"),
        ])
        .is_none());
    }

    #[test]
    fn only_the_main_window_can_call_the_login_commands() {
        assert!(is_main_window_label("main"));
        assert!(!is_main_window_label("qq-music-login-auth"));
        assert!(!is_main_window_label("lastfm-auth"));
    }

    #[test]
    fn non_refresh_cookie_candidates_must_remain_stable() {
        let started = std::time::Instant::now();
        let candidate = NormalizedCookie {
            value: "uin=123456789; qm_keyst=fake-key".to_owned(),
            can_refresh: false,
        };
        let mut previous = None;

        assert!(!candidate_is_ready(&mut previous, &candidate, started));
        assert!(!candidate_is_ready(
            &mut previous,
            &candidate,
            started + COOKIE_STABILITY_INTERVAL - std::time::Duration::from_millis(1)
        ));
        assert!(candidate_is_ready(
            &mut previous,
            &candidate,
            started + COOKIE_STABILITY_INTERVAL
        ));
    }

    #[test]
    fn changed_candidates_restart_stability_and_refresh_tokens_bypass_waiting() {
        let started = std::time::Instant::now();
        let mut previous = None;
        let first = NormalizedCookie {
            value: "uin=123456789; qm_keyst=first-key".to_owned(),
            can_refresh: false,
        };
        let changed = NormalizedCookie {
            value: "uin=123456789; qm_keyst=second-key".to_owned(),
            can_refresh: false,
        };

        assert!(!candidate_is_ready(&mut previous, &first, started));
        assert!(!candidate_is_ready(
            &mut previous,
            &changed,
            started + COOKIE_STABILITY_INTERVAL
        ));
        assert!(candidate_is_ready(
            &mut previous,
            &NormalizedCookie {
                value: changed.value,
                can_refresh: true,
            },
            started + COOKIE_STABILITY_INTERVAL
        ));
    }

    #[test]
    fn failed_login_window_setup_cancels_the_attempt_and_closes_the_window() {
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let mut closed = false;

        let result = finish_login_setup::<()>(
            Err("QQ Music login setup timed out".to_owned()),
            &cancelled,
            || closed = true,
        );

        assert_eq!(result, Err("QQ Music login setup timed out".to_owned()));
        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
        assert!(!is_login_attempt_active(&cancelled));
        assert!(closed);
    }

    #[test]
    fn temporary_profile_cleanup_retries_transient_lock_failures() {
        let mut attempts = 0;
        let mut waits = 0;
        let result = remove_profile_with_retry(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                } else {
                    Ok(())
                }
            },
            |_| waits += 1,
        );

        assert!(result.is_ok());
        assert_eq!(attempts, 3);
        assert_eq!(waits, 2);
    }

    #[test]
    fn temporary_profile_cleanup_treats_missing_directory_as_success() {
        let mut attempts = 0;
        let result = remove_profile_with_retry(
            || {
                attempts += 1;
                Err(std::io::Error::from(std::io::ErrorKind::NotFound))
            },
            |_| panic!("missing profile should not be retried"),
        );

        assert!(result.is_ok());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn temporary_profile_cleanup_bounds_permanent_failures() {
        let mut attempts = 0;
        let mut waits = 0;
        let result = remove_profile_with_retry(
            || {
                attempts += 1;
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            },
            |_| waits += 1,
        );

        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(attempts, PROFILE_CLEANUP_ATTEMPTS);
        assert_eq!(waits, PROFILE_CLEANUP_ATTEMPTS - 1);
        assert!(PROFILE_CLEANUP_RETRY_INTERVAL > std::time::Duration::ZERO);
    }
}
