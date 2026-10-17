use std::{
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Manager, Theme, Url};

const PROXY_FILE_NAME: &str = "webview-proxy.json";
const PROXY_FILE_VERSION: u8 = 1;
const MAX_LEGACY_CONFIG_BYTES: usize = 1_048_576;
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct PreferenceError(String);

impl PreferenceError {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for PreferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PreferenceError {}

impl From<io::Error> for PreferenceError {
    fn from(error: io::Error) -> Self {
        Self(format!("desktop preference I/O failed: {error}"))
    }
}

impl From<serde_json::Error> for PreferenceError {
    fn from(error: serde_json::Error) -> Self {
        Self(format!("desktop preference JSON is invalid: {error}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAppOption {
    Ask,
    Exit,
    MinimizeToTray,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseDecision {
    Hide,
    Ask,
    Exit,
    MinimizeToTray,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayIconTheme {
    Auto,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopPreferences {
    pub close_app_option: CloseAppOption,
    pub tray_icon_theme: TrayIconTheme,
    pub linux_enable_custom_titlebar: bool,
    pub windows_taskbar_docking: bool,
    pub windows_window_animations: bool,
    /// The OS asks for reduced motion; the renderer reads it and sends it along.
    pub system_reduced_motion: bool,
}

impl Default for DesktopPreferences {
    fn default() -> Self {
        Self {
            close_app_option: CloseAppOption::Ask,
            tray_icon_theme: TrayIconTheme::Auto,
            linux_enable_custom_titlebar: false,
            windows_taskbar_docking: true,
            windows_window_animations: true,
            system_reduced_motion: false,
        }
    }
}

pub fn parse_desktop_preferences(value: &Value) -> Result<DesktopPreferences, PreferenceError> {
    let settings = value
        .as_object()
        .ok_or_else(|| PreferenceError::invalid("settings must be an object"))?;
    let close_app_option = match settings.get("closeAppOption").and_then(Value::as_str) {
        Some("ask") => CloseAppOption::Ask,
        Some("exit") => CloseAppOption::Exit,
        Some("minimizeToTray") => CloseAppOption::MinimizeToTray,
        _ => {
            return Err(PreferenceError::invalid(
                "closeAppOption must be ask, exit, or minimizeToTray",
            ));
        }
    };
    let tray_icon_theme = match settings.get("trayIconTheme").and_then(Value::as_str) {
        Some("auto") => TrayIconTheme::Auto,
        Some("light") => TrayIconTheme::Light,
        Some("dark") => TrayIconTheme::Dark,
        _ => {
            return Err(PreferenceError::invalid(
                "trayIconTheme must be auto, light, or dark",
            ));
        }
    };
    let boolean = |key: &str| {
        settings
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| PreferenceError::invalid(format!("{key} must be a boolean")))
    };

    Ok(DesktopPreferences {
        close_app_option,
        tray_icon_theme,
        linux_enable_custom_titlebar: boolean("linuxEnableCustomTitlebar")?,
        windows_taskbar_docking: boolean("windowsTaskbarDocking")?,
        windows_window_animations: boolean("windowsWindowAnimations")?,
        system_reduced_motion: boolean("systemReducedMotion")?,
    })
}

pub fn close_decision(is_macos: bool, option: CloseAppOption) -> CloseDecision {
    if is_macos {
        return CloseDecision::Hide;
    }

    match option {
        CloseAppOption::Ask => CloseDecision::Ask,
        CloseAppOption::Exit => CloseDecision::Exit,
        CloseAppOption::MinimizeToTray => CloseDecision::MinimizeToTray,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CloseChoiceAction {
    Exit,
    MinimizeToTray,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CloseChoice {
    pub action: CloseChoiceAction,
    pub remember: bool,
}

pub fn parse_close_choice(value: Value) -> Result<CloseChoice, PreferenceError> {
    serde_json::from_value(value)
        .map_err(|error| PreferenceError::invalid(format!("invalid close choice: {error}")))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayIconAsset {
    Light,
    Dark,
}

impl TrayIconAsset {
    pub const fn relative_path(self) -> &'static str {
        match self {
            Self::Light => "img/icons/menu-light@88.png",
            Self::Dark => "img/icons/menu-dark@88.png",
        }
    }
}

pub fn tray_icon_asset(setting: TrayIconTheme, system_theme: Theme) -> TrayIconAsset {
    match setting {
        TrayIconTheme::Light => TrayIconAsset::Light,
        TrayIconTheme::Dark => TrayIconAsset::Dark,
        TrayIconTheme::Auto => match system_theme {
            Theme::Dark => TrayIconAsset::Light,
            Theme::Light => TrayIconAsset::Dark,
            _ => TrayIconAsset::Dark,
        },
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProxyPayload {
    protocol: ProxyProtocol,
    server: String,
    port: u16,
}

#[derive(Debug, Deserialize)]
enum ProxyProtocol {
    #[serde(rename = "HTTP")]
    Http,
    #[serde(rename = "HTTPS")]
    Https,
}

pub fn parse_set_proxy_payload(value: Value) -> Result<Url, PreferenceError> {
    let payload: ProxyPayload = serde_json::from_value(value)
        .map_err(|error| PreferenceError::invalid(format!("invalid setProxy payload: {error}")))?;
    if payload.port == 0 {
        return Err(PreferenceError::invalid(
            "proxy port must be between 1 and 65535",
        ));
    }
    if payload.server.is_empty() || payload.server.trim() != payload.server {
        return Err(PreferenceError::invalid(
            "proxy server must be a non-empty host",
        ));
    }

    let protocol = match payload.protocol {
        ProxyProtocol::Http => "http",
        ProxyProtocol::Https => "https",
    };
    let server = if payload.server.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{}]", payload.server)
    } else {
        payload.server
    };
    let url = Url::parse(&format!("{protocol}://{}:{}", server, payload.port))
        .map_err(|_| PreferenceError::invalid("proxy server or port is invalid"))?;
    validate_proxy_url(&url)?;
    Ok(url)
}

pub fn parse_remove_proxy_payload(value: &Value) -> Result<(), PreferenceError> {
    if value.is_null() {
        Ok(())
    } else {
        Err(PreferenceError::invalid("removeProxy payload must be null"))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredProxy {
    version: u8,
    url: Value,
}

pub fn save_webview_proxy<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    payload: Value,
) -> Result<Url, PreferenceError> {
    let url = parse_set_proxy_payload(payload)?;
    let config_dir = app.path().app_config_dir().map_err(|error| {
        PreferenceError::invalid(format!("app config path is unavailable: {error}"))
    })?;
    save_proxy_to_dir(&config_dir, &url)?;
    Ok(url)
}

pub fn remove_webview_proxy<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    payload: &Value,
) -> Result<(), PreferenceError> {
    parse_remove_proxy_payload(payload)?;
    let config_dir = app.path().app_config_dir().map_err(|error| {
        PreferenceError::invalid(format!("app config path is unavailable: {error}"))
    })?;
    remove_proxy_from_dir(&config_dir)
}

pub fn load_webview_proxy<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Option<Url>, PreferenceError> {
    let config_dir = app.path().app_config_dir().map_err(|error| {
        PreferenceError::invalid(format!("app config path is unavailable: {error}"))
    })?;
    load_proxy_from_dir(&config_dir)
}

pub fn migrate_legacy_webview_proxy<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    legacy_config: &[u8],
) -> Result<bool, PreferenceError> {
    let config_dir = app.path().app_config_dir().map_err(|error| {
        PreferenceError::invalid(format!("app config path is unavailable: {error}"))
    })?;
    migrate_legacy_proxy_from_bytes(&config_dir, legacy_config)
}

fn proxy_file(config_dir: &Path) -> PathBuf {
    config_dir.join(PROXY_FILE_NAME)
}

fn migrate_legacy_proxy_from_bytes(
    config_dir: &Path,
    legacy_config: &[u8],
) -> Result<bool, PreferenceError> {
    match fs::symlink_metadata(proxy_file(config_dir)) {
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if legacy_config.len() > MAX_LEGACY_CONFIG_BYTES {
        return Err(PreferenceError::invalid("legacy config is too large"));
    }
    let root: Value = serde_json::from_slice(legacy_config)?;
    let root = root
        .as_object()
        .ok_or_else(|| PreferenceError::invalid("legacy config root must be an object"))?;
    let Some(value) = root.get("proxy") else {
        return Ok(false);
    };
    if value.is_null() || value.as_str() == Some("") {
        return Ok(false);
    }
    let raw = value
        .as_str()
        .ok_or_else(|| PreferenceError::invalid("legacy proxy must be a string"))?;
    if raw.trim() != raw {
        return Err(PreferenceError::invalid(
            "legacy proxy must not contain surrounding whitespace",
        ));
    }
    let url =
        Url::parse(raw).map_err(|_| PreferenceError::invalid("legacy proxy URL is invalid"))?;
    validate_proxy_url(&url)?;
    if url.port().is_none() {
        return Err(PreferenceError::invalid(
            "legacy proxy URL must contain an explicit port",
        ));
    }
    save_proxy_to_dir(config_dir, &url)?;
    Ok(true)
}

fn save_proxy_to_dir(config_dir: &Path, url: &Url) -> Result<(), PreferenceError> {
    validate_proxy_url(url)?;
    save_proxy_state_to_dir(config_dir, Value::String(url.as_str().to_string()))
}

fn save_proxy_state_to_dir(config_dir: &Path, url: Value) -> Result<(), PreferenceError> {
    fs::create_dir_all(config_dir)?;
    let stored = StoredProxy {
        version: PROXY_FILE_VERSION,
        url,
    };
    let bytes = serde_json::to_vec(&stored)?;
    let (temporary_path, mut temporary) = create_temporary_file(config_dir)?;

    let write_result = (|| -> Result<(), PreferenceError> {
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        fs::rename(&temporary_path, proxy_file(config_dir))?;
        sync_directory(config_dir)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

fn create_temporary_file(config_dir: &Path) -> Result<(PathBuf, fs::File), PreferenceError> {
    for _ in 0..32 {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = config_dir.join(format!(
            ".{PROXY_FILE_NAME}.{}.{sequence}.tmp",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(PreferenceError::invalid(
        "could not reserve a proxy preference temporary file",
    ))
}

fn load_proxy_from_dir(config_dir: &Path) -> Result<Option<Url>, PreferenceError> {
    let bytes = match fs::read(proxy_file(config_dir)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if bytes.len() > 4096 {
        return Err(PreferenceError::invalid("stored proxy file is too large"));
    }

    let stored: StoredProxy = serde_json::from_slice(&bytes)?;
    if stored.version != PROXY_FILE_VERSION {
        return Err(PreferenceError::invalid(
            "stored proxy version is unsupported",
        ));
    }
    let Some(raw_url) = stored.url.as_str() else {
        if stored.url.is_null() {
            return Ok(None);
        }
        return Err(PreferenceError::invalid(
            "stored proxy URL must be a string or null",
        ));
    };
    let url =
        Url::parse(raw_url).map_err(|_| PreferenceError::invalid("stored proxy URL is invalid"))?;
    validate_proxy_url(&url)?;
    Ok(Some(url))
}

fn remove_proxy_from_dir(config_dir: &Path) -> Result<(), PreferenceError> {
    save_proxy_state_to_dir(config_dir, Value::Null)
}

fn validate_proxy_url(url: &Url) -> Result<(), PreferenceError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(PreferenceError::invalid(
            "proxy URL must contain only an HTTP(S) host and port",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), PreferenceError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), PreferenceError> {
    Ok(())
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlayerStatePayload {
    pub playing: bool,
    pub liked_current_track: bool,
}

#[cfg(test)]
pub fn parse_player_state_payload(value: Value) -> Result<PlayerStatePayload, PreferenceError> {
    serde_json::from_value(value)
        .map_err(|error| PreferenceError::invalid(format!("invalid player payload: {error}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MediaStateEnvelope {
    playing: bool,
    liked_current_track: bool,
    position_seconds: f64,
    repeat_mode: MediaRepeatMode,
    shuffle: bool,
}

#[derive(Debug, Deserialize)]
enum MediaRepeatMode {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "on")]
    Playlist,
    #[serde(rename = "one")]
    Track,
}

pub fn parse_player_state_from_media_state(
    value: &Value,
) -> Result<PlayerStatePayload, PreferenceError> {
    let state: MediaStateEnvelope = serde_json::from_value(value.clone()).map_err(|error| {
        PreferenceError::invalid(format!("invalid mediaState payload: {error}"))
    })?;
    if !state.position_seconds.is_finite() || state.position_seconds < 0.0 {
        return Err(PreferenceError::invalid(
            "mediaState positionSeconds must be finite and non-negative",
        ));
    }
    let _media_state = (state.position_seconds, state.repeat_mode, state.shuffle);
    Ok(PlayerStatePayload {
        playing: state.playing,
        liked_current_track: state.liked_current_track,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayMenuText {
    pub playback: &'static str,
    pub like: &'static str,
}

pub fn tray_menu_text(state: PlayerStatePayload) -> TrayMenuText {
    TrayMenuText {
        playback: if state.playing { "暂停" } else { "播放" },
        like: if state.liked_current_track {
            "取消喜欢"
        } else {
            "加入喜欢"
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "yesplaymusic-desktop-preferences-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_relevant_settings_without_trusting_their_types() {
        let settings = json!({
            "closeAppOption": "minimizeToTray",
            "trayIconTheme": "auto",
            "linuxEnableCustomTitlebar": true,
            "windowsTaskbarDocking": false,
            "windowsWindowAnimations": true,
            "systemReducedMotion": true,
            "unrelated": true
        });
        assert_eq!(
            parse_desktop_preferences(&settings).unwrap(),
            DesktopPreferences {
                close_app_option: CloseAppOption::MinimizeToTray,
                tray_icon_theme: TrayIconTheme::Auto,
                linux_enable_custom_titlebar: true,
                windows_taskbar_docking: false,
                windows_window_animations: true,
                system_reduced_motion: true,
            }
        );
        let with = |key: &str, value: serde_json::Value| {
            let mut settings = settings.clone();
            settings[key] = value;
            settings
        };
        assert!(parse_desktop_preferences(&with("closeAppOption", json!(false))).is_err());
        assert!(parse_desktop_preferences(&with("trayIconTheme", json!("system"))).is_err());
        assert!(
            parse_desktop_preferences(&with("linuxEnableCustomTitlebar", json!("yes"))).is_err()
        );
        assert!(parse_desktop_preferences(&with("windowsTaskbarDocking", json!(1))).is_err());
        assert!(parse_desktop_preferences(&with("windowsWindowAnimations", json!(null))).is_err());
        assert!(parse_desktop_preferences(&with("systemReducedMotion", json!("reduce"))).is_err());
        for key in ["trayIconTheme", "systemReducedMotion"] {
            let mut missing = settings.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(parse_desktop_preferences(&missing).is_err(), "{key}");
        }
    }

    #[test]
    fn macos_always_hides_and_other_platforms_follow_the_setting() {
        for option in [
            CloseAppOption::Ask,
            CloseAppOption::Exit,
            CloseAppOption::MinimizeToTray,
        ] {
            assert_eq!(close_decision(true, option), CloseDecision::Hide);
        }
        assert_eq!(
            close_decision(false, CloseAppOption::Ask),
            CloseDecision::Ask
        );
        assert_eq!(
            close_decision(false, CloseAppOption::Exit),
            CloseDecision::Exit
        );
        assert_eq!(
            close_decision(false, CloseAppOption::MinimizeToTray),
            CloseDecision::MinimizeToTray
        );
    }

    #[test]
    fn close_choice_requires_a_known_action_and_boolean_remember_flag() {
        assert_eq!(
            parse_close_choice(json!({
                "action": "minimizeToTray",
                "remember": true
            }))
            .unwrap(),
            CloseChoice {
                action: CloseChoiceAction::MinimizeToTray,
                remember: true,
            }
        );
        assert!(parse_close_choice(json!({
            "action": "cancel",
            "remember": false
        }))
        .is_err());
        assert!(parse_close_choice(json!({
            "action": "exit",
            "remember": "yes"
        }))
        .is_err());
        assert!(parse_close_choice(json!({
            "action": "exit",
            "remember": false,
            "extra": true
        }))
        .is_err());
    }

    #[test]
    fn auto_tray_icons_contrast_with_the_system_theme() {
        assert_eq!(
            tray_icon_asset(TrayIconTheme::Auto, Theme::Dark),
            TrayIconAsset::Light
        );
        assert_eq!(
            tray_icon_asset(TrayIconTheme::Auto, Theme::Light),
            TrayIconAsset::Dark
        );
        assert_eq!(
            tray_icon_asset(TrayIconTheme::Light, Theme::Light).relative_path(),
            "img/icons/menu-light@88.png"
        );
        assert_eq!(
            tray_icon_asset(TrayIconTheme::Dark, Theme::Dark).relative_path(),
            "img/icons/menu-dark@88.png"
        );
    }

    #[test]
    fn accepts_only_complete_http_proxy_payloads() {
        let http = parse_set_proxy_payload(json!({
            "protocol": "HTTP",
            "server": "proxy.example.com",
            "port": 8080
        }))
        .unwrap();
        let https = parse_set_proxy_payload(json!({
            "protocol": "HTTPS",
            "server": "127.0.0.1",
            "port": 3128
        }))
        .unwrap();
        assert_eq!(http.as_str(), "http://proxy.example.com:8080/");
        assert_eq!(https.as_str(), "https://127.0.0.1:3128/");
        assert_eq!(
            parse_set_proxy_payload(json!({
                "protocol": "HTTP",
                "server": "::1",
                "port": 8080
            }))
            .unwrap()
            .as_str(),
            "http://[::1]:8080/"
        );

        for invalid in [
            json!({"protocol": "http", "server": "localhost", "port": 1080}),
            json!({"protocol": "SOCKS5", "server": "localhost", "port": 1080}),
            json!({"protocol": "HTTP", "server": "", "port": 8080}),
            json!({"protocol": "HTTP", "server": " proxy.local", "port": 8080}),
            json!({"protocol": "HTTP", "server": "proxy.local/path", "port": 8080}),
            json!({"protocol": "HTTP", "server": "proxy.local", "port": 0}),
            json!({"protocol": "HTTP", "server": "proxy.local", "port": 65536}),
            json!({"protocol": "HTTP", "server": "proxy.local", "port": "8080"}),
            json!({"protocol": "HTTP", "server": "proxy.local", "port": 1.5}),
            json!({
                "protocol": "HTTP",
                "server": "proxy.local",
                "port": 8080,
                "bypass": "localhost"
            }),
        ] {
            assert!(parse_set_proxy_payload(invalid).is_err());
        }
    }

    #[test]
    fn remove_proxy_accepts_only_an_empty_payload() {
        assert!(parse_remove_proxy_payload(&Value::Null).is_ok());
        assert!(parse_remove_proxy_payload(&json!({})).is_err());
        assert!(parse_remove_proxy_payload(&json!(false)).is_err());
    }

    #[test]
    fn proxy_storage_round_trips_and_replaces_atomically() {
        let directory = TestDirectory::new();
        let first = Url::parse("http://localhost:8080").unwrap();
        let second = Url::parse("http://proxy.example.com:3128").unwrap();

        assert_eq!(load_proxy_from_dir(&directory.0).unwrap(), None);
        save_proxy_to_dir(&directory.0, &first).unwrap();
        assert_eq!(load_proxy_from_dir(&directory.0).unwrap(), Some(first));
        save_proxy_to_dir(&directory.0, &second).unwrap();
        assert_eq!(load_proxy_from_dir(&directory.0).unwrap(), Some(second));
        assert_eq!(
            fs::read_dir(&directory.0).unwrap().count(),
            1,
            "temporary files must not remain"
        );

        remove_proxy_from_dir(&directory.0).unwrap();
        remove_proxy_from_dir(&directory.0).unwrap();
        assert_eq!(load_proxy_from_dir(&directory.0).unwrap(), None);
    }

    #[test]
    fn legacy_root_proxy_migrates_once_without_overwriting_native_state() {
        let directory = TestDirectory::new();
        let legacy = br#"{"proxy":"HTTP://proxy.example.com:8080","settings":{}}"#;

        assert!(migrate_legacy_proxy_from_bytes(&directory.0, legacy).unwrap());
        assert_eq!(
            load_proxy_from_dir(&directory.0).unwrap().unwrap().as_str(),
            "http://proxy.example.com:8080/"
        );
        assert!(!migrate_legacy_proxy_from_bytes(
            &directory.0,
            br#"{"proxy":"HTTPS://other.example.com:8443"}"#
        )
        .unwrap());
        assert_eq!(
            load_proxy_from_dir(&directory.0).unwrap().unwrap().as_str(),
            "http://proxy.example.com:8080/"
        );

        remove_proxy_from_dir(&directory.0).unwrap();
        assert!(!migrate_legacy_proxy_from_bytes(&directory.0, legacy).unwrap());
        assert_eq!(load_proxy_from_dir(&directory.0).unwrap(), None);
    }

    #[test]
    fn legacy_root_proxy_rejects_unsafe_urls_without_writing_state() {
        let directory = TestDirectory::new();

        for legacy in [
            br#"{"proxy":"socks5://proxy.example.com:1080"}"#.as_slice(),
            br#"{"proxy":"http://user:pass@proxy.example.com:8080"}"#.as_slice(),
            br#"{"proxy":"http://proxy.example.com:8080/path"}"#.as_slice(),
            br#"{"proxy":8080}"#.as_slice(),
        ] {
            assert!(migrate_legacy_proxy_from_bytes(&directory.0, legacy).is_err());
            assert_eq!(load_proxy_from_dir(&directory.0).unwrap(), None);
        }
    }

    #[test]
    fn rejects_tampered_proxy_storage() {
        let directory = TestDirectory::new();
        let path = proxy_file(&directory.0);

        fs::write(&path, br#"{"version":1,"url":"file:///tmp/socket"}"#).unwrap();
        assert!(load_proxy_from_dir(&directory.0).is_err());
        fs::write(
            &path,
            br#"{"version":1,"url":"http://user:pass@proxy.local:8080/"}"#,
        )
        .unwrap();
        assert!(load_proxy_from_dir(&directory.0).is_err());
        fs::write(
            &path,
            br#"{"version":1,"url":"http://proxy.local:8080/","extra":true}"#,
        )
        .unwrap();
        assert!(load_proxy_from_dir(&directory.0).is_err());
        fs::write(&path, br#"{"version":2,"url":"http://proxy.local:8080/"}"#).unwrap();
        assert!(load_proxy_from_dir(&directory.0).is_err());
        fs::write(&path, vec![b'x'; 4097]).unwrap();
        assert!(load_proxy_from_dir(&directory.0).is_err());
    }

    #[test]
    fn player_payload_and_menu_text_are_exact() {
        let playing = parse_player_state_payload(json!({
            "playing": true,
            "likedCurrentTrack": true
        }))
        .unwrap();
        assert_eq!(
            tray_menu_text(playing),
            TrayMenuText {
                playback: "暂停",
                like: "取消喜欢",
            }
        );

        let paused = parse_player_state_payload(json!({
            "playing": false,
            "likedCurrentTrack": false
        }))
        .unwrap();
        assert_eq!(
            tray_menu_text(paused),
            TrayMenuText {
                playback: "播放",
                like: "加入喜欢",
            }
        );
        assert!(parse_player_state_payload(json!({
            "playing": false,
            "likedCurrentTrack": false,
            "position": 1
        }))
        .is_err());
        assert!(parse_player_state_payload(json!({
            "playing": "false",
            "likedCurrentTrack": false
        }))
        .is_err());
        assert!(parse_player_state_payload(json!({
            "playing": false
        }))
        .is_err());
    }

    #[test]
    fn extracts_tray_state_from_the_strict_shared_media_payload() {
        let value = json!({
            "playing": true,
            "likedCurrentTrack": false,
            "positionSeconds": 12.5,
            "repeatMode": "one",
            "shuffle": true
        });
        assert_eq!(
            parse_player_state_from_media_state(&value).unwrap(),
            PlayerStatePayload {
                playing: true,
                liked_current_track: false,
            }
        );
        assert!(parse_player_state_from_media_state(&json!({
            "playing": true,
            "likedCurrentTrack": false,
            "positionSeconds": -1,
            "repeatMode": "one",
            "shuffle": true
        }))
        .is_err());
        assert!(parse_player_state_from_media_state(&json!({
            "playing": true,
            "likedCurrentTrack": false,
            "positionSeconds": 1,
            "repeatMode": "all",
            "shuffle": true
        }))
        .is_err());
        assert!(parse_player_state_from_media_state(&json!({
            "playing": true,
            "likedCurrentTrack": false,
            "positionSeconds": 1,
            "repeatMode": "off",
            "shuffle": true,
            "unexpected": true
        }))
        .is_err());
    }
}
