//! The shell shared by the server-rendered public pages: head tags,
//! language alternates and the topic tabs that match the news page.
use crate::{
    error::{AppError, Result},
    reading::Reader,
    state::AppState,
};
use chrono::{DateTime, Utc};
use sqlx::{Postgres, QueryBuilder};
use std::fmt::Write;
use uuid::Uuid;

/// Topics with their own server-rendered page instead of the news feed.
const PAGE_TOPICS: [&str; 2] = ["reads", "llm"];

/// What differs between the server-rendered sections.
pub(crate) struct Chrome<'a> {
    /// The topic whose tab is marked as current.
    pub(crate) current: &'a str,
    /// Extra class on `<body>` for section styles.
    pub(crate) section: &'a str,
    pub(crate) stylesheet: &'a str,
    pub(crate) script: &'a str,
    /// Title and path of the section's Atom feed.
    pub(crate) feed: (&'a str, &'a str),
}

/// Where the next page of a list starts: the last item's `(first_seen_at, id)`. Lists run
/// newest first, and a cursor travels as `{micros}_{id}`.
pub(crate) type After = (DateTime<Utc>, Uuid);

pub(crate) fn parse_after(value: &str) -> Result<After> {
    let invalid = || AppError::BadRequest("分页参数无效".into());
    let (time, id) = value.split_once('_').ok_or_else(invalid)?;
    let time = time
        .parse()
        .ok()
        .and_then(DateTime::from_timestamp_micros)
        .ok_or_else(invalid)?;
    Ok((time, Uuid::parse_str(id).map_err(|_| invalid())?))
}

/// Ends an `items` query with the page condition, the order and one row beyond `limit`.
pub(crate) fn push_page(query: &mut QueryBuilder<Postgres>, after: Option<After>, limit: usize) {
    if let Some((time, id)) = after {
        query
            .push(" AND (items.first_seen_at, items.id) < (")
            .push_bind(time)
            .push(", ")
            .push_bind(id)
            .push(")");
    }
    query
        .push(" ORDER BY items.first_seen_at DESC, items.id DESC LIMIT ")
        .push_bind(i64::try_from(limit).unwrap_or(i64::MAX) + 1);
}

/// Drops the extra row `push_page` asked for and returns the next page's cursor, if any.
pub(crate) fn next_page<T>(
    rows: &mut Vec<T>,
    limit: usize,
    key: impl Fn(&T) -> After,
) -> Option<String> {
    (rows.len() > limit).then(|| {
        rows.truncate(limit);
        let (time, id) = key(&rows[limit - 1]);
        format!("{}_{id}", time.timestamp_micros())
    })
}

pub(crate) fn choose<'a>(en: bool, zh: &'a str, english: &'a str) -> &'a str {
    if en && !english.is_empty() {
        english
    } else {
        zh
    }
}

pub(crate) fn localized(path: &str, en: bool) -> String {
    format!(
        "{path}{}",
        if en {
            if path.contains('?') {
                "&lang=en"
            } else {
                "?lang=en"
            }
        } else {
            ""
        }
    )
}

pub(crate) fn absolute(state: &AppState, path: &str) -> String {
    format!(
        "{}{}",
        state.config.public_base_url.as_str().trim_end_matches('/'),
        path
    )
}

pub(crate) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn layout(
    state: &AppState,
    reader: Reader,
    en: bool,
    path: &str,
    title: &str,
    description: &str,
    content: &str,
    structured: Option<serde_json::Value>,
    chrome: &Chrome<'_>,
) -> String {
    let topics = state.digest.topics_for(reader);
    let mut tabs = format!(
        "<a class=tab href=\"{}\">𝐀𝐋𝐋</a><a class=tab href=\"{}\">𝐀𝐈</a>",
        localized("/", en),
        escape(&localized("/?topic=insight", en))
    );
    let equity_tab = format!(
        "<a class=tab href=\"{}\">𝑬</a>",
        escape(&localized("/?topic=equity-insight", en))
    );
    if !topics.iter().any(|topic| topic.id == "overseas-ai") {
        tabs.push_str(&equity_tab);
    }
    // A catalogue section's topics share one tab; swiping back into it enters the last
    // (public.js).
    let mut sections = crate::catalog::Section::ALL.map(|section| {
        let last = section
            .topics()
            .iter()
            .rev()
            .find(|topic| topics.iter().any(|known| known.id == **topic));
        (section, last)
    });
    for topic in topics.iter() {
        let id = &topic.id;
        if let Some(section) = crate::catalog::Section::of(id) {
            if let Some(last) = sections
                .iter_mut()
                .find(|(known, _)| *known == section)
                .and_then(|(_, last)| last.take())
            {
                let path = format!("/{}", section.path());
                let _ = write!(
                    tabs,
                    "<a class=tab href=\"{}\" data-last=\"{}\"{}>{}</a>",
                    escape(&localized(&path, en)),
                    escape(&localized(&format!("{path}/{last}"), en)),
                    if crate::catalog::Section::of(chrome.current) == Some(section) {
                        " aria-current=\"true\""
                    } else {
                        ""
                    },
                    section.label()
                );
            }
            continue;
        }
        let href = if PAGE_TOPICS.contains(&id.as_str()) {
            localized(&format!("/{id}"), en)
        } else {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("topic", id);
            if en {
                query.append_pair("lang", "en");
            }
            format!("/?{}", query.finish())
        };
        let _ = write!(
            tabs,
            "<a class=tab href=\"{}\"{}>{}</a>",
            escape(&href),
            if id == chrome.current {
                " aria-current=\"true\""
            } else {
                ""
            },
            escape(choose(en, &topic.name, &topic.name_en))
        );
        if id == "overseas-ai" {
            tabs.push_str(&equity_tab);
        }
    }
    let schema = structured
        .map(|value| {
            format!(
                "<script type=\"application/ld+json\">{}</script>",
                value
                    .to_string()
                    .replace('&', "\\u0026")
                    .replace('<', "\\u003c")
                    .replace('>', "\\u003e")
            )
        })
        .unwrap_or_default();
    format!(
        r##"<!doctype html>
<html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title><meta name="description" content="{description}"><meta name="theme-color" media="(prefers-color-scheme: light)" content="#f6f6f4"><meta name="theme-color" media="(prefers-color-scheme: dark)" content="#0f1115">
<link rel="canonical" href="{canonical}"><link rel="alternate" hreflang="zh-CN" href="{zh}"><link rel="alternate" hreflang="en" href="{english}"><link rel="alternate" hreflang="x-default" href="{zh}">
<meta property="og:type" content="{og_type}"><meta property="og:title" content="{title}"><meta property="og:description" content="{description}"><meta property="og:url" content="{canonical}"><meta property="og:site_name" content="News"><meta name="twitter:card" content="summary">
<link rel="icon" type="image/svg+xml" href="/icon.svg"><link rel="alternate" type="application/atom+xml" title="{feed_title}" href="{feed_href}"><link rel="stylesheet" href="/styles.css?v=faint"><link rel="stylesheet" href="{stylesheet}">{schema}</head>
<body class="public {section}"><header class="topbar"><a class="brand" href="{home}" aria-label="News"><img class="brand-logo" src="/icon.svg" alt=""><span>𝑵𝒆𝒘𝒔</span></a><nav class="tabs" aria-label="{topics_label}">{tabs}</nav><div class="tools"><a class="space-link button button-quiet button-small" href="/me/">{workspace}</a><a class="space-link button button-quiet button-small" href="{other_lang}" hreflang="{other_code}">{language_label}</a></div></header>
<main class="shell">{content}</main>
<button id="backToTop" class="button back-to-top" type="button" aria-label="{back_to_top}" title="{back_to_top}"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 12 6-6 6 6M12 6v12"/></svg></button>
<script src="/public.js?v=10" defer></script><script src="{script}" defer></script><script src="/stats.js?v=3" data-track-url="{track_url}" defer></script></body></html>"##,
        lang = choose(en, "zh-CN", "en"),
        title = escape(title),
        description = escape(description),
        canonical = escape(&absolute(state, &localized(path, en))),
        zh = escape(&absolute(state, path)),
        english = escape(&absolute(state, &localized(path, true))),
        og_type = if schema.is_empty() {
            "website"
        } else {
            "article"
        },
        feed_title = escape(chrome.feed.0),
        feed_href = escape(chrome.feed.1),
        stylesheet = escape(chrome.stylesheet),
        section = chrome.section,
        script = escape(chrome.script),
        home = localized("/", en),
        topics_label = choose(en, "主题", "Topics"),
        workspace = choose(en, "工作台", "Workspace"),
        other_lang = escape(&localized(path, !en)),
        other_code = choose(en, "en", "zh-CN"),
        language_label = choose(en, "EN", "中文"),
        back_to_top = choose(en, "回到顶部", "Back to top"),
        track_url = crate::reads::TRACK_URL,
    )
}
