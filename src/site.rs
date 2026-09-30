//! The shell shared by the server-rendered public pages (Reads and 𝑳𝑳𝑴): head tags,
//! language alternates and the topic tabs that match the news page.
use crate::{error::Result, state::AppState};
use std::fmt::Write;

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
pub(crate) async fn layout(
    state: &AppState,
    en: bool,
    path: &str,
    title: &str,
    description: &str,
    content: &str,
    structured: Option<serde_json::Value>,
    chrome: &Chrome<'_>,
) -> Result<String> {
    let topics: Vec<(String, String, String)> = sqlx::query_as("SELECT id, name, name_en FROM topics WHERE enabled AND visible AND owner_user_id IS NULL ORDER BY sort_weight, created_at")
        .fetch_all(&state.pool).await?;
    let mut tabs = format!(
        "<a class=tab href=\"{}\">𝐀𝐋𝐋</a><a class=tab href=\"{}\">𝐀𝐈</a>",
        localized("/", en),
        escape(&localized("/?topic=insight", en))
    );
    let equity_tab = format!(
        "<a class=tab href=\"{}\">𝑬</a>",
        escape(&localized("/?topic=equity-insight", en))
    );
    if !topics.iter().any(|(id, _, _)| id == "overseas-ai") {
        tabs.push_str(&equity_tab);
    }
    for (id, name, name_en) in topics {
        let href = if PAGE_TOPICS.contains(&id.as_str()) {
            localized(&format!("/{id}"), en)
        } else {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("topic", &id);
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
            escape(choose(en, &name, &name_en))
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
    Ok(format!(
        r##"<!doctype html>
<html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title><meta name="description" content="{description}"><meta name="theme-color" media="(prefers-color-scheme: light)" content="#f6f6f4"><meta name="theme-color" media="(prefers-color-scheme: dark)" content="#0f1115">
<link rel="canonical" href="{canonical}"><link rel="alternate" hreflang="zh-CN" href="{zh}"><link rel="alternate" hreflang="en" href="{english}"><link rel="alternate" hreflang="x-default" href="{zh}">
<meta property="og:type" content="{og_type}"><meta property="og:title" content="{title}"><meta property="og:description" content="{description}"><meta property="og:url" content="{canonical}"><meta property="og:site_name" content="News"><meta name="twitter:card" content="summary">
<link rel="icon" type="image/svg+xml" href="/icon.svg"><link rel="alternate" type="application/atom+xml" title="{feed_title}" href="{feed_href}"><link rel="stylesheet" href="/styles.css?v=photos-v1"><link rel="stylesheet" href="{stylesheet}">{schema}</head>
<body class="public {section}"><header class="topbar"><a class="brand" href="{home}" aria-label="News"><img class="brand-logo" src="/icon.svg" alt=""><span>𝑵𝒆𝒘𝒔</span></a><nav class="tabs" aria-label="{topics_label}">{tabs}</nav><div class="tools"><a class="space-link button button-quiet button-small" href="/me/">{workspace}</a><a class="space-link button button-quiet button-small" href="{other_lang}" hreflang="{other_code}">{language_label}</a></div></header>
<main class="shell">{content}</main>
<button id="backToTop" class="button back-to-top" type="button" aria-label="{back_to_top}" title="{back_to_top}"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 12 6-6 6 6M12 6v12"/></svg></button>
<script src="/public.js?v=2" defer></script><script src="{script}" defer></script><script src="/stats.js?v=2" data-track-url="{track_url}" defer></script></body></html>"##,
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
    ))
}
