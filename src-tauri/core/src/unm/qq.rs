//! QQ Music provider replacing `unm_engine_qq` 0.4.0. That engine sends a
//! numeric `guid`, which QQ rejects with an empty `purl`, requests the song
//! mid instead of the media file, and turns an empty `purl` into a bare server
//! URL. Requests mirror the QQ desktop web client.

use std::{
    borrow::Cow,
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    time::Duration,
};

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use serde_json::{json, Value};
use unm_engine::interface::Engine;
use unm_types::{Context, RetrievedSongInfo, SerializedIdentifier, Song, SongSearchInformation};

pub const ENGINE_ID: &str = "qq";
const API_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// QQ and NetEase durations of the same recording differ by a few seconds.
const DURATION_TOLERANCE_MS: i64 = 5_000;
const IDENTIFIER_SEPARATOR: &str = ":::";

pub struct QqEngine {
    client: reqwest::Client,
}

impl Default for QqEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl QqEngine {
    pub fn new() -> Self {
        Self {
            client: build_client(None).expect("static QQ HTTP client configuration is valid"),
        }
    }

    async fn call(&self, body: &Value, ctx: &Context) -> anyhow::Result<Value> {
        let proxied;
        let client = match ctx.proxy_uri.as_deref() {
            Some(proxy) => {
                proxied = build_client(Some(proxy))?;
                &proxied
            }
            None => &self.client,
        };
        let mut request = client
            .get(API_URL)
            .query(&[("data", body.to_string())])
            .header(reqwest::header::ORIGIN, "http://y.qq.com")
            .header(reqwest::header::REFERER, "http://y.qq.com/");
        if let Some(cookie) = cookie(ctx) {
            request = request.header(reqwest::header::COOKIE, cookie);
        }
        let response = request
            .send()
            .await
            .context("QQ connection failed")?
            .error_for_status()
            .context("QQ returned an HTTP error")?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            bail!("QQ response is too large");
        }
        let bytes = response.bytes().await.context("QQ response interrupted")?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            bail!("QQ response is too large");
        }
        serde_json::from_slice(&bytes).context("QQ returned invalid JSON")
    }
}

#[async_trait]
impl Engine for QqEngine {
    async fn search<'a>(
        &self,
        song: &'a Song,
        ctx: &'a Context,
    ) -> anyhow::Result<Option<SongSearchInformation>> {
        let response = self.call(&search_request(&song.keyword()), ctx).await?;
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
        ctx: &'a Context,
    ) -> anyhow::Result<RetrievedSongInfo> {
        let (song_mid, media_mid) = identifier
            .split_once(IDENTIFIER_SEPARATOR)
            .filter(|(song_mid, media_mid)| !song_mid.is_empty() && !media_mid.is_empty())
            .ok_or_else(|| anyhow!("QQ identifier is invalid"))?;
        let uin = cookie(ctx).and_then(cookie_uin).unwrap_or("0");
        for (prefix, extension) in formats(ctx.enable_flac) {
            let filename = format!("{prefix}{media_mid}{extension}");
            let response = self
                .call(&vkey_request(&filename, song_mid, uin, random_guid()), ctx)
                .await?;
            if let Some(url) = playable_url(&response) {
                return Ok(RetrievedSongInfo::builder()
                    .source(Cow::Borrowed(ENGINE_ID))
                    .url(url)
                    .build());
            }
        }
        bail!("QQ returned no playable URL")
    }
}

fn build_client(proxy: Option<&str>) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT);
    if let Some(proxy) = proxy {
        builder = builder.proxy(reqwest::Proxy::all(proxy).context("QQ proxy is invalid")?);
    }
    builder.build().context("QQ HTTP client could not be built")
}

fn cookie(ctx: &Context) -> Option<&str> {
    ctx.config
        .as_ref()
        .and_then(|config| config.get_deref(Cow::Borrowed("qq:cookie")))
        .map(str::trim)
        .filter(|cookie| !cookie.is_empty())
}

/// The numeric account from `uin`, also accepting QQ's `o`-prefixed form.
fn cookie_uin(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|field| {
        let (name, value) = field.split_once('=')?;
        if name.trim() != "uin" {
            return None;
        }
        let value = value.trim();
        let digits = value.strip_prefix('o').unwrap_or(value);
        (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())).then_some(digits)
    })
}

fn formats(enable_flac: bool) -> &'static [(&'static str, &'static str)] {
    if enable_flac {
        &[("F000", ".flac"), ("M800", ".mp3"), ("M500", ".mp3")]
    } else {
        &[("M800", ".mp3"), ("M500", ".mp3")]
    }
}

fn random_guid() -> u32 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(std::process::id().into());
    (hasher.finish() % 9_999_999) as u32 + 1
}

fn search_request(keyword: &str) -> Value {
    json!({
        "search": {
            "method": "DoSearchForQQMusicDesktop",
            "module": "music.search.SearchCgiService",
            "param": {
                "num_per_page": 5,
                "page_num": 1,
                "query": keyword,
                "search_type": 0
            }
        }
    })
}

fn vkey_request(filename: &str, song_mid: &str, uin: &str, guid: u32) -> Value {
    json!({
        "req_0": {
            "module": "vkey.GetVkeyServer",
            "method": "CgiGetVkey",
            "param": {
                // QQ only accepts the device guid as a string.
                "guid": guid.to_string(),
                "loginflag": 1,
                "filename": [filename],
                "songmid": [song_mid],
                "songtype": [0],
                "uin": uin,
                "platform": "20"
            }
        }
    })
}

/// The first search result with a mid whose duration matches the NetEase
/// track, serialized as `song_mid:::media_mid`.
fn select_match(response: &Value, duration_ms: Option<i64>) -> Option<String> {
    response
        .pointer("/search/data/body/song/list")?
        .as_array()?
        .iter()
        .find_map(|entry| {
            let song_mid = entry.get("mid")?.as_str().filter(|mid| !mid.is_empty())?;
            if let Some(expected) = duration_ms.filter(|duration| *duration > 0) {
                let seconds = entry.get("interval")?.as_i64()?;
                if (seconds.saturating_mul(1_000) - expected).abs() >= DURATION_TOLERANCE_MS {
                    return None;
                }
            }
            let media_mid = entry
                .pointer("/file/media_mid")
                .and_then(Value::as_str)
                .filter(|mid| !mid.is_empty())
                .unwrap_or(song_mid);
            Some(format!("{song_mid}{IDENTIFIER_SEPARATOR}{media_mid}"))
        })
}

fn playable_url(response: &Value) -> Option<String> {
    let data = response.pointer("/req_0/data")?;
    let purl = data
        .pointer("/midurlinfo/0/purl")?
        .as_str()
        .filter(|purl| !purl.is_empty())?;
    let server = data
        .pointer("/sip/0")?
        .as_str()
        .filter(|server| server.starts_with("http"))?;
    Some(format!(
        "{}/{}",
        server.trim_end_matches('/'),
        purl.trim_start_matches('/')
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{cookie_uin, playable_url, select_match, vkey_request};

    fn search_response(entries: serde_json::Value) -> serde_json::Value {
        json!({ "search": { "data": { "body": { "song": { "list": entries } } } } })
    }

    #[test]
    fn vkey_requests_send_the_guid_as_a_string() {
        let request = vkey_request("M800media.mp3", "songmid", "12345", 42);
        let param = &request["req_0"]["param"];

        assert_eq!(param["guid"], json!("42"));
        assert_eq!(param["filename"], json!(["M800media.mp3"]));
        assert_eq!(param["songmid"], json!(["songmid"]));
        assert_eq!(param["uin"], json!("12345"));
        assert_eq!(param["platform"], json!("20"));
    }

    #[test]
    fn search_prefers_the_media_file_of_a_duration_matched_entry() {
        let response = search_response(json!([
            { "mid": "far", "interval": 120, "file": { "media_mid": "far-file" } },
            { "mid": "near", "interval": 241, "file": { "media_mid": "near-file" } },
        ]));

        assert_eq!(
            select_match(&response, Some(240_000)).as_deref(),
            Some("near:::near-file")
        );
        assert_eq!(
            select_match(&response, None).as_deref(),
            Some("far:::far-file")
        );
        assert_eq!(select_match(&response, Some(400_000)), None);
    }

    #[test]
    fn search_falls_back_to_the_song_mid_without_a_media_file() {
        let response = search_response(json!([
            { "mid": "", "interval": 240 },
            { "mid": "song", "interval": 240 },
        ]));

        assert_eq!(
            select_match(&response, Some(240_000)).as_deref(),
            Some("song:::song")
        );
    }

    #[test]
    fn empty_purls_are_not_turned_into_server_urls() {
        let empty = json!({ "req_0": { "data": {
            "sip": ["http://ws.stream.qqmusic.qq.com/"],
            "midurlinfo": [{ "purl": "" }]
        } } });
        let playable = json!({ "req_0": { "data": {
            "sip": ["http://ws.stream.qqmusic.qq.com/"],
            "midurlinfo": [{ "purl": "M800media.mp3?vkey=abc" }]
        } } });

        assert_eq!(playable_url(&empty), None);
        assert_eq!(
            playable_url(&playable).as_deref(),
            Some("http://ws.stream.qqmusic.qq.com/M800media.mp3?vkey=abc")
        );
    }

    #[test]
    fn uin_accepts_plain_and_o_prefixed_accounts_only() {
        assert_eq!(cookie_uin("qm_keyst=x; uin=12345"), Some("12345"));
        assert_eq!(cookie_uin("uin=o0012345; qm_keyst=x"), Some("0012345"));
        assert_eq!(cookie_uin("p_uin=999; qm_keyst=x"), None);
        assert_eq!(cookie_uin("uin=abc"), None);
    }
}
