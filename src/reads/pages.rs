use crate::{
    digest::{Rendered, conditional},
    error::{AppError, Result},
    state::AppState,
};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use chrono::{DateTime, Utc};
use pulldown_cmark::{Event, Parser, Tag, TagEnd, html};
use serde::Deserialize;
use serde_json::json;
use sqlx::FromRow;
use std::fmt::Write;
use uuid::Uuid;

const PAGE_SIZE: i64 = 12;
const SITEMAP_SIZE: i64 = 10000;
const VISIBLE: &str = "FROM items i JOIN read_articles a ON a.item_id = i.id JOIN topics t ON t.id = i.topic_id WHERE i.topic_id = 'reads' AND t.enabled AND t.visible AND t.owner_user_id IS NULL AND NOT i.hidden";
const CARD: &str = "i.id, i.title, i.title_en, i.summary, i.summary_en, i.source, i.url, i.first_seen_at, a.author, a.views";

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/reads", get(index))
        .route("/reads/{id}", get(detail))
        .route("/sitemap.xml", get(sitemap))
        .route("/reads/sitemap/{page}", get(sitemap_page))
        .route("/robots.txt", get(robots))
        .layer(tower_http::compression::CompressionLayer::new())
}

#[derive(Default, Deserialize)]
struct Params {
    lang: Option<String>,
    page: Option<u32>,
    view: Option<String>,
}

impl Params {
    fn english(&self) -> bool {
        self.lang.as_deref() == Some("en")
    }
    fn page(&self) -> Result<u32> {
        match self.page.unwrap_or(1) {
            page @ 1..=100000 => Ok(page),
            _ => Err(AppError::NotFound("页面不存在".into())),
        }
    }
}

#[derive(FromRow)]
struct Card {
    id: Uuid,
    title: String,
    title_en: String,
    summary: String,
    summary_en: String,
    source: String,
    url: String,
    first_seen_at: DateTime<Utc>,
    author: String,
    views: Option<i64>,
}

#[derive(FromRow)]
struct Detail {
    #[sqlx(flatten)]
    card: Card,
    body: String,
    body_en: String,
    source_title: String,
    source_language: String,
    full_text: Option<sqlx::types::Json<super::full_text::Document>>,
}

fn choose<'a>(en: bool, zh: &'a str, english: &'a str) -> &'a str {
    if en && !english.is_empty() {
        english
    } else {
        zh
    }
}

fn localized(path: &str, en: bool) -> String {
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

fn listing(page: u32, en: bool) -> String {
    localized(
        &if page == 1 {
            "/reads".into()
        } else {
            format!("/reads?page={page}")
        },
        en,
    )
}

async fn index(
    State(state): State<AppState>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.english();
    let page = params.page()?;
    let enabled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM topics WHERE id='reads' AND enabled AND visible AND owner_user_id IS NULL)")
        .fetch_one(&state.pool).await?;
    if !enabled {
        return Err(AppError::NotFound("主题不存在".into()));
    }
    let hot = params.view.as_deref() == Some("hot");
    if hot && page != 1 {
        return Err(AppError::NotFound("页面不存在".into()));
    }
    let mut rows;
    if hot {
        let mut query = sqlx::QueryBuilder::new("SELECT ");
        query.push(CARD).push(" ").push(VISIBLE).push(
            " AND a.hot_rank IS NOT NULL AND i.first_seen_at >= NOW() - INTERVAL '30 days' ORDER BY a.hot_rank LIMIT 24"
        );
        rows = query
            .build_query_as::<Card>()
            .fetch_all(&state.pool)
            .await?;
    } else {
        let mut query = sqlx::QueryBuilder::new("SELECT ");
        query
            .push(CARD)
            .push(" ")
            .push(VISIBLE)
            .push(" ORDER BY i.pinned DESC, i.first_seen_at DESC, i.id DESC LIMIT ")
            .push_bind(PAGE_SIZE + 1)
            .push(" OFFSET ")
            .push_bind(i64::from(page - 1) * PAGE_SIZE);
        rows = query
            .build_query_as::<Card>()
            .fetch_all(&state.pool)
            .await?;
    }
    if page > 1 && rows.is_empty() {
        return Err(AppError::NotFound("页面不存在".into()));
    }
    let more = !hot && rows.len() > PAGE_SIZE as usize;
    if !hot {
        rows.truncate(PAGE_SIZE as usize);
    }
    let description = choose(
        en,
        "值得读完的文章与观点",
        "Ideas and stories worth your time",
    );
    let mut content = format!("<header class=reads-heading><h1>𝑹𝒆𝒂𝒅𝒔</h1><p>{description}</p>");
    let _ = write!(
        content,
        "<nav class=reads-views aria-label=\"{}\"><a href=\"{}\"{}>{}</a><a href=\"{}\"{}>{}</a></nav></header><div class=reads-list>",
        choose(en, "阅读排序", "Reading order"),
        localized("/reads", en),
        if !hot { " aria-current=\"page\"" } else { "" },
        choose(en, "最新", "Latest"),
        escape(&localized("/reads?view=hot", en)),
        if hot { " aria-current=\"page\"" } else { "" },
        choose(en, "热读", "Popular")
    );
    for (index, row) in rows.iter().enumerate() {
        content.push_str(&card(
            row,
            en,
            hot.then_some(index + 1),
            state.config.timezone,
        ));
    }
    if rows.is_empty() {
        content.push_str(if hot { choose(
            en,
            "<p class=muted>热读正在积累，先看看<a href=\"/reads\">最新文章</a>。</p>",
            "<p class=muted>Popular reads are taking shape. Explore the <a href=\"/reads?lang=en\">latest articles</a>.</p>",
        ) } else { choose(
            en,
            "<p class=muted>第一篇精选，正在路上。</p>",
            "<p class=muted>The first read is on its way.</p>",
        ) });
    }
    content.push_str("</div><nav class=reads-pagination aria-label=Pagination>");
    if page > 1 {
        let _ = write!(
            content,
            "<a rel=prev href=\"{}\">{}</a>",
            escape(&listing(page - 1, en)),
            choose(en, "上一页", "Newer")
        );
    }
    if more {
        let _ = write!(
            content,
            "<a rel=next href=\"{}\">{}</a>",
            escape(&listing(page + 1, en)),
            choose(en, "继续阅读", "Older reads")
        );
    }
    content.push_str("</nav>");
    let path = if hot {
        "/reads?view=hot".into()
    } else {
        listing(page, false)
    };
    let title = if hot {
        format!("{} · 𝑹𝒆𝒂𝒅𝒔", choose(en, "热读", "Popular"))
    } else if page == 1 {
        "𝑹𝒆𝒂𝒅𝒔 · 𝑵𝒆𝒘𝒔".to_owned()
    } else {
        format!("𝑹𝒆𝒂𝒅𝒔 · {page} · 𝑵𝒆𝒘𝒔")
    };
    let html = layout(&state, en, &path, &title, description, &content, None).await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let id = Uuid::parse_str(&id).map_err(|_| AppError::NotFound("文章不存在".into()))?;
    let mut query = sqlx::QueryBuilder::new("SELECT ");
    query
        .push(CARD)
        .push(", a.body, a.body_en, a.source_title, a.source_language, a.full_text ")
        .push(VISIBLE)
        .push(" AND i.id = ")
        .push_bind(id);
    let row = query
        .build_query_as::<Detail>()
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("文章不存在".into()))?;
    let en = params.english();
    let item = &row.card;
    let title = choose(en, &item.title, &item.title_en);
    let summary = choose(en, &item.summary, &item.summary_en);
    let body = choose(en, &row.body, &row.body_en);
    let language = choose(en, "zh-CN", "en");
    let author = if item.author.is_empty() {
        &item.source
    } else {
        &item.author
    };
    let views = format!(
        "<span class=read-views data-page-views hidden title=\"{}\">{EYE}<span data-page-views-value></span></span>",
        choose(en, "累计阅读次数", "Total page views")
    );
    let mut content = format!(
        "<article class=read><header class=read-heading><div class=item-meta><time datetime=\"{}\">{}</time></div><div class=read-title><h1>{}</h1>{views}</div><p class=read-deck>{}</p><div class=item-meta><span>{}</span><span>·</span><a href=\"{}\" rel=\"noopener noreferrer\">{}</a></div></header><div class=read-body lang=\"{}\">{}</div>",
        item.first_seen_at.to_rfc3339(),
        item.first_seen_at
            .with_timezone(&state.config.timezone)
            .format("%Y-%m-%d"),
        escape(title),
        escape(summary),
        choose(en, "原文", "Source"),
        escape(&item.url),
        escape(author),
        escape(language),
        render_markdown(body)
    );
    if let Some(document) = &row.full_text {
        let translated = document.translated(en);
        let _ = write!(
            content,
            "<section class=read-full><h2>{}</h2><div class=read-body lang=\"{}\">{}</div></section>",
            if translated {
                choose(en, "全文译文", "Full translation")
            } else {
                choose(en, "原文", "Original article")
            },
            escape(if translated {
                choose(en, "zh-CN", "en")
            } else {
                &row.source_language
            }),
            render_events(
                document.events(en),
                &document.images,
                Some(&state.config.tmpfile_base_url)
            ),
        );
    }
    let _ = write!(
        content,
        "<footer class=read-source><a href=\"{}\" rel=\"noopener noreferrer\">{} ↗</a>",
        escape(&item.url),
        escape(&row.source_title)
    );
    let _ = write!(
        content,
        "<p>{}</p>",
        choose(
            en,
            "精读与翻译由 AI 辅助整理，原文观点与内容归属原作者。",
            "AI-assisted reading guide and translation. Original content is attributed to its author.",
        )
    );
    let _ = write!(
        content,
        "</footer><div class=read-actions><a href=\"{}\">← 𝑹𝒆𝒂𝒅𝒔</a></div></article>",
        localized("/reads", en)
    );
    // Rank only the latest 200 visible alternatives, then load the three cards.
    let mut query = sqlx::QueryBuilder::new(
        "WITH recent AS MATERIALIZED (
            SELECT id, tags, first_seen_at FROM items
            WHERE topic_id = 'reads' AND NOT hidden AND id <> ",
    );
    query
        .push_bind(id)
        .push(
            " ORDER BY first_seen_at DESC, id DESC LIMIT 200
        ), related AS (
            SELECT id, first_seen_at, tags && (SELECT tags FROM items WHERE id = ",
        )
        .push_bind(id)
        .push(
            ") AS matches FROM recent
            ORDER BY matches DESC, first_seen_at DESC, id DESC LIMIT 3
        ) SELECT ",
        )
        .push(CARD)
        .push(
            " FROM related r JOIN items i ON i.id = r.id
            JOIN read_articles a ON a.item_id = i.id JOIN topics t ON t.id = i.topic_id
            WHERE t.enabled AND t.visible AND t.owner_user_id IS NULL
            ORDER BY r.matches DESC, r.first_seen_at DESC, r.id DESC",
        );
    let related = query
        .build_query_as::<Card>()
        .fetch_all(&state.pool)
        .await?;
    if !related.is_empty() {
        let _ = write!(
            content,
            "<aside class=read-related><h2>{}</h2>",
            choose(en, "继续阅读", "Keep reading")
        );
        for row in &related {
            content.push_str(&card(row, en, None, state.config.timezone));
        }
        content.push_str("</aside>");
    }
    let path = format!("/reads/{id}");
    let canonical = absolute(&state, &localized(&path, en));
    let structured = json!({
        "@context": "https://schema.org", "@type":"Article", "headline":title,
        "description":summary, "url":canonical, "mainEntityOfPage":canonical,
        "datePublished":item.first_seen_at.to_rfc3339(), "inLanguage":language,
        "publisher":{"@type":"Organization", "name":"News", "url":state.config.public_base_url},
        "author":{"@type":"Organization", "name":"News"},
        "isBasedOn": item.url,
    });
    let html = layout(
        &state,
        en,
        &path,
        &format!("{title} · 𝑹𝒆𝒂𝒅𝒔"),
        summary,
        &content,
        Some(structured),
    )
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

const EYE: &str = r#"<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></svg>"#;

fn card(item: &Card, en: bool, rank: Option<usize>, timezone: chrono_tz::Tz) -> String {
    let rank = rank
        .map(|rank| format!("<span class=read-rank aria-hidden=true>{rank:02}</span>"))
        .unwrap_or_default();
    let views = item
        .views
        .filter(|views| *views > 0 && item.first_seen_at >= Utc::now() - chrono::Duration::days(30))
        .map(|views| {
            format!(
                " · <span class=read-views title=\"{}\">{EYE}<span>{views}</span></span>",
                choose(en, "累计阅读次数", "Total page views")
            )
        })
        .unwrap_or_default();
    format!(
        "<article class=item>{rank}<div class=read-card><div class=item-meta><span>{}</span> · <span class=read-date><time datetime=\"{}\">{}</time>{views}</span></div><h2 class=item-title><a href=\"{}\">{}</a></h2><p class=item-summary>{}</p></div></article>",
        escape(&item.source),
        item.first_seen_at.to_rfc3339(),
        item.first_seen_at
            .with_timezone(&timezone)
            .format("%Y-%m-%d"),
        localized(&format!("/reads/{}", item.id), en),
        escape(choose(en, &item.title, &item.title_en)),
        escape(choose(en, &item.summary, &item.summary_en))
    )
}

fn absolute(state: &AppState, path: &str) -> String {
    format!(
        "{}{}",
        state.config.public_base_url.as_str().trim_end_matches('/'),
        path
    )
}

#[allow(clippy::too_many_arguments)]
async fn layout(
    state: &AppState,
    en: bool,
    path: &str,
    title: &str,
    description: &str,
    content: &str,
    structured: Option<serde_json::Value>,
) -> Result<String> {
    let topics: Vec<(String, String, String)> = sqlx::query_as("SELECT id, name, name_en FROM topics WHERE enabled AND visible AND owner_user_id IS NULL ORDER BY sort_weight, id")
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
        let href = if id == "reads" {
            localized("/reads", en)
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
            if id == "reads" {
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
<link rel="icon" type="image/svg+xml" href="/icon.svg"><link rel="alternate" type="application/atom+xml" title="Reads" href="/feed.xml?topic=reads"><link rel="stylesheet" href="/styles.css?v=tags-row-v1"><link rel="stylesheet" href="/reads.css?v=6">{schema}</head>
<body class="public reads"><header class="topbar"><a class="brand" href="{home}" aria-label="News"><img class="brand-logo" src="/icon.svg" alt=""><span>𝑵𝒆𝒘𝒔</span></a><nav class="tabs" aria-label="{topics_label}">{tabs}</nav><div class="tools"><a class="space-link button button-quiet button-small" href="/me/">{workspace}</a><a class="space-link button button-quiet button-small" href="{other_lang}" hreflang="{other_code}">{language_label}</a></div></header>
<main class="shell">{content}</main>
<button id="backToTop" class="button back-to-top" type="button" aria-label="{back_to_top}" title="{back_to_top}"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 12 6-6 6 6M12 6v12"/></svg></button>
<script src="/public.js?v=1" defer></script><script src="/reads.js?v=4" defer></script><script src="/stats.js?v=2" data-track-url="{track_url}" defer></script></body></html>"##,
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
        home = localized("/", en),
        topics_label = choose(en, "主题", "Topics"),
        workspace = choose(en, "工作台", "Workspace"),
        other_lang = escape(&localized(path, !en)),
        other_code = choose(en, "en", "zh-CN"),
        language_label = choose(en, "EN", "中文"),
        back_to_top = choose(en, "回到顶部", "Back to top"),
        track_url = super::stats::TRACK_URL,
    ))
}

fn render_markdown(value: &str) -> String {
    render_events(Parser::new_ext(value, super::full_text::OPTIONS), &[], None)
}

fn render_events<'a>(
    events: impl IntoIterator<Item = Event<'a>>,
    images: &[super::full_text::Image],
    storage: Option<&url::Url>,
) -> String {
    let mut image = None;
    let mut alt = String::new();
    let parser = events.into_iter().filter_map(|event| match event {
        Event::Html(_)
        | Event::InlineHtml(_) => None,
        Event::Start(Tag::Image { dest_url, .. }) => {
            alt.clear();
            image = Some(dest_url.strip_prefix("reads-image:")
                .and_then(|id| id.parse::<usize>().ok()).and_then(|index| images.get(index))
                .and_then(|image| image.stored.as_ref()).filter(|stored| stored.expires_at > Utc::now())
                .and_then(|stored| url::Url::parse(&stored.direct_link).ok())
                .filter(|url| crate::media::is_public_https_url(url) && storage.is_some_and(|base| base.origin() == url.origin())));
            None
        }
        Event::End(TagEnd::Image) => {
            let url = image.take().flatten();
            Some(if let Some(url) = url {
                Event::Html(format!("<img src=\"{}\" alt=\"{}\" loading=lazy decoding=async referrerpolicy=no-referrer>", escape(url.as_str()), escape(&alt)).into())
            } else { Event::Text(std::mem::take(&mut alt).into()) })
        }
        Event::Text(text) | Event::Code(text) if image.is_some() => { alt.push_str(&text); None }
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

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn robots(State(state): State<AppState>, headers: HeaderMap) -> Response {
    conditional(
        &headers,
        &Rendered::new(format!(
            "User-agent: *\nAllow: /\nDisallow: /api/\nDisallow: /me/\nSitemap: {}\n",
            absolute(&state, "/sitemap.xml")
        )),
        "text/plain; charset=utf-8",
    )
}

async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let count: i64 = sqlx::QueryBuilder::new("SELECT COUNT(*) ")
        .push(VISIBLE)
        .build_query_scalar()
        .fetch_one(&state.pool)
        .await?;
    let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
    for page in 1..=((count + SITEMAP_SIZE - 1) / SITEMAP_SIZE).max(1) {
        let _ = write!(
            xml,
            "<sitemap><loc>{}</loc></sitemap>",
            escape(&absolute(&state, &format!("/reads/sitemap/{page}")))
        );
    }
    xml.push_str("</sitemapindex>");
    Ok(conditional(
        &headers,
        &Rendered::new(xml),
        "application/xml; charset=utf-8",
    ))
}

async fn sitemap_page(
    State(state): State<AppState>,
    Path(page): Path<u32>,
    headers: HeaderMap,
) -> Result<Response> {
    if page == 0 || page > 100000 {
        return Err(AppError::NotFound("页面不存在".into()));
    }
    let rows: Vec<(Uuid, DateTime<Utc>)> = sqlx::QueryBuilder::new("SELECT i.id, i.first_seen_at ")
        .push(VISIBLE)
        .push(" ORDER BY i.first_seen_at, i.id LIMIT ")
        .push_bind(SITEMAP_SIZE)
        .push(" OFFSET ")
        .push_bind(i64::from(page - 1) * SITEMAP_SIZE)
        .build_query_as()
        .fetch_all(&state.pool)
        .await?;
    if page > 1 && rows.is_empty() {
        return Err(AppError::NotFound("页面不存在".into()));
    }
    let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
    if page == 1 {
        for path in ["/", "/reads", "/reads?lang=en"] {
            let _ = write!(
                xml,
                "<url><loc>{}</loc></url>",
                escape(&absolute(&state, path))
            );
        }
    }
    for (id, time) in rows {
        for en in [false, true] {
            let _ = write!(
                xml,
                "<url><loc>{}</loc><lastmod>{}</lastmod></url>",
                escape(&absolute(&state, &localized(&format!("/reads/{id}"), en))),
                time.to_rfc3339()
            );
        }
    }
    xml.push_str("</urlset>");
    Ok(conditional(
        &headers,
        &Rendered::new(xml),
        "application/xml; charset=utf-8",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_text_loads_only_unexpired_images_from_configured_storage() {
        let mut images = vec![super::super::full_text::Image {
            origin: "https://original.example/diagram.png".into(),
            stored: Some(crate::media::UploadResponse {
                direct_link: "https://media.example.test/diagram.png".into(),
                expires_at: Utc::now() + chrono::Duration::days(7),
            }),
        }];
        let base = url::Url::parse("https://media.example.test").unwrap();
        let render = |images: &[_]| {
            render_events(
                Parser::new(
                    "![A diagram](reads-image:0) ![tracking](https://tracker.example/pixel)",
                ),
                images,
                Some(&base),
            )
        };
        let html = render(&images);
        assert_eq!(html.matches("<img ").count(), 1);
        assert!(html.contains("loading=lazy decoding=async"));
        assert!(!html.contains("original.example"));
        assert!(!html.contains("tracker.example"));
        images[0].stored.as_mut().unwrap().direct_link =
            "https://untrusted.example/diagram.png".into();
        assert!(!render(&images).contains("<img "));
        images[0].stored.as_mut().unwrap().direct_link =
            "https://media.example.test/diagram.png".into();
        images[0].stored.as_mut().unwrap().expires_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(!render(&images).contains("<img "));
    }

    #[test]
    fn markdown_cannot_execute_scripts_or_load_tracking_images() {
        let html = render_markdown(
            "## Useful\n\n<script>alert(1)</script>\n\n[bad](javascript:alert%281%29) ![alt](https://tracker.test/pixel) <img src=x onerror=alert(1)>\n\n[good](https://example.com/post)",
        );
        assert!(html.contains("<h2>Useful</h2>"));
        assert!(html.contains("href=\"https://example.com/post\""));
        for bad in ["<script", "<img", "javascript:", "onerror", "tracker.test"] {
            assert!(!html.contains(bad), "{html}");
        }
    }
}
