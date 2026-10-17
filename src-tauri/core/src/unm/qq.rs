//! QQ Music provider, replacing `unm_engine_qq` 0.4.0. That engine sends
//! `guid` as a number (QQ answers with an empty `purl`), asks for the song
//! mid instead of the media file, and joins an empty `purl` onto the server
//! root. A live check without a login cookie got code 104009 with an empty
//! `sip` and empty `purl`s even for the guest file, so in practice this engine
//! only yields audio when `qq:cookie` holds a logged-in session.

use std::{
    borrow::Cow, collections::hash_map::RandomState, hash::BuildHasher, sync::Mutex, time::Duration,
};

use anyhow::{anyhow, Context as _};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, COOKIE, USER_AGENT};
use serde_json::{json, Value};
use unm_engine::interface::Engine;
use unm_types::{Context, RetrievedSongInfo, SerializedIdentifier, Song, SongSearchInformation};

pub const ENGINE_ID: &str = "qq";
const API_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const COOKIE_CONFIG_KEY: &str = "qq:cookie";
const IDENTIFIER_SEPARATOR: &str = ":::";
/// Same window as `unm_selector::SimilarSongSelector`.
const DURATION_TOLERANCE_MS: i64 = 5_000;
/// What QQ answers for every file when the request leaves from outside
/// mainland China, free songs included.
const RESULT_REGION_LOCKED: i64 = 104_003;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
// Header values the bundled `unm_request` client sends.
const USER_AGENT_VALUE: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/66.0.3359.181 Safari/537.36";
const REFERER_VALUE: &str = "https://y.qq.com/";

pub struct QqEngine {
    endpoint: String,
    direct: reqwest::Client,
    /// Last proxied client, so a fixed proxy setting reuses one connection pool.
    proxied: Mutex<Option<(String, reqwest::Client)>>,
}

impl QqEngine {
    pub fn new() -> Self {
        Self {
            endpoint: API_URL.to_owned(),
            direct: client_builder()
                .build()
                .expect("static QQ HTTP client configuration is valid"),
            proxied: Mutex::new(None),
        }
    }

    fn client(&self, context: &Context) -> anyhow::Result<reqwest::Client> {
        let Some(proxy) = context.proxy_uri.as_deref().filter(|uri| !uri.is_empty()) else {
            return Ok(self.direct.clone());
        };
        let mut cached = self
            .proxied
            .lock()
            .map_err(|_| anyhow!("QQ client cache is poisoned"))?;
        if let Some((uri, client)) = cached.as_ref() {
            if uri == proxy {
                return Ok(client.clone());
            }
        }
        let client = client_builder()
            .proxy(reqwest::Proxy::all(proxy).context("QQ proxy is invalid")?)
            .build()
            .context("QQ HTTP client could not be built")?;
        *cached = Some((proxy.to_owned(), client.clone()));
        Ok(client)
    }

    async fn call(&self, body: &Value, context: &Context) -> anyhow::Result<Value> {
        let mut request = self
            .client(context)?
            .get(&self.endpoint)
            .query(&[("data", body.to_string())]);
        if let Some(cookie) = configured_cookie(context) {
            request = request.header(COOKIE, cookie);
        }
        request
            .send()
            .await
            .context("QQ request failed")?
            .error_for_status()
            .context("QQ returned an HTTP error")?
            .json()
            .await
            .context("QQ returned invalid JSON")
    }
}

impl Default for QqEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Engine for QqEngine {
    async fn search<'a>(
        &self,
        song: &'a Song,
        context: &'a Context,
    ) -> anyhow::Result<Option<SongSearchInformation>> {
        let response = self.call(&search_request(&song.keyword()), context).await?;
        Ok(select_match(&response, song.duration).map(|identifier| {
            SongSearchInformation::builder()
                .source(Cow::Borrowed(ENGINE_ID))
                .identifier(identifier)
                .build()
        }))
    }

    async fn retrieve<'a>(
        &self,
        identifier: &'a SerializedIdentifier,
        context: &'a Context,
    ) -> anyhow::Result<RetrievedSongInfo> {
        let (song_mid, media_mid) = identifier
            .split_once(IDENTIFIER_SEPARATOR)
            .filter(|(song_mid, media_mid)| !song_mid.is_empty() && !media_mid.is_empty())
            .ok_or_else(|| anyhow!("QQ identifier is invalid"))?;
        let credentials = configured_cookie(context)
            .map(Credentials::parse)
            .unwrap_or_default();
        let filenames = candidate_filenames(media_mid, credentials.logged_in, context.enable_flac);
        let request = vkey_request(&filenames, song_mid, credentials.uin, &random_guid());
        let response = self.call(&request, context).await?;
        let url = playable_url(&response, &filenames).ok_or_else(|| refusal(&response))?;
        Ok(RetrievedSongInfo::builder()
            .source(Cow::Borrowed(ENGINE_ID))
            .url(url)
            .build())
    }
}

fn client_builder() -> reqwest::ClientBuilder {
    let mut headers = HeaderMap::with_capacity(4);
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/plain, */*"),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("zh-CN,zh;q=0.9"));
    headers.insert(
        reqwest::header::REFERER,
        HeaderValue::from_static(REFERER_VALUE),
    );
    reqwest::Client::builder()
        .connect_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .default_headers(headers)
}

fn configured_cookie(context: &Context) -> Option<&str> {
    context
        .config
        .as_ref()?
        .get_deref(Cow::Borrowed(COOKIE_CONFIG_KEY))
        .map(str::trim)
        .filter(|cookie| !cookie.is_empty())
}

/// What the vkey request needs from the user's cookie. The cookie itself is
/// forwarded verbatim; only this module interprets it.
#[derive(Debug, PartialEq, Eq)]
struct Credentials<'a> {
    uin: &'a str,
    logged_in: bool,
}

impl Default for Credentials<'_> {
    fn default() -> Self {
        Self {
            uin: "0",
            logged_in: false,
        }
    }
}

impl<'a> Credentials<'a> {
    fn parse(cookie: &'a str) -> Self {
        let field = |name: &str| {
            cookie.split(';').find_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                let value = value.trim();
                (key.trim() == name && !value.is_empty()).then_some(value)
            })
        };
        let key = field("qqmusic_key").or_else(|| field("qm_keyst"));
        // WeChat sign-ins keep the account in `wxuin`; y.qq.com marks them with
        // `login_type=2`, `tmeLoginType=1` and a `W_X` music key.
        let wechat = field("login_type") == Some("2")
            || field("tmeLoginType") == Some("1")
            || key.is_some_and(|key| key.starts_with("W_X"));
        let (preferred, other) = if wechat {
            ("wxuin", "uin")
        } else {
            ("uin", "wxuin")
        };
        let uin = field(preferred)
            .and_then(normalize_uin)
            .or_else(|| field(other).and_then(normalize_uin));
        Self {
            uin: uin.unwrap_or("0"),
            logged_in: uin.is_some() && key.is_some(),
        }
    }
}

/// QQ login pages set the account as `o` plus a zero-padded number.
fn normalize_uin(raw: &str) -> Option<&str> {
    let digits = raw.strip_prefix('o').unwrap_or(raw).trim_start_matches('0');
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())).then_some(digits)
}

/// Best quality first. Without a session only the guest M500 MP3 is asked
/// for, as the bundled engine did (QQ currently refuses that too).
fn candidate_filenames(media_mid: &str, logged_in: bool, enable_flac: bool) -> Vec<String> {
    let formats: &[(&str, &str)] = match (logged_in, enable_flac) {
        (true, true) => &[("F000", "flac"), ("M800", "mp3"), ("M500", "mp3")],
        (true, false) => &[("M800", "mp3"), ("M500", "mp3")],
        (false, _) => &[("M500", "mp3")],
    };
    formats
        .iter()
        .map(|(prefix, extension)| format!("{prefix}{media_mid}.{extension}"))
        .collect()
}

/// QQ only honours a device guid sent as a string of digits.
fn random_guid() -> String {
    // Each RandomState carries fresh random keys.
    let hash = RandomState::new().hash_one(());
    (1_000_000_000 + hash % 9_000_000_000).to_string()
}

fn search_request(keyword: &str) -> Value {
    json!({
        "search": {
            "method": "DoSearchForQQMusicDesktop",
            "module": "music.search.SearchCgiService",
            "param": { "num_per_page": 5, "page_num": 1, "query": keyword, "search_type": 0 }
        }
    })
}

/// One request for every candidate quality; QQ answers each file separately.
fn vkey_request(filenames: &[String], song_mid: &str, uin: &str, guid: &str) -> Value {
    json!({
        "req_0": {
            "module": "vkey.GetVkeyServer",
            "method": "CgiGetVkey",
            "param": {
                "guid": guid,
                "loginflag": 1,
                "filename": filenames,
                "songmid": vec![song_mid; filenames.len()],
                "songtype": vec![0; filenames.len()],
                "uin": uin,
                "platform": "20"
            }
        }
    })
}

/// Picks the search result to play, as `song_mid:::media_mid`.
///
/// Results without a media file are unplayable and skipped. Among the rest,
/// in QQ's order: the first whose duration is within tolerance wins; failing
/// that, the first that reports no duration (many uploads lack one). A result
/// with a known, mismatched duration is never chosen. Without an expected
/// duration the first playable result wins.
fn select_match(response: &Value, expected_ms: Option<i64>) -> Option<String> {
    let entries = response
        .pointer("/search/data/body/song/list")?
        .as_array()?
        .iter()
        .filter_map(|entry| {
            let song_mid = non_empty_str(entry.get("mid"))?;
            let media_mid = non_empty_str(entry.pointer("/file/media_mid"))?;
            let duration_ms = entry
                .get("interval")
                .and_then(Value::as_i64)
                .filter(|seconds| *seconds > 0)
                .map(|seconds| seconds.saturating_mul(1_000));
            Some((song_mid, media_mid, duration_ms))
        });

    let mut fallback = None;
    for (song_mid, media_mid, duration_ms) in entries {
        let identifier = || format!("{song_mid}{IDENTIFIER_SEPARATOR}{media_mid}");
        match (expected_ms.filter(|expected| *expected > 0), duration_ms) {
            (None, _) => return Some(identifier()),
            (Some(expected), Some(actual)) if (actual - expected).abs() < DURATION_TOLERANCE_MS => {
                return Some(identifier())
            }
            (Some(_), None) if fallback.is_none() => fallback = Some(identifier()),
            _ => {}
        }
    }
    fallback
}

/// The URL of the best candidate QQ granted. An empty `purl` means QQ refused
/// that file, not that it lives at the server root.
fn playable_url(response: &Value, filenames: &[String]) -> Option<String> {
    let data = response.pointer("/req_0/data")?;
    let server = data
        .get("sip")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find(|server| server.starts_with("http"))?;
    let granted = data.get("midurlinfo")?.as_array()?;
    let purl = filenames.iter().find_map(|filename| {
        granted
            .iter()
            .filter(|info| info.get("filename").and_then(Value::as_str) == Some(filename))
            .find_map(|info| non_empty_str(info.get("purl")))
    })?;
    Some(format!(
        "{}/{}",
        server.trim_end_matches('/'),
        purl.trim_start_matches('/')
    ))
}

/// Names the `result` codes QQ attached to the files it refused.
fn refusal(response: &Value) -> anyhow::Error {
    let mut codes: Vec<i64> = response
        .pointer("/req_0/data/midurlinfo")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|info| info.get("result").and_then(Value::as_i64))
        .filter(|code| *code != 0)
        .collect();
    codes.sort_unstable();
    codes.dedup();
    match codes.as_slice() {
        [] => anyhow!("QQ returned no playable URL"),
        [RESULT_REGION_LOCKED] => anyhow!(
            "QQ returned no playable URL (result 104003: region or license lock, \
             e.g. an IP outside mainland China)"
        ),
        _ => {
            let codes: Vec<String> = codes.iter().map(i64::to_string).collect();
            anyhow!("QQ returned no playable URL (result {})", codes.join(", "))
        }
    }
}

fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value?.as_str().filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use unm_types::{config::ConfigManager, Artist};

    use super::*;

    type Recorded = Arc<Mutex<Vec<String>>>;

    /// Serves canned search/vkey bodies and records each raw request head.
    async fn start_fake_qq(search: Value, vkey: Value) -> (String, Recorded) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let recorded = Recorded::default();
        let requests = recorded.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !head.ends_with(b"\r\n\r\n") {
                    let read = socket.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    head.extend_from_slice(&buffer[..read]);
                }
                let head = String::from_utf8(head).unwrap();
                let body = if head.contains("CgiGetVkey") {
                    vkey.to_string()
                } else {
                    search.to_string()
                };
                requests.lock().unwrap().push(head);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{address}/cgi-bin/musicu.fcg"), recorded)
    }

    fn engine(endpoint: String) -> QqEngine {
        QqEngine {
            endpoint,
            direct: client_builder().no_proxy().build().unwrap(),
            proxied: Mutex::new(None),
        }
    }

    fn context(cookie: &str, enable_flac: bool) -> Context {
        let mut context = Context::default();
        context.enable_flac = enable_flac;
        context.config = Some(ConfigManager::new(HashMap::from([(
            Cow::Borrowed(COOKIE_CONFIG_KEY),
            cookie.to_owned(),
        )])));
        context
    }

    fn song(duration: Option<i64>) -> Song {
        Song::builder()
            .name("晴天".to_owned())
            .artists(vec![Artist::builder().name("周杰伦".to_owned()).build()])
            .duration(duration)
            .build()
    }

    /// The decoded `data` JSON and lower-cased header lines of a request head.
    fn decode(head: &str) -> (Value, Vec<String>) {
        let mut lines = head.lines();
        let target = lines.next().unwrap().split(' ').nth(1).unwrap();
        let url = reqwest::Url::parse(&format!("http://fake{target}")).unwrap();
        let data = url
            .query_pairs()
            .find(|(key, _)| key == "data")
            .map(|(_, value)| serde_json::from_str(&value).unwrap())
            .unwrap();
        (data, lines.map(str::to_ascii_lowercase).collect())
    }

    fn search_results(entries: Value) -> Value {
        json!({ "search": { "data": { "body": { "song": { "list": entries } } } } })
    }

    fn vkey_response(purls: &[(&str, &str)]) -> Value {
        let infos: Vec<Value> = purls
            .iter()
            .map(|(filename, purl)| json!({ "filename": filename, "purl": purl }))
            .collect();
        json!({ "req_0": { "code": 0, "data": {
            "sip": ["http://ws.stream.qqmusic.qq.com/"],
            "midurlinfo": infos
        } } })
    }

    #[tokio::test]
    async fn search_then_retrieve_sends_what_qq_requires() {
        let (endpoint, recorded) = start_fake_qq(
            search_results(json!([
                { "mid": "nofile", "interval": 269 },
                { "mid": "song", "interval": 270, "file": { "media_mid": "media" } },
            ])),
            vkey_response(&[
                ("F000media.flac", ""),
                ("M800media.mp3", "M800media.mp3?vkey=granted"),
                ("M500media.mp3", "M500media.mp3?vkey=worse"),
            ]),
        )
        .await;
        let engine = engine(endpoint);
        let context = context("uin=o0012345; qqmusic_key=KEY", true);

        let found = engine
            .search(&song(Some(269_000)), &context)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.identifier, "song:::media");
        let retrieved = engine.retrieve(&found.identifier, &context).await.unwrap();
        assert_eq!(
            retrieved.url,
            "http://ws.stream.qqmusic.qq.com/M800media.mp3?vkey=granted"
        );

        let recorded = recorded.lock().unwrap();
        let (vkey, headers) = decode(&recorded[1]);
        let param = &vkey["req_0"]["param"];
        assert!(param["guid"]
            .as_str()
            .is_some_and(|guid| !guid.is_empty() && guid.bytes().all(|b| b.is_ascii_digit())));
        assert_eq!(
            param["filename"],
            json!(["F000media.flac", "M800media.mp3", "M500media.mp3"])
        );
        assert_eq!(param["songmid"], json!(["song", "song", "song"]));
        assert_eq!(param["uin"], json!("12345"));
        for expected in [
            "cookie: uin=o0012345; qqmusic_key=key",
            "referer: https://y.qq.com/",
            "user-agent: mozilla/5.0",
        ] {
            assert!(
                headers.iter().any(|line| line.starts_with(expected)),
                "missing {expected}"
            );
        }
    }

    #[tokio::test]
    async fn empty_purls_are_an_error_not_the_server_root() {
        let (endpoint, _) = start_fake_qq(
            search_results(json!([])),
            vkey_response(&[("M500media.mp3", "")]),
        )
        .await;
        let error = engine(endpoint)
            .retrieve(&"song:::media".to_owned(), &Context::default())
            .await
            .err()
            .expect("retrieve should fail");
        assert_eq!(error.to_string(), "QQ returned no playable URL");
    }

    #[test]
    fn refusals_name_the_result_codes() {
        let refused = |results: &[i64]| {
            let infos: Vec<Value> = results
                .iter()
                .map(|result| json!({ "filename": "f", "purl": "", "result": result }))
                .collect();
            refusal(&json!({ "req_0": { "data": { "midurlinfo": infos } } })).to_string()
        };
        assert!(refused(&[104_003, 104_003]).contains("result 104003: region or license lock"));
        assert!(refused(&[104_013, 0, 104_003]).ends_with("(result 104003, 104013)"));
    }

    #[test]
    fn results_without_a_media_file_are_skipped() {
        let response = search_results(json!([
            { "mid": "a", "interval": 240 },
            { "mid": "b", "interval": 240, "file": { "media_mid": "" } },
            { "mid": "c", "interval": 240, "file": { "media_mid": "c-file" } },
        ]));
        assert_eq!(
            select_match(&response, Some(240_000)).as_deref(),
            Some("c:::c-file")
        );
    }

    #[test]
    fn duration_matches_beat_unknown_durations_and_mismatches_never_win() {
        let entry = |mid: &str, interval: Value| json!({ "mid": mid, "interval": interval, "file": { "media_mid": mid } });
        let response = search_results(json!([
            entry("far", json!(120)),
            entry("unknown", Value::Null),
            entry("near", json!(242)),
        ]));
        assert_eq!(
            select_match(&response, Some(240_000)).as_deref(),
            Some("near:::near")
        );
        assert_eq!(
            select_match(&response, Some(400_000)).as_deref(),
            Some("unknown:::unknown")
        );
        assert_eq!(select_match(&response, None).as_deref(), Some("far:::far"));

        let only_mismatch = search_results(json!([entry("far", json!(120))]));
        assert_eq!(select_match(&only_mismatch, Some(240_000)), None);
    }

    #[test]
    fn uin_is_normalized_and_login_needs_a_key() {
        assert_eq!(
            Credentials::parse("uin=o0012345; qm_keyst=k"),
            Credentials {
                uin: "12345",
                logged_in: true
            }
        );
        assert_eq!(
            Credentials::parse(" qqmusic_key=k ;uin=678"),
            Credentials {
                uin: "678",
                logged_in: true
            }
        );
        assert_eq!(
            Credentials::parse("uin=12345"),
            Credentials {
                uin: "12345",
                logged_in: false
            }
        );
        for unusable in [
            "p_uin=1; qm_keyst=k",
            "uin=o0; qm_keyst=k",
            "uin=abc; qm_keyst=k",
        ] {
            assert_eq!(Credentials::parse(unusable), Credentials::default());
        }
    }

    #[test]
    fn wechat_sessions_use_wxuin() {
        let wechat = Credentials {
            uin: "777",
            logged_in: true,
        };
        for cookie in [
            "wxuin=o0777; qm_keyst=W_X_k",
            "uin=123; wxuin=777; login_type=2; qm_keyst=k",
            "uin=123; wxuin=777; tmeLoginType=1; qqmusic_key=k",
            "uin=123; wxuin=777; qm_keyst=W_X_k",
        ] {
            assert_eq!(Credentials::parse(cookie), wechat, "{cookie}");
        }
        assert_eq!(
            Credentials::parse("uin=123; wxuin=777; qm_keyst=Q_H_L_k").uin,
            "123"
        );
    }
}
