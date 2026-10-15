//! The shell shared by the server-rendered public pages: head tags,
//! language alternates and the topic tabs that match the news page.
use crate::{
    error::{AppError, Result},
    reading::Reader,
    state::AppState,
};
use chrono::{DateTime, Utc};
use pulldown_cmark::{Event, Tag, TagEnd, html};
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

/// A topic's name in the page's language, as the navigation shows it. Pages are served only
/// for topics the reader may see, so the id stands in only if the topic is gone.
pub(crate) fn topic_name(state: &AppState, reader: Reader, id: &str, en: bool) -> String {
    state
        .digest
        .topics_for(reader)
        .iter()
        .find(|topic| topic.id == id)
        .map_or_else(
            || id.to_owned(),
            |topic| choose(en, &topic.name, &topic.name_en).to_owned(),
        )
}

/// A search reads this many characters of its words.
pub(crate) const SEARCH_CHARS: usize = 40;

/// The words a search box sent, trimmed and cut to `SEARCH_CHARS`.
pub(crate) fn search_words(q: Option<&str>) -> String {
    q.unwrap_or_default()
        .trim()
        .chars()
        .take(SEARCH_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// An ILIKE pattern finding `words` anywhere; ILIKE's own wildcards in them match literally.
pub(crate) fn containing(words: &str) -> String {
    format!(
        "%{}%",
        words
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// A search box sending its words to `action` as `q`, in the page's language.
pub(crate) fn search_box(class: &str, action: &str, words: &str, hint: &str, en: bool) -> String {
    format!(
        "<form class={class} action=\"{}\" role=search><input type=search name=q value=\"{}\" maxlength={SEARCH_CHARS} placeholder=\"{hint}\" aria-label=\"{hint}\" enterkeyhint=search>{}</form>",
        escape(action),
        escape(words),
        if en {
            "<input type=hidden name=lang value=en>"
        } else {
            ""
        }
    )
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

/// A page's headline, escaped. A Chinese or Japanese headline is cut after its punctuation into
/// phrases that wrap as wholes (`.phrase` in reads.css), so a line ends at "：" or "，" whenever
/// the next phrase fits instead of splitting a phrase; one without such punctuation stays as it is.
pub(crate) fn headline(title: &str) -> String {
    let pause = |c: char| matches!(c, '，' | '：' | '；' | '、' | '！' | '？' | '。');
    let mut phrases = Vec::new();
    let mut start = 0;
    let mut chars = title.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if pause(c) && !chars.peek().is_some_and(|&(_, next)| pause(next)) {
            phrases.push(&title[start..at + c.len_utf8()]);
            start = at + c.len_utf8();
        }
    }
    phrases.push(&title[start..]);
    phrases.retain(|phrase| !phrase.trim().is_empty());
    if phrases.len() < 2 {
        return escape(title);
    }
    phrases
        .iter()
        .map(|phrase| format!("<span class=phrase>{}</span>", escape(phrase)))
        .collect()
}

/// The eye before a read count.
pub(crate) const EYE: &str = r#"<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></svg>"#;

/// Where a Markdown picture loads from, and its size when known.
pub(crate) struct Picture {
    pub(crate) src: String,
    pub(crate) size: Option<(i32, i32)>,
}

/// Markdown as HTML that can't run anything: raw HTML is dropped, links keep only http(s)
/// addresses without credentials, and a picture shows only where `picture` gives it an
/// address; otherwise its description stays as text.
pub(crate) fn markdown<'a>(
    events: impl IntoIterator<Item = Event<'a>>,
    picture: impl Fn(&str) -> Option<Picture>,
) -> String {
    let mut image = None;
    let mut alt = String::new();
    let parser = events.into_iter().filter_map(|event| match event {
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Image { dest_url, .. }) => {
            alt.clear();
            image = Some(picture(&dest_url));
            None
        }
        Event::End(TagEnd::Image) => Some(match image.take().flatten() {
            Some(Picture { src, size }) => {
                let size = size
                    .map(|(width, height)| format!(" width={width} height={height}"))
                    .unwrap_or_default();
                Event::Html(
                    format!(
                        "<img src=\"{}\" alt=\"{}\"{size} loading=lazy decoding=async referrerpolicy=no-referrer>",
                        escape(&src),
                        escape(&alt)
                    )
                    .into(),
                )
            }
            None => Event::Text(std::mem::take(&mut alt).into()),
        }),
        Event::Text(text) | Event::Code(text) if image.is_some() => {
            alt.push_str(&text);
            None
        }
        _ if image.is_some() => None,
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe = url::Url::parse(&dest_url).is_ok_and(|url| {
                matches!(url.scheme(), "https" | "http")
                    && url.username().is_empty()
                    && url.password().is_none()
            });
            Some(Event::Start(Tag::Link {
                link_type,
                dest_url: if safe { dest_url } else { "".into() },
                title,
                id,
            }))
        }
        other => Some(other),
    });
    let mut html = String::new();
    html::push_html(&mut html, parser);
    html
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
        "<a class=tab href=\"{}\">{}</a>",
        escape(&localized("/?topic=equity-insight", en)),
        choose(en, "股市", "𝑬")
    );
    if !topics.iter().any(|topic| topic.id == "overseas-ai") {
        tabs.push_str(&equity_tab);
    }
    // 分享 is not a topic: readers write it. Its tab follows 精读.
    let articles_tab = format!(
        "<a class=tab href=\"{}\"{}>{}</a>",
        localized("/articles", en),
        if chrome.current == "articles" {
            " aria-current=\"true\""
        } else {
            ""
        },
        crate::articles::label(en)
    );
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
                    section.label(en)
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
        if id == "reads" {
            tabs.push_str(&articles_tab);
        }
    }
    if !topics.iter().any(|topic| topic.id == "reads") {
        tabs.push_str(&articles_tab);
    }
    // An article's picture in its structured data also stands for it where the page is shared.
    let card = structured
        .as_ref()
        .and_then(|value| value["image"].as_str())
        .map(|image| {
            format!(
                "<meta property=\"og:image\" content=\"{}\"><meta name=\"twitter:card\" content=\"summary_large_image\">",
                escape(image)
            )
        })
        .unwrap_or_else(|| "<meta name=\"twitter:card\" content=\"summary\">".into());
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
<meta property="og:type" content="{og_type}"><meta property="og:title" content="{title}"><meta property="og:description" content="{description}"><meta property="og:url" content="{canonical}"><meta property="og:site_name" content="News">{card}
<link rel="icon" type="image/svg+xml" href="/icon.svg"><link rel="alternate" type="application/atom+xml" title="{feed_title}" href="{feed_href}"><link rel="stylesheet" href="/styles.css?v=figures"><link rel="stylesheet" href="{stylesheet}">{schema}</head>
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

#[cfg(test)]
mod tests {
    use super::headline;

    #[test]
    fn headlines_break_after_their_punctuation_and_stay_escaped() {
        assert_eq!(
            headline("认识 xabcnews.com：让 AI 替你读全网，你只读值得读的"),
            "<span class=phrase>认识 xabcnews.com：</span><span class=phrase>让 AI 替你读全网，</span><span class=phrase>你只读值得读的</span>"
        );
        // Punctuation that runs together stays with the phrase before it.
        assert_eq!(
            headline("真的吗？！<b>不</b>"),
            "<span class=phrase>真的吗？！</span><span class=phrase>&lt;b&gt;不&lt;/b&gt;</span>"
        );
        // One phrase, or none to cut, is left as it is.
        assert_eq!(
            headline("廉价模型正在动摇 <AI> 基建"),
            "廉价模型正在动摇 &lt;AI&gt; 基建"
        );
        assert_eq!(headline("Why it matters: a test"), "Why it matters: a test");
        assert_eq!(headline("结尾的问号？"), "结尾的问号？");
    }
}
