//! Public X data shared by the photograph topics: post records from X's embed endpoint, which
//! needs no account, and the addresses of posts and their photographs.

use crate::{
    error::Result,
    fetch::{MAX_SOURCE, NewItem, NewsWindow},
    reads,
    state::AppState,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, de::IgnoredAny};
use url::Url;

/// The data behind X's embedded posts.
const EMBED_URL: &str = "https://cdn.syndication.twimg.com/tweet-result";
/// Photographs are shown at X's medium size, up to 1200 pixels on the longer side; ones much
/// smaller than that look soft when opened.
pub(crate) const MIN_LONGER_SIDE: u32 = 1000;

/// A post as X's embed data describes it. Unavailable posts come back as tombstones without
/// these fields and do not parse.
#[derive(Debug, Deserialize)]
pub(crate) struct Post {
    pub created_at: DateTime<Utc>,
    pub user: Author,
    #[serde(default)]
    pub possibly_sensitive: bool,
    /// The post's words, HTML-escaped and ending with the links to its media.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub lang: String,
    #[serde(default, rename = "mediaDetails")]
    pub media: Vec<Media>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Author {
    pub name: String,
    pub screen_name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Media {
    #[serde(rename = "type")]
    pub kind: String,
    pub media_url_https: String,
    pub original_info: Size,
    /// Present when X covers the photograph with a sensitive-content warning.
    #[serde(default)]
    sensitive_media_warning: Option<IgnoredAny>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Size {
    pub width: u32,
    pub height: u32,
}

impl Post {
    /// The author's account when it is a plain X handle.
    pub(crate) fn account(&self) -> Option<&str> {
        let account = self.user.screen_name.as_str();
        (!account.is_empty()
            && account
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .then_some(account)
    }

    /// X marks the post, or one of its photographs, as sensitive.
    pub(crate) fn sensitive(&self) -> bool {
        self.possibly_sensitive
            || self
                .media
                .iter()
                .any(|media| media.sensitive_media_warning.is_some())
    }

    /// What every X feed checks before taking a post: a real author, no sensitive mark, and
    /// a time inside the window. Returns the post's address and credit line.
    pub(crate) fn admit(
        &self,
        id: &str,
        window: NewsWindow,
    ) -> std::result::Result<(String, String), &'static str> {
        let account = self.account().ok_or("X 帖子作者无效")?;
        if self.sensitive() {
            return Err("X 标记为敏感内容");
        }
        if !window.contains(self.created_at) {
            return Err("发布时间不在采集窗口内");
        }
        Ok((
            format!("https://x.com/{account}/status/{id}"),
            format!("{} @{account}", self.user.name.trim()),
        ))
    }

    /// Rewrites the item with X's record of the post: its address, author and time.
    pub(crate) fn adopt(&self, item: &mut NewItem, url: String) {
        item.url = url;
        item.source = self.user.name.trim().chars().take(MAX_SOURCE).collect();
        item.published_at = Some(self.created_at);
    }

    /// The photographs in the post's order with their media keys.
    pub(crate) fn photos(&self) -> impl Iterator<Item = (String, &Size)> {
        self.media
            .iter()
            .filter(|media| media.kind == "photo")
            .filter_map(|media| Some((media_key(&media.media_url_https)?, &media.original_info)))
    }

    /// The words as the author wrote them: entities decoded, media links removed.
    pub(crate) fn words(&self) -> String {
        let text = self
            .text
            .split_inclusive(char::is_whitespace)
            .filter(|word| !word.trim_start().starts_with("https://t.co/"))
            .collect::<String>();
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
            .trim()
            .to_owned()
    }
}

pub(crate) async fn read_post(state: &AppState, id: &str) -> Result<Post> {
    let mut url = Url::parse(EMBED_URL).expect("fixed embed URL");
    url.query_pairs_mut()
        .append_pair("id", id)
        .append_pair("lang", "en")
        .append_pair("token", &embed_token(id));
    let (_, bytes) = reads::download(state, url, reads::Content::Json).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// The address of a photograph at the size the topics show.
pub(crate) fn photo_url(key: &str) -> String {
    format!("https://pbs.twimg.com/media/{key}?format=jpg&name=medium")
}

/// The media key of a photograph address, `pbs.twimg.com/media/<key>.jpg` or
/// `pbs.twimg.com/media/<key>?format=jpg`.
pub(crate) fn media_key(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if url.host_str() != Some("pbs.twimg.com") {
        return None;
    }
    let name = url.path().strip_prefix("/media/")?;
    let key = name.split_once('.').map_or(name, |(key, _)| key);
    (!key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')))
    .then(|| key.to_owned())
}

/// The account of a post address, `x.com/<account>/status/<id>`, in lower case.
pub(crate) fn author(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    let mut parts = url.path_segments()?;
    let account = parts.next()?;
    (!account.is_empty() && account != "i" && parts.next() == Some("status"))
        .then(|| account.to_ascii_lowercase())
}

/// X's embed widget sends a token derived from the post id: `id / 1e15 · π` in base 36,
/// with its zeros and point removed.
pub(crate) fn embed_token(id: &str) -> String {
    let value = id.parse::<f64>().unwrap_or_default() / 1e15 * std::f64::consts::PI;
    base36(value)
        .chars()
        .filter(|c| !matches!(c, '0' | '.'))
        .collect()
}

/// JavaScript's `Number.prototype.toString(36)` for the positive values of `embed_token`, as
/// V8's `DoubleToRadixCString` writes them: fraction digits stop at the precision of the input
/// and round half to even. Post ids below 2^63 keep the integer part far below 2^53, where
/// the integer digits are exact.
fn base36(value: f64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut integer = value.floor();
    let mut fraction = value - integer;
    let mut delta = (0.5 * (value.next_up() - value)).max(0.0_f64.next_up());
    let mut digits = Vec::new();
    if fraction >= delta {
        loop {
            fraction *= 36.0;
            delta *= 36.0;
            let digit = fraction as usize;
            digits.push(digit);
            fraction -= digit as f64;
            if (fraction > 0.5 || (fraction == 0.5 && digit % 2 == 1)) && fraction + delta > 1.0 {
                // Round up; a carry past the first fraction digit reaches the integer part.
                loop {
                    match digits.pop() {
                        Some(last) if last + 1 < 36 => {
                            digits.push(last + 1);
                            break;
                        }
                        Some(_) => {}
                        None => {
                            integer += 1.0;
                            break;
                        }
                    }
                }
                break;
            }
            if fraction < delta {
                break;
            }
        }
    }
    let mut text = Vec::new();
    loop {
        let remainder = integer % 36.0;
        text.push(DIGITS[remainder as usize]);
        integer = (integer - remainder) / 36.0;
        if integer <= 0.0 {
            break;
        }
    }
    text.reverse();
    if !digits.is_empty() {
        text.push(b'.');
        text.extend(digits.into_iter().map(|digit| DIGITS[digit]));
    }
    String::from_utf8(text).expect("ASCII digits")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn embed_token_matches_the_embed_widget() {
        // `((Number(id) / 1e15) * Math.PI).toString(36)` in Node.js.
        for (id, written, token) in [
            ("2103404903255204189", "53k.1hn5z8st", "53k1hn5z8st"),
            ("2105040167379640804", "53p.6fmlyjq2", "53p6fmlyjq2"),
            ("2104967031397437525", "53o.y5urkgy9", "53oy5urkgy9"),
            ("20", "0.000000006dq1a2xwd93", "6dq1a2xwd93"),
            ("1234567890123456789", "2zq.ic77uqyk", "2zqic77uqyk"),
            ("9223372036854775807", "mcw.2svce2ao", "mcw2svce2ao"),
            ("1600000000000000000", "3vm.jqiybmlu", "3vmjqiybmlu"),
            ("2105278415775695352", "53p.xdnnybyi", "53pxdnnybyi"),
            ("1111111111111111111", "2oy.npf5u2p5", "2oynpf5u2p5"),
            ("999999999999999", "3.53i5ab8p54", "353i5ab8p54"),
        ] {
            let value = id.parse::<f64>().unwrap() / 1e15 * std::f64::consts::PI;
            assert_eq!(base36(value), written, "{id}");
            assert_eq!(embed_token(id), token, "{id}");
        }
        assert_eq!(base36(35.0), "z");
        assert_eq!(base36(36.5), "10.i");
    }

    #[test]
    fn reads_media_keys_and_accounts() {
        assert_eq!(
            media_key("https://pbs.twimg.com/media/HTZHv1gaAAAWIqE.jpg").as_deref(),
            Some("HTZHv1gaAAAWIqE")
        );
        assert_eq!(
            media_key("https://pbs.twimg.com/media/HTZHv1gaAAAWIqE?format=jpg&name=large")
                .as_deref(),
            Some("HTZHv1gaAAAWIqE")
        );
        assert!(media_key("https://pbs.twimg.com/profile_images/1/a.jpg").is_none());
        assert!(media_key("https://example.com/media/HTZHv1gaAAAWIqE.jpg").is_none());
        assert!(media_key("https://pbs.twimg.com/media/.jpg").is_none());
        assert_eq!(
            author("https://x.com/Alice_1/status/2105000000000000001").as_deref(),
            Some("alice_1")
        );
        assert!(author("https://x.com/i/status/2105000000000000001").is_none());
        assert!(author("https://example.com/about").is_none());
    }

    #[test]
    fn words_drop_media_links_and_entities() {
        let post: Post = serde_json::from_value(json!({
            "created_at": "2026-09-30T23:35:34.000Z",
            "user": { "name": "océ", "screen_name": "oce_1899" },
            "text": "le canada 🍁\n\nmontréal &lt;3 &amp; plus https://t.co/DLAa8OTZXq",
        }))
        .unwrap();
        assert_eq!(post.words(), "le canada 🍁\n\nmontréal <3 & plus");
        assert_eq!(post.account(), Some("oce_1899"));
    }
}
