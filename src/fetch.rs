use crate::{
    error::{AppError, Result},
    state::AppState,
    topics::Topic,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::time::Duration;
use url::Url;

const RETRY_DELAYS: [u64; 2] = [10, 30];
const MAX_TITLE: usize = 200;
const MAX_SUMMARY: usize = 600;
const MAX_SOURCE: usize = 80;
const MAX_TAGS: usize = 6;
const MAX_TAG: usize = 30;
const MAX_ERROR: usize = 300;

/// A story already stored for the topic: shown to the model so it does not repeat it, and
/// used again on the way back to catch the repeats it makes anyway.
#[derive(Debug, FromRow)]
pub struct Known {
    pub title: String,
    pub url: String,
    pub fingerprint: Vec<u8>,
}

/// The Responses API usage block, reduced to what the run history shows.
#[derive(Debug, Default, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: i32,
    #[serde(default)]
    pub output_tokens: i32,
    #[serde(default)]
    pub cost_in_usd_ticks: i64,
    #[serde(default)]
    pub input_tokens_details: InputDetails,
    #[serde(default)]
    pub output_tokens_details: OutputDetails,
    #[serde(default)]
    pub server_side_tool_usage_details: ToolDetails,
}

#[derive(Debug, Default, Deserialize)]
pub struct InputDetails {
    #[serde(default)]
    pub cached_tokens: i32,
}

#[derive(Debug, Default, Deserialize)]
pub struct OutputDetails {
    #[serde(default)]
    pub reasoning_tokens: i32,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolDetails {
    #[serde(default)]
    pub web_search_calls: i32,
    #[serde(default)]
    pub x_search_calls: i32,
}

/// One story as the model returned it, before validation.
#[derive(Debug, Deserialize)]
pub struct Candidate {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub title_en: String,
    #[serde(default)]
    pub summary_en: String,
    pub url: String,
    #[serde(default)]
    pub source: String,
    pub published_at: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub importance: i64,
}

#[derive(Debug, Deserialize)]
struct ModelDigest {
    items: Vec<Candidate>,
}

pub struct Fetched {
    pub response_id: Option<String>,
    pub usage: Usage,
    pub candidates: Vec<Candidate>,
    /// The model's JSON as returned, kept on the run for troubleshooting.
    pub payload: Value,
}

/// A validated story ready to be stored.
#[derive(Debug)]
pub struct NewItem {
    pub fingerprint: Vec<u8>,
    pub title: String,
    pub summary: String,
    pub title_en: String,
    pub summary_en: String,
    pub url: String,
    pub source: String,
    pub published_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
    pub importance: i16,
}

/// Asks the model for fresh stories on the topic. Transient upstream failures (429, 5xx,
/// connection errors) are retried twice; everything else fails the run.
pub async fn fetch(state: &AppState, topic: &Topic, known: &[Known]) -> Result<Fetched> {
    let url = format!(
        "{}/v1/responses",
        state.config.ai_base_url.as_str().trim_end_matches('/')
    );
    let body = request_body(
        topic,
        Utc::now().with_timezone(&state.config.timezone),
        known,
    );
    let mut delays = RETRY_DELAYS.iter();
    let reply: Value = loop {
        let failure = match state
            .http
            .post(&url)
            .bearer_auth(&state.config.ai_api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => break response.json().await?,
            Ok(response)
                if response.status().as_u16() == 429 || response.status().is_server_error() =>
            {
                format!("AI 接口返回 {}", response.status())
            }
            Ok(response) => {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(AppError::Internal(
                    format!("AI 接口返回 {status}: {}", clip(&text, MAX_ERROR)).into(),
                ));
            }
            Err(error) => error.to_string(),
        };
        let Some(seconds) = delays.next() else {
            return Err(AppError::Internal(failure.into()));
        };
        tracing::warn!(%failure, seconds, "retrying AI request");
        tokio::time::sleep(Duration::from_secs(*seconds)).await;
    };
    parse_reply(reply)
}

fn request_body(topic: &Topic, now: DateTime<Tz>, known: &[Known]) -> Value {
    let collected = if known.is_empty() {
        "（无）".to_owned()
    } else {
        known
            .iter()
            .map(|item| format!("- {} | {}", item.title, item.url))
            .collect::<Vec<_>>()
            .join("\n")
    };
    json!({
        "model": topic.model,
        "reasoning": { "effort": topic.reasoning_effort },
        "store": false,
        "tools": [{ "type": "web_search" }, { "type": "x_search" }],
        "instructions": instructions(topic),
        "input": format!(
            "栏目方向：{}\n当前时间：{}（{}），UTC {}\n已收录（{} 条）：\n{}",
            topic.brief,
            now.format("%Y-%m-%d %H:%M"),
            now.timezone().name(),
            now.to_utc().format("%Y-%m-%d %H:%M"),
            known.len(),
            collected,
        ),
        "text": { "format": {
            "type": "json_schema", "name": "news_digest", "strict": true, "schema": schema(),
        } },
    })
}

fn instructions(topic: &Topic) -> String {
    format!(
        "你是一名严谨的新闻编辑，为「{name}」栏目整理最新资讯。\n规则：\n\
         1. 只收录发布时间在最近 {lookback} 小时内、可以核实的真实报道；每条必须给出指向具体文章页面的 URL，不能是站点首页或栏目页。\n\
         2. 先用 web_search 和 x_search 检索，再综合判断重要性；同一事件只保留一条，优先一手来源或权威媒体。检索次数控制在 {searches} 次以内。\n\
         3. 「已收录」列表中的事件不要重复，除非出现实质性新进展。\n\
         4. 每条给出中文标题、中文摘要（1 到 2 句，说清发生了什么、为什么重要），以及英文标题、英文摘要。\n\
         5. importance 取 1 到 5，5 表示行业级重大事件；tags 给 1 到 5 个短标签。\n\
         6. 最多返回 {max_items} 条；没有符合条件的新内容时返回空数组。\n\
         7. 严格按 JSON Schema 输出，不输出任何其他文字。",
        name = topic.name,
        lookback = topic.lookback_hours,
        searches = (topic.max_items + 2).min(10),
        max_items = topic.max_items,
    )
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["items"],
        "properties": { "items": { "type": "array", "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["title", "summary", "title_en", "summary_en", "url", "source", "published_at", "tags", "importance"],
            "properties": {
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "title_en": { "type": "string" },
                "summary_en": { "type": "string" },
                "url": { "type": "string" },
                "source": { "type": "string" },
                "published_at": { "type": ["string", "null"] },
                "tags": { "type": "array", "items": { "type": "string" } },
                "importance": { "type": "integer" }
            }
        } } }
    })
}

fn parse_reply(reply: Value) -> Result<Fetched> {
    if let Some(error) = reply.get("error").filter(|error| !error.is_null()) {
        return Err(AppError::Internal(
            format!("AI 接口错误: {}", clip(&error.to_string(), MAX_ERROR)).into(),
        ));
    }
    if reply["status"] != "completed" {
        return Err(AppError::Internal(
            format!("AI 响应未完成: {}", reply["status"]).into(),
        ));
    }
    let text = reply["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|output| output["type"] == "message")
        .filter_map(|output| output["content"].as_array())
        .flatten()
        .find(|content| content["type"] == "output_text")
        .and_then(|content| content["text"].as_str())
        .ok_or_else(|| AppError::Internal("AI 响应没有文本输出".into()))?;
    // Strict mode makes prose around the object unlikely, but the outermost braces still
    // hold it when a model slips.
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => text,
    };
    let payload: Value = serde_json::from_str(json)
        .map_err(|_| AppError::Internal("AI 响应不是有效的 JSON".into()))?;
    let digest: ModelDigest = serde_json::from_value(payload.clone())
        .map_err(|_| AppError::Internal("AI 响应不符合约定的结构".into()))?;
    Ok(Fetched {
        response_id: reply["id"].as_str().map(str::to_owned),
        usage: serde_json::from_value(reply["usage"].clone()).unwrap_or_default(),
        candidates: digest.items,
        payload,
    })
}

/// Turns a candidate into a storable item, or `None` when it fails the rules that keep the
/// digest trustworthy: a real http(s) URL, a title, and a publication time no older than
/// `cutoff` (the lookback window plus slack for timezone slips).
pub fn validate(candidate: Candidate, cutoff: DateTime<Utc>) -> Option<NewItem> {
    let url = Url::parse(candidate.url.trim()).ok().filter(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
    })?;
    let title = clip(&candidate.title, MAX_TITLE);
    if title.is_empty() {
        return None;
    }
    let published_at = candidate.published_at.as_deref().and_then(parse_time);
    if published_at.is_some_and(|at| at < cutoff) {
        return None;
    }
    let mut tags = candidate
        .tags
        .iter()
        .map(|tag| clip(tag, MAX_TAG))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    tags.dedup();
    tags.truncate(MAX_TAGS);
    Some(NewItem {
        fingerprint: Sha256::digest(canonical_url(&url).as_bytes()).to_vec(),
        title,
        summary: clip(&candidate.summary, MAX_SUMMARY),
        title_en: clip(&candidate.title_en, MAX_TITLE),
        summary_en: clip(&candidate.summary_en, MAX_SUMMARY),
        url: url.to_string(),
        source: clip(&candidate.source, MAX_SOURCE),
        published_at,
        tags,
        importance: candidate.importance.clamp(1, 5) as i16,
    })
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    DateTime::parse_from_rfc3339(value)
        .map(|at| at.to_utc())
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S")
                .ok()
                .map(|at| at.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .map(|day| day.and_time(NaiveTime::MIN).and_utc())
        })
}

/// The identity of a story: `www.` dropped, fragment and tracking parameters removed,
/// trailing slash trimmed. Scheme, host and default ports are already normalised by `Url`.
pub fn canonical_url(url: &Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    if let Some(host) = url
        .host_str()
        .and_then(|host| host.strip_prefix("www."))
        .map(str::to_owned)
    {
        let _ = url.set_host(Some(&host));
    }
    let kept = url
        .query_pairs()
        .filter(|(key, _)| !is_tracking(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    url.set_query(None);
    if !kept.is_empty() {
        url.query_pairs_mut().extend_pairs(kept);
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(if path.is_empty() { "/" } else { &path });
    url.to_string()
}

fn is_tracking(key: &str) -> bool {
    key.starts_with("utm_")
        || key.starts_with("share_")
        || matches!(
            key,
            "fbclid" | "gclid" | "igshid" | "ref" | "ref_src" | "spm"
        )
}

/// A title reduced to its alphanumeric core, so punctuation, spacing and full-width
/// variants cannot hide a duplicate.
pub fn normalize_title(title: &str) -> String {
    title
        .chars()
        .map(|c| match c as u32 {
            0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The same story under a slightly different headline: identical, or one normalised title
/// is a long prefix of the other (a trailing clause or source suffix).
pub fn similar_title(known: &[String], title: &str) -> bool {
    known.iter().any(|candidate| {
        candidate == title
            || (candidate.chars().count().min(title.chars().count()) >= 12
                && (candidate.starts_with(title) || title.starts_with(candidate)))
    })
}

fn clip(value: &str, max: usize) -> String {
    value.trim().chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        Candidate, canonical_url, normalize_title, parse_reply, parse_time, similar_title, validate,
    };
    use chrono::{Duration, TimeZone, Utc};
    use url::Url;

    fn candidate(url: &str, published_at: Option<&str>) -> Candidate {
        Candidate {
            title: " Title ".into(),
            summary: "Summary".into(),
            title_en: String::new(),
            summary_en: String::new(),
            url: url.into(),
            source: "Source".into(),
            published_at: published_at.map(str::to_owned),
            tags: vec!["a".into(), "a".into(), "".into(), "b".into()],
            importance: 9,
        }
    }

    #[test]
    fn parses_a_real_responses_reply() {
        let reply = serde_json::from_str(include_str!("../tests/fixtures/responses_web_low.json"))
            .expect("fixture is JSON");
        let fetched = parse_reply(reply).expect("reply parses");
        assert_eq!(fetched.candidates.len(), 5);
        assert_eq!(fetched.usage.input_tokens, 54878);
        assert_eq!(fetched.usage.input_tokens_details.cached_tokens, 20736);
        assert_eq!(
            fetched
                .usage
                .server_side_tool_usage_details
                .web_search_calls,
            8
        );
        assert_eq!(fetched.usage.cost_in_usd_ticks, 556_403_400);
        assert!(fetched.response_id.is_some());
        assert!(fetched.payload["items"].is_array());
    }

    #[test]
    fn rejects_incomplete_or_errored_replies() {
        let errored = serde_json::json!({ "status": "completed", "error": { "message": "boom" } });
        assert!(parse_reply(errored).is_err());
        let incomplete = serde_json::json!({ "status": "incomplete", "output": [] });
        assert!(parse_reply(incomplete).is_err());
    }

    #[test]
    fn canonicalises_urls() {
        let url = Url::parse("https://www.Example.com:443/a/b/?utm_source=x&id=7&fbclid=1#top")
            .expect("valid url");
        assert_eq!(canonical_url(&url), "https://example.com/a/b?id=7");
        let root = Url::parse("https://example.com/?utm_medium=m").expect("valid url");
        assert_eq!(canonical_url(&root), "https://example.com/");
    }

    #[test]
    fn validates_candidates() {
        let cutoff = Utc::now() - Duration::hours(36);
        let item = validate(
            candidate("https://example.com/story", Some("2099-01-01")),
            cutoff,
        )
        .expect("valid item");
        assert_eq!(item.title, "Title");
        assert_eq!(item.tags, ["a", "b"]);
        assert_eq!(item.importance, 5);
        assert!(validate(candidate("ftp://example.com/x", None), cutoff).is_none());
        assert!(validate(candidate("https://user:pw@example.com/x", None), cutoff).is_none());
        assert!(
            validate(
                candidate("https://example.com/x", Some("2001-01-01")),
                cutoff
            )
            .is_none()
        );
        assert!(validate(candidate("https://example.com/x", Some("garbage")), cutoff).is_some());
    }

    #[test]
    fn parses_common_time_formats() {
        assert_eq!(
            parse_time("2026-09-02T18:41:00Z"),
            Some(Utc.with_ymd_and_hms(2026, 9, 2, 18, 41, 0).unwrap())
        );
        assert_eq!(
            parse_time("2026-09-02T18:41:00"),
            Some(Utc.with_ymd_and_hms(2026, 9, 2, 18, 41, 0).unwrap())
        );
        assert_eq!(
            parse_time("2026-09-02"),
            Some(Utc.with_ymd_and_hms(2026, 9, 2, 0, 0, 0).unwrap())
        );
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn normalises_and_compares_titles() {
        assert_eq!(
            normalize_title("Ｇｅｍｉｎｉ 3.8 Flash 发布！"),
            "gemini38flash发布"
        );
        let known = vec![normalize_title(
            "Google Launches Gemini 3.8 Flash and Cyber Variant",
        )];
        assert!(similar_title(
            &known,
            &normalize_title("Google launches Gemini 3.8 Flash")
        ));
        assert!(!similar_title(
            &known,
            &normalize_title("OpenAI ships GPT-6")
        ));
        assert!(!similar_title(&known, &normalize_title("Google")));
    }
}
