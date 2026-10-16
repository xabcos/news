//! The 分享 pages: public articles newest first, one author's articles, an article, and the
//! previews its author and an administrator read before readers do.
use super::{MARKDOWN, image_id, label, missing, parse};
use crate::{
    auth::User,
    digest::{Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{Chrome, EYE, Picture, absolute, choose, escape, localized, markdown},
    state::AppState,
};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use pulldown_cmark::Parser;
use serde::Deserialize;
use serde_json::json;
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::{collections::HashMap, fmt::Write};
use uuid::Uuid;

const PAGE_SIZE: i64 = 24;
/// The most addresses one sitemap may list.
const SITEMAP_SIZE: i64 = 50_000;
/// Articles of authors whose accounts are open; the caller names which of them.
const FROM: &str = " FROM articles a JOIN authors w ON w.user_id = a.author_id JOIN users u ON u.id = a.author_id WHERE u.status";
/// Articles read like Reads: the same list, endless scrolling and article layout.
const LIST: Chrome<'static> = Chrome {
    current: "articles",
    section: "reads articles",
    stylesheet: "/reads.css?v=12",
    script: "/reads.js?v=8",
    feed: ("News", "/feed.xml"),
};
/// An article counts its reads, and its review takes a ❤️.
const ARTICLE: Chrome<'static> = Chrome {
    script: "/articles.js?v=2",
    ..LIST
};

pub(super) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route("/articles", get(index))
            .route("/articles/{id}", get(detail)),
    )
    .route("/articles/sitemap.xml", get(sitemap))
    .route("/api/articles/{id}/preview", get(preview))
    .layer(tower_http::compression::CompressionLayer::new())
}

/// The list and the public articles, at their Chinese addresses: an article is written in one
/// language, and its English page only translates the site around it.
async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let rows: Vec<(Uuid, DateTime<Utc>)> =
        QueryBuilder::<Postgres>::new("SELECT a.id, a.updated_at")
            .push(FROM)
            .push(" AND a.status = 'published' ORDER BY a.published_at, a.id LIMIT ")
            .push_bind(SITEMAP_SIZE)
            .build_query_as()
            .fetch_all(&state.pool)
            .await?;
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"><url><loc>{}</loc></url>",
        escape(&absolute(&state, "/articles"))
    );
    for (id, updated_at) in rows {
        let _ = write!(
            xml,
            "<url><loc>{}</loc><lastmod>{}</lastmod></url>",
            escape(&absolute(&state, &format!("/articles/{id}"))),
            updated_at.to_rfc3339()
        );
    }
    xml.push_str("</urlset>");
    Ok(conditional(
        &headers,
        &Rendered::new(xml),
        "application/xml; charset=utf-8",
    ))
}

#[derive(Deserialize)]
struct Params {
    lang: Option<String>,
    page: Option<u32>,
    by: Option<i64>,
}

impl Params {
    fn english(&self) -> bool {
        self.lang.as_deref() == Some("en")
    }
}

#[derive(FromRow)]
struct Card {
    id: Uuid,
    author_id: i64,
    author: String,
    title: String,
    excerpt: String,
    views: i64,
    published_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct Article {
    id: Uuid,
    author_id: i64,
    author: String,
    title: String,
    body: String,
    excerpt: String,
    cover: Option<Uuid>,
    status: String,
    revision: i32,
    views: i64,
    updated_at: DateTime<Utc>,
    published_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct Image {
    id: Uuid,
    url: String,
    width: i32,
    height: i32,
    expires_at: DateTime<Utc>,
}

/// Who reads the page, which decides what it shows and where its pictures load from.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Readers: published articles, pictures from tmpfile.
    Public,
    /// An administrator's preview, with the ❤️.
    Review,
    /// The author's own preview, whatever the status; pictures come through this server, so
    /// their tmpfile links stay private until the article is public.
    Author(i64),
}

fn listing(page: u32, en: bool, by: Option<i64>) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(by) = by {
        query.append_pair("by", &by.to_string());
    }
    if page > 1 {
        query.append_pair("page", &page.to_string());
    }
    let query = query.finish();
    localized(
        &if query.is_empty() {
            "/articles".into()
        } else {
            format!("/articles?{query}")
        },
        en,
    )
}

async fn index(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.english();
    let page = match params.page.unwrap_or(1) {
        page @ 1..=100000 => page,
        _ => return Err(AppError::NotFound("页面不存在".into())),
    };
    let author = match params.by {
        Some(id) => Some(
            sqlx::query_scalar::<_, String>(
                "SELECT w.name FROM authors w JOIN users u ON u.id = w.user_id WHERE w.user_id = $1 AND u.status",
            )
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| AppError::NotFound("作者不存在".into()))?,
        ),
        None => None,
    };
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT a.id, a.author_id, w.name AS author, a.title, a.excerpt, a.views, a.published_at",
    );
    query.push(FROM).push(" AND a.status = 'published'");
    if let Some(by) = params.by {
        query.push(" AND a.author_id = ").push_bind(by);
    }
    query
        .push(" ORDER BY a.published_at DESC, a.id DESC LIMIT ")
        .push_bind(PAGE_SIZE + 1)
        .push(" OFFSET ")
        .push_bind(i64::from(page - 1) * PAGE_SIZE);
    let mut rows: Vec<Card> = query.build_query_as().fetch_all(&state.pool).await?;
    if page > 1 && rows.is_empty() {
        return Err(AppError::NotFound("页面不存在".into()));
    }
    let more = rows.len() > PAGE_SIZE as usize;
    rows.truncate(PAGE_SIZE as usize);
    let description = match &author {
        Some(name) => choose(
            en,
            &format!("{name} 的文章"),
            &format!("Articles by {name}"),
        )
        .to_owned(),
        None => choose(
            en,
            "读者分享的有趣好玩的东西",
            "Fun and interesting things our readers share",
        )
        .to_owned(),
    };
    let mut content = format!(
        "<header class=reads-heading><h1>{}</h1><p>{}</p><div class=reads-tools><a class=article-write href=\"/me/\">{}</a></div></header><div class=reads-list>",
        label(en),
        escape(&description),
        choose(en, "写文章", "Write")
    );
    for row in &rows {
        content.push_str(&card(row, en, state.config.timezone));
    }
    if rows.is_empty() && author.is_some() {
        content.push_str(choose(
            en,
            "<p class=muted>还没有公开的文章。</p>",
            "<p class=muted>No public articles yet.</p>",
        ));
    } else if rows.is_empty() {
        content.push_str(choose(
            en,
            "<p class=muted>还没有公开的文章，<a href=\"/me/\">来写第一篇</a>。</p>",
            "<p class=muted>No articles yet. <a href=\"/me/\">Write the first one</a>.</p>",
        ));
    }
    content.push_str("</div><nav class=reads-pagination aria-label=Pagination>");
    if page > 1 {
        let _ = write!(
            content,
            "<a rel=prev href=\"{}\">{}</a>",
            escape(&listing(page - 1, en, params.by)),
            choose(en, "上一页", "Newer")
        );
    }
    if more {
        let _ = write!(
            content,
            "<a rel=next href=\"{}\">{}</a>",
            escape(&listing(page + 1, en, params.by)),
            choose(en, "继续阅读", "Older articles")
        );
    }
    content.push_str("</nav>");
    let title = match (&author, page) {
        (Some(name), _) => format!("{name} · {}", label(en)),
        (None, 1) => format!("{} · 𝑵𝒆𝒘𝒔", label(en)),
        (None, page) => format!("{} · {page} · 𝑵𝒆𝒘𝒔", label(en)),
    };
    let html = crate::site::layout(
        &state,
        reader,
        en,
        &listing(page, false, params.by),
        &title,
        &description,
        &content,
        None,
        &LIST,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

fn card(item: &Card, en: bool, timezone: chrono_tz::Tz) -> String {
    let views = if item.views > 0 {
        format!(
            " · <span class=read-views title=\"{}\">{EYE}<span>{}</span></span>",
            choose(en, "阅读次数", "Reads"),
            item.views
        )
    } else {
        String::new()
    };
    format!(
        "<article class=item><div class=read-card><div class=item-meta><a href=\"{}\">{}</a> · <span class=read-date><time datetime=\"{}\">{}</time>{views}</span></div><h2 class=item-title><a href=\"{}\">{}</a></h2><p class=item-summary>{}</p></div></article>",
        escape(&localized(&format!("/articles?by={}", item.author_id), en)),
        escape(&item.author),
        item.published_at.to_rfc3339(),
        item.published_at
            .with_timezone(&timezone)
            .format("%Y-%m-%d"),
        localized(&format!("/articles/{}", item.id), en),
        escape(&item.title),
        escape(&item.excerpt)
    )
}

/// An article as readers see it; an administrator also reads what waits for a ❤️.
async fn detail(
    State(state): State<AppState>,
    reader: Reader,
    Path(id): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let mode = if reader.is_admin() {
        Mode::Review
    } else {
        Mode::Public
    };
    let html = render(&state, reader, parse(&id)?, params.english(), mode).await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// The author's preview of their own article, as it would read.
async fn preview(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    Query(params): Query<Params>,
) -> Result<Response> {
    let html = render(
        &state,
        Reader::Public,
        parse(&id)?,
        params.english(),
        Mode::Author(user.id),
    )
    .await?;
    let mut response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

async fn render(
    state: &AppState,
    reader: Reader,
    id: Uuid,
    en: bool,
    mode: Mode,
) -> Result<String> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT a.id, a.author_id, w.name AS author, a.title, a.body, a.excerpt, a.cover, a.status, a.revision, a.views, a.updated_at, a.published_at",
    );
    query.push(FROM).push(" AND a.id = ").push_bind(id);
    match mode {
        Mode::Public => query.push(" AND a.status = 'published'"),
        Mode::Review => query.push(" AND a.status IN ('pending', 'published')"),
        Mode::Author(author) => query.push(" AND a.author_id = ").push_bind(author),
    };
    let article: Article = query
        .build_query_as()
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(missing)?;
    let images: HashMap<Uuid, Image> = sqlx::query_as::<_, Image>(
        "SELECT id, url, width, height, expires_at FROM article_images WHERE article_id = $1",
    )
    .bind(article.id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|image| (image.id, image))
    .collect();
    let now = Utc::now();
    let picture = |id: Uuid| {
        let image = images.get(&id)?;
        let src = match mode {
            Mode::Author(_) => format!("/api/articles/{}/images/{id}", article.id),
            Mode::Public | Mode::Review => (image.expires_at > now).then(|| image.url.clone())?,
        };
        Some(Picture {
            src,
            size: Some((image.width, image.height)),
        })
    };
    let body = markdown(Parser::new_ext(&article.body, MARKDOWN), |destination| {
        image_id(destination).and_then(picture)
    });
    let cover = article.cover.and_then(picture);
    let title = if article.title.is_empty() {
        choose(en, "无标题", "Untitled")
    } else {
        &article.title
    };
    let date = article.published_at.unwrap_or(article.updated_at);
    // Readers' copy of the page counts their read; previews only show the number.
    let views = format!(
        "<span class=read-views{} title=\"{}\">{EYE}<span>{}</span></span>",
        if mode == Mode::Public {
            format!(" data-article-views=\"{}\"", article.id)
        } else {
            String::new()
        },
        choose(en, "阅读次数", "Reads"),
        article.views
    );
    let status = match article.status.as_str() {
        "published" => choose(en, "已公开", "Public"),
        "pending" => choose(en, "待审", "In review"),
        _ => choose(en, "草稿", "Draft"),
    };
    let notice = match mode {
        Mode::Public => String::new(),
        Mode::Review => {
            let published = article.status == "published";
            format!(
                "<div class=article-notice><span>{status}</span><button class=\"button button-small\" type=button data-article-review=\"{}\" data-revision=\"{}\" data-published=\"{published}\">{}</button></div>",
                article.id,
                article.revision,
                if published {
                    choose(en, "取消 ❤️", "Take back ❤️")
                } else {
                    choose(en, "❤️ 公开", "❤️ Publish")
                }
            )
        }
        Mode::Author(_) => format!(
            "<div class=article-notice><span>{} · {status}</span><a href=\"/me/write.html?id={}\">{}</a></div>",
            choose(en, "预览", "Preview"),
            article.id,
            choose(en, "继续编辑", "Edit")
        ),
    };
    let by = localized(&format!("/articles?by={}", article.author_id), en);
    let mut content = format!(
        "{notice}<article class=\"read article\">{}<header class=read-heading><div class=read-title><h1>{}</h1></div><div class=item-meta><a href=\"{}\">{}</a> · <time datetime=\"{}\">{}</time> · {views}</div></header><div class=read-body>{body}</div></article>",
        cover
            .as_ref()
            .map(|cover| {
                let (width, height) = cover.size.unwrap_or_default();
                format!(
                    "<img class=article-cover src=\"{}\" width={width} height={height} alt=\"\" decoding=async referrerpolicy=no-referrer>",
                    escape(&cover.src)
                )
            })
            .unwrap_or_default(),
        crate::site::headline(title),
        escape(&by),
        escape(&article.author),
        date.to_rfc3339(),
        date.with_timezone(&state.config.timezone).format("%Y-%m-%d"),
    );
    let _ = write!(
        content,
        "<div class=read-actions><a href=\"{}\">← {}</a></div>",
        localized("/articles", en),
        label(en)
    );
    let path = match mode {
        Mode::Author(_) => format!("/api/articles/{}/preview", article.id),
        Mode::Public | Mode::Review => format!("/articles/{}", article.id),
    };
    let canonical = absolute(state, &localized(&path, en));
    let mut structured = json!({
        "@context": "https://schema.org", "@type": "Article", "headline": title,
        "description": article.excerpt, "url": canonical, "mainEntityOfPage": canonical,
        "datePublished": date.to_rfc3339(), "dateModified": article.updated_at.to_rfc3339(),
        "author": {"@type": "Person", "name": article.author, "url": absolute(state, &by)},
        "publisher": {"@type": "Organization", "name": "News", "url": state.config.public_base_url},
    });
    // Only a public page names its tmpfile picture for sharing.
    if mode == Mode::Public
        && let Some(cover) = &cover
    {
        structured["image"] = json!(cover.src);
    }
    Ok(crate::site::layout(
        state,
        reader,
        en,
        &path,
        &format!("{title} · {}", label(en)),
        &article.excerpt,
        &content,
        Some(structured),
        &ARTICLE,
    ))
}
