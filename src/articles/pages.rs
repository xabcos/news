//! The 分享 pages: public articles newest first, one author's articles, an article, and the
//! previews its author and an administrator read before readers do. 解说 and the lessons of 学习
//! read on the same article page.
use super::{
    COMMENTARY, MARKDOWN, Search, commentary_label, image_id, label,
    learn::{self, Place},
    missing, parse,
};
use crate::{
    auth::User,
    digest::{Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{Chrome, EYE, Picture, absolute, choose, escape, localized, markdown, search_box},
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
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::{collections::HashMap, fmt::Write};
use uuid::Uuid;

const PAGE_SIZE: i64 = 24;
/// How 热读 and 值得读 rank articles: by reads, halving with each week since publication.
const HOT: &str = "ln(1 + a.views::float8) * power(2, -extract(epoch FROM NOW() - a.published_at)::float8 / 604800)";
/// The most addresses one sitemap may list.
const SITEMAP_SIZE: i64 = 50_000;
/// Articles of authors whose accounts are open; the caller names which of them.
const FROM: &str = " FROM articles a JOIN authors w ON w.user_id = a.author_id JOIN users u ON u.id = a.author_id WHERE u.status";
/// Articles read like Reads: the same endless scrolling and article layout, but the list tiles
/// picture cards across the window as 解说's does.
const LIST: Chrome<'static> = Chrome {
    current: "articles",
    section: "reads articles article-cards",
    stylesheet: "/reads.css?v=18",
    script: "/reads.js?v=9",
    feed: ("News", "/feed.xml"),
};
/// An article counts its reads, and its review takes a ❤️.
const ARTICLE: Chrome<'static> = Chrome {
    section: "reads articles",
    script: "/articles.js?v=5",
    ..LIST
};
/// 文娱 · 解说 belongs to Culture, whose tab it marks, and its cards tile like Culture's while
/// they load and count like the lists here.
const EXPLAINED: Chrome<'static> = Chrome {
    current: "movie",
    section: "catalog culture catalog-commentary",
    stylesheet: "/catalog.css?v=9",
    ..LIST
};
const EXPLANATION: Chrome<'static> = Chrome {
    current: "movie",
    ..ARTICLE
};
/// A lesson marks 学习's tab and has its course's contents beside it.
const LESSON: Chrome<'static> = Chrome {
    current: "learn",
    section: "reads articles learn",
    ..ARTICLE
};

pub(super) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route("/articles", get(index))
            .route("/articles/{id}", get(detail))
            .route(COMMENTARY, get(commentary)),
    )
    .route("/articles/sitemap.xml", get(sitemap))
    .route("/articles/worth.json", get(worth))
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
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"><url><loc>{}</loc></url><url><loc>{}</loc></url><url><loc>{}</loc></url>",
        escape(&absolute(&state, "/articles")),
        escape(&absolute(&state, COMMENTARY)),
        escape(&absolute(&state, learn::PATH))
    );
    for course in learn::courses(&state).await? {
        let _ = write!(
            xml,
            "<url><loc>{}</loc></url>",
            escape(&absolute(&state, &format!("{}/{}", learn::PATH, course.id)))
        );
    }
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
    view: Option<String>,
    q: Option<String>,
}

impl Params {
    fn english(&self) -> bool {
        self.lang.as_deref() == Some("en")
    }
}

/// A public article on 分享's or 解说's list: the cover while tmpfile keeps it, and its author,
/// title, date, reads and opening.
#[derive(FromRow)]
struct Card {
    id: Uuid,
    author_id: i64,
    author: String,
    title: String,
    excerpt: String,
    views: i64,
    published_at: DateTime<Utc>,
    cover: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

const CARD: &str =
    "SELECT a.id, a.author_id, w.name AS author, a.title, a.excerpt, a.views, a.published_at,
        CASE WHEN i.expires_at > now() THEN i.url END AS cover, i.width, i.height";

/// The public articles of 分享's list, or with `film` of 解说's, holding the words searched;
/// `select` names what to take of them. Lessons have their contents under 学习.
fn listed(select: &str, film: bool, by: Option<i64>, search: &Search) -> QueryBuilder<Postgres> {
    let mut query = QueryBuilder::new(select);
    query
        .push(
            " FROM articles a JOIN authors w ON w.user_id = a.author_id JOIN users u ON u.id = a.author_id
              LEFT JOIN article_images i ON i.id = a.cover
              WHERE u.status AND a.status = 'published' AND ",
        )
        .push(if film {
            "a.film IS NOT NULL"
        } else {
            "a.film IS NULL AND a.course IS NULL"
        });
    if let Some(by) = by {
        query.push(" AND a.author_id = ").push_bind(by);
    }
    search.narrow(&mut query);
    query
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
    /// Whether it is a 解说, which belongs to Culture.
    film: bool,
    /// A lesson's track, chapter and place.
    course: Option<String>,
    chapter: Option<String>,
    lesson: Option<i32>,
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
    /// their tmpfile links stay private until the article is public. An administrator, who
    /// edits anyone's article, previews anyone's (`None`).
    Author(Option<i64>),
}

/// The address of a list's page, one author's with `by`, a search's with `words`.
fn listing(path: &str, page: u32, en: bool, by: Option<i64>, words: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(by) = by {
        query.append_pair("by", &by.to_string());
    }
    if !words.is_empty() {
        query.append_pair("q", words);
    }
    if page > 1 {
        query.append_pair("page", &page.to_string());
    }
    let query = query.finish();
    localized(
        &if query.is_empty() {
            path.into()
        } else {
            format!("{path}?{query}")
        },
        en,
    )
}

/// Links to a list's neighbouring pages; the page script follows the next one as readers scroll.
fn pagination(path: &str, page: u32, more: bool, en: bool, by: Option<i64>, words: &str) -> String {
    let mut nav = "<nav class=reads-pagination aria-label=Pagination>".to_owned();
    if page > 1 {
        let _ = write!(
            nav,
            "<a rel=prev href=\"{}\">{}</a>",
            escape(&listing(path, page - 1, en, by, words)),
            choose(en, "上一页", "Newer")
        );
    }
    if more {
        let _ = write!(
            nav,
            "<a rel=next href=\"{}\">{}</a>",
            escape(&listing(path, page + 1, en, by, words)),
            choose(en, "继续阅读", "Older articles")
        );
    }
    nav.push_str("</nav>");
    nav
}

/// The page of a list asked for.
fn page(params: &Params) -> Result<u32> {
    match params.page.unwrap_or(1) {
        page @ 1..=100000 => Ok(page),
        _ => Err(AppError::NotFound("页面不存在".into())),
    }
}

async fn index(
    State(state): State<AppState>,
    // The page is the same for everyone, but a preview of it still needs an administrator.
    _: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.english();
    let page = page(&params)?;
    let search = Search::new(params.q.as_deref());
    // 热读 ranks the section's last 30 days as Reads ranks its own: reads, fading by half a week.
    let hot = params.view.as_deref() == Some("hot") && params.by.is_none() && search.is_empty();
    if hot && page != 1 {
        return Err(AppError::NotFound("页面不存在".into()));
    }
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
    // 解说 has a list of its own.
    let mut query = listed(CARD, false, params.by, &search);
    if hot {
        query
            .push(" AND a.views > 0 AND a.published_at >= NOW() - INTERVAL '30 days' ORDER BY ")
            .push(HOT)
            .push(" DESC, a.published_at DESC, a.id DESC LIMIT 24");
    } else {
        search.order(&mut query);
        query
            .push(" LIMIT ")
            .push_bind(PAGE_SIZE + 1)
            .push(" OFFSET ")
            .push_bind(i64::from(page - 1) * PAGE_SIZE);
    }
    let mut rows: Vec<Card> = query.build_query_as().fetch_all(&state.pool).await?;
    if page > 1 && rows.is_empty() {
        return Err(AppError::NotFound("页面不存在".into()));
    }
    let more = !hot && rows.len() > PAGE_SIZE as usize;
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
    let found = found(
        &search,
        page,
        en,
        listed("SELECT count(*)", false, params.by, &search).build_query_scalar(),
        &state,
    )
    .await?;
    // One author's page keeps to their articles in order.
    let views = if author.is_none() {
        format!(
            "<nav class=reads-views aria-label=\"{}\"><a href=\"{}\"{}>{}</a><a href=\"{}\"{}>{}</a></nav>",
            choose(en, "阅读排序", "Reading order"),
            localized("/articles", en),
            if hot || !search.is_empty() {
                ""
            } else {
                " aria-current=\"page\""
            },
            choose(en, "最新", "Latest"),
            escape(&localized("/articles?view=hot", en)),
            if hot { " aria-current=\"page\"" } else { "" },
            choose(en, "热读", "Popular"),
        )
    } else {
        String::new()
    };
    let mut content = format!(
        "<header class=reads-heading><h1>{}</h1><p>{}</p><div class=reads-tools>{}<a class=article-write href=\"/me/\">{}</a>{views}</div></header><div class=reads-list>",
        label(en),
        found.unwrap_or_else(|| escape(&description)),
        search_box(
            "reads-search",
            "/articles",
            &search.words,
            choose(en, "搜索分享的标题和内容", "Search titles and openings"),
            en
        ),
        choose(en, "写文章", "Write")
    );
    for (index, row) in rows.iter().enumerate() {
        content.push_str(&card(
            row,
            en,
            hot.then_some(index + 1),
            true,
            state.config.timezone,
        ));
    }
    content.push_str("</div>");
    if rows.is_empty() && !search.is_empty() {
        content.push_str(choose(
            en,
            "<p class=muted>没有找到相关文章，换个词试试，或看看<a href=\"/articles\">最新文章</a>。</p>",
            "<p class=muted>No articles match these words. Try others, or explore the <a href=\"/articles?lang=en\">latest ones</a>.</p>",
        ));
    } else if rows.is_empty() && hot {
        content.push_str(choose(
            en,
            "<p class=muted>热读正在积累，先看看<a href=\"/articles\">最新文章</a>。</p>",
            "<p class=muted>Popular articles are taking shape. Explore the <a href=\"/articles?lang=en\">latest ones</a>.</p>",
        ));
    } else if rows.is_empty() && author.is_some() {
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
    content.push_str(&pagination(
        "/articles",
        page,
        more,
        en,
        params.by,
        &search.words,
    ));
    let title = match (&author, page) {
        (Some(name), _) => format!("{name} · {}", label(en)),
        (None, _) if hot => format!("{} · {}", choose(en, "热读", "Popular"), label(en)),
        (None, 1) => format!("{} · 𝑵𝒆𝒘𝒔", label(en)),
        (None, page) => format!("{} · {page} · 𝑵𝒆𝒘𝒔", label(en)),
    };
    let path = if hot {
        "/articles?view=hot".to_owned()
    } else {
        listing("/articles", page, false, params.by, &search.words)
    };
    let html = crate::site::layout(
        &state,
        en,
        &path,
        &title,
        &description,
        &content,
        None,
        &LIST,
    );
    Ok(listed_page(&headers, html, &search))
}

/// What a search found, for the line under a list's title: on its first page how many articles
/// hold the words, on the pages scrolling loads only the words.
async fn found(
    search: &Search,
    page: u32,
    en: bool,
    count: sqlx::query::QueryScalar<'_, Postgres, i64, sqlx::postgres::PgArguments>,
    state: &AppState,
) -> Result<Option<String>> {
    if search.is_empty() {
        return Ok(None);
    }
    let words = escape(&search.words);
    Ok(Some(if page > 1 {
        choose(
            en,
            &format!("「{words}」的搜索结果"),
            &format!("Results for “{words}”"),
        )
        .to_owned()
    } else {
        let found = count.fetch_one(&state.pool).await?;
        choose(
            en,
            &format!("「{words}」找到 {found} 篇"),
            &format!("Results for “{words}”: {found}"),
        )
        .to_owned()
    }))
}

/// A list page as cached; pages for words stay out of search engines, as Reads' do.
fn listed_page(headers: &HeaderMap, html: String, search: &Search) -> Response {
    let mut response = conditional(headers, &Rendered::new(html), "text/html; charset=utf-8");
    if !search.is_empty() {
        response.headers_mut().insert(
            header::HeaderName::from_static("x-robots-tag"),
            HeaderValue::from_static("noindex"),
        );
    }
    response
}

/// One of 值得读's articles, as the home page script lays it out.
#[derive(FromRow, Serialize)]
struct Worth {
    id: Uuid,
    title: String,
    views: i64,
    film: bool,
    cover: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

/// 值得读, which opens ALL: XABC's four newest 分享, newest first, then the twelve hottest of the
/// last 30 days of 分享 and 解说 together, ranked as 热读 is, each article once; while few are
/// read, the newest fill the places. Lessons stay under 学习.
async fn worth(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let articles: Vec<Worth> = QueryBuilder::<Postgres>::new(
        "WITH newest AS (
             SELECT a.id, a.published_at FROM articles a JOIN users u ON u.id = a.author_id
             WHERE u.email = ",
    )
    .push_bind(super::xabc::EMAIL)
    .push(
        " AND a.status = 'published' AND a.film IS NULL AND a.course IS NULL
             ORDER BY a.published_at DESC, a.id DESC LIMIT 4)
         SELECT a.id, a.title, a.views, a.film IS NOT NULL AS film,
                CASE WHEN i.expires_at > now() THEN i.url END AS cover, i.width, i.height
         FROM articles a JOIN users u ON u.id = a.author_id
         LEFT JOIN article_images i ON i.id = a.cover
         LEFT JOIN newest n ON n.id = a.id
         WHERE u.status AND a.status = 'published' AND a.course IS NULL
           AND a.published_at >= NOW() - INTERVAL '30 days'
         ORDER BY n.published_at DESC NULLS LAST, ",
    )
    .push(HOT)
    .push(" DESC, a.published_at DESC, a.id DESC LIMIT 16")
    .build_query_as()
    .fetch_all(&state.pool)
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(json!({ "articles": articles }).to_string()),
        "application/json",
    ))
}

/// 文娱 · 解说: XABC's articles on films and series, newest first, as picture cards under
/// Culture's tabs.
async fn commentary(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.english();
    let page = page(&params)?;
    let search = Search::new(params.q.as_deref());
    let mut query = listed(CARD, true, None, &search);
    search.order(&mut query);
    let mut rows: Vec<Card> = query
        .push(" LIMIT ")
        .push_bind(PAGE_SIZE + 1)
        .push(" OFFSET ")
        .push_bind(i64::from(page - 1) * PAGE_SIZE)
        .build_query_as()
        .fetch_all(&state.pool)
        .await?;
    if page > 1 && rows.is_empty() {
        return Err(AppError::NotFound("页面不存在".into()));
    }
    let more = rows.len() > PAGE_SIZE as usize;
    rows.truncate(PAGE_SIZE as usize);
    let found = found(
        &search,
        page,
        en,
        listed("SELECT count(*)", true, None, &search).build_query_scalar(),
        &state,
    )
    .await?;
    let mut content = crate::catalog::commentary_tabs(&state, reader, en);
    content.push_str(&search_box(
        "catalog-search",
        COMMENTARY,
        &search.words,
        choose(en, "搜索解说的片名和内容", "Search titles and openings"),
        en,
    ));
    if let Some(found) = found {
        let _ = write!(content, "<p class=\"commentary-found muted\">{found}</p>");
    }
    content.push_str("<div class=\"reads-list catalog-grid\">");
    for row in &rows {
        content.push_str(&card(row, en, None, false, state.config.timezone));
    }
    content.push_str("</div>");
    if rows.is_empty() {
        let _ = write!(
            content,
            "<p class=empty>{}</p>",
            if search.is_empty() {
                choose(en, "第一篇解说正在写。", "The first one is being written.")
            } else {
                choose(
                    en,
                    "没有找到相关解说，换个词试试。",
                    "Nothing explained matches these words. Try others.",
                )
            }
        );
    }
    content.push_str(&pagination(COMMENTARY, page, more, en, None, &search.words));
    let label = commentary_label(en);
    let title = match page {
        1 => format!("{label} · {}", crate::catalog::Section::Culture.label(en)),
        page => format!(
            "{label} · {page} · {}",
            crate::catalog::Section::Culture.label(en)
        ),
    };
    let html = crate::site::layout(
        &state,
        en,
        &listing(COMMENTARY, page, false, None, &search.words),
        &title,
        choose(
            en,
            "好电影、好剧的完整解说：剧情、结局与幕后",
            "Good films and series explained: story, ending and the making",
        ),
        &content,
        None,
        &EXPLAINED,
    );
    Ok(listed_page(&headers, html, &search))
}

/// A card's reads, whose place stays hidden while nobody has read it: the page script brings
/// the count up to date, so a cached list still shows what is read now.
fn reads(id: Uuid, views: i64, en: bool) -> String {
    format!(
        "<span data-views=\"{id}\"{}> · <span class=read-views title=\"{}\">{EYE}<span>{views}</span></span></span>",
        if views > 0 { "" } else { " hidden" },
        choose(en, "阅读次数", "Reads"),
    )
}

/// An article's card: its cover, then 热读's place, the author unless the list is all XABC's
/// (解说's), the date, the reads, the title and the opening.
fn card(
    item: &Card,
    en: bool,
    rank: Option<usize>,
    author: bool,
    timezone: chrono_tz::Tz,
) -> String {
    let href = localized(&format!("/articles/{}", item.id), en);
    let cover = item
        .cover
        .as_ref()
        .map(|cover| {
            format!(
                "<a class=image-news-media href=\"{href}\" tabindex=-1 aria-hidden=true><img src=\"{}\" width={} height={} alt=\"\" loading=lazy decoding=async referrerpolicy=no-referrer></a>",
                escape(cover),
                item.width.unwrap_or_default(),
                item.height.unwrap_or_default()
            )
        })
        .unwrap_or_default();
    let rank = rank
        .map(|rank| format!("<span class=read-rank aria-hidden=true>{rank:02}</span>"))
        .unwrap_or_default();
    let author = if author {
        format!(
            "<a href=\"{}\">{}</a> · ",
            escape(&localized(&format!("/articles?by={}", item.author_id), en)),
            escape(&item.author)
        )
    } else {
        String::new()
    };
    format!(
        "<article class=\"item image-news-card catalog-card\">{cover}<div class=catalog-card-body><p class=item-meta>{rank}{author}<time datetime=\"{}\">{}</time>{}</p><h2 class=item-title><a href=\"{href}\">{}</a></h2><p class=item-summary>{}</p></div></article>",
        item.published_at.to_rfc3339(),
        item.published_at
            .with_timezone(&timezone)
            .format("%Y-%m-%d"),
        reads(item.id, item.views, en),
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
    let html = render(&state, parse(&id)?, params.english(), mode).await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// The author's preview of their own article, or an administrator's of any, as it would read.
async fn preview(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    Query(params): Query<Params>,
) -> Result<Response> {
    let html = render(
        &state,
        parse(&id)?,
        params.english(),
        Mode::Author((!user.is_admin()).then_some(user.id)),
    )
    .await?;
    let mut response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    Ok(response)
}

async fn render(state: &AppState, id: Uuid, en: bool, mode: Mode) -> Result<String> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT a.id, a.author_id, w.name AS author, a.title, a.body, a.excerpt, a.cover, a.status, a.revision, a.views, a.updated_at, a.published_at, a.film IS NOT NULL AS film, a.course, a.chapter, a.lesson",
    );
    query.push(FROM).push(" AND a.id = ").push_bind(id);
    match mode {
        Mode::Public => query.push(" AND a.status = 'published'"),
        Mode::Review => query.push(" AND a.status IN ('pending', 'published')"),
        Mode::Author(Some(author)) => query.push(" AND a.author_id = ").push_bind(author),
        Mode::Author(None) => &mut query,
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
    // An administrator's previews lead to the editor and copy the title and body to post on X
    // Articles; the review preview also holds the ❤️.
    let edit = |label: &str| {
        format!(
            "<a class=\"button button-small button-secondary\" href=\"/me/write.html?id={}\">{label}</a>",
            article.id
        )
    };
    let copies = format!(
        "<button class=\"button button-small button-secondary\" type=button data-article-copy=title>{}</button><button class=\"button button-small button-secondary\" type=button data-article-copy=body>{}</button>",
        choose(en, "复制标题", "Copy title"),
        choose(en, "复制正文", "Copy body"),
    );
    let notice = match mode {
        Mode::Public => String::new(),
        Mode::Review => {
            let published = article.status == "published";
            format!(
                "<div class=article-notice><span class=article-status>{status}</span><span class=article-actions>{}{copies}<button class=\"button button-small button-primary\" type=button data-article-review=\"{}\" data-revision=\"{}\" data-published=\"{published}\">{}</button></span></div>",
                edit(choose(en, "编辑", "Edit")),
                article.id,
                article.revision,
                if published {
                    choose(en, "取消 ❤️", "Take back ❤️")
                } else {
                    choose(en, "❤️ 公开", "❤️ Publish")
                },
            )
        }
        Mode::Author(author) => format!(
            "<div class=article-notice><span class=article-status>{} · {status}</span><span class=article-actions>{}{}</span></div>",
            choose(en, "预览", "Preview"),
            edit(choose(en, "继续编辑", "Edit")),
            if author.is_none() {
                copies.as_str()
            } else {
                ""
            },
        ),
    };
    // A 解说 leads back to its list, whose articles are all XABC's; a lesson to its track.
    let (list, name, by) = if article.film {
        (COMMENTARY, commentary_label(en), localized(COMMENTARY, en))
    } else {
        (
            "/articles",
            label(en),
            localized(&format!("/articles?by={}", article.author_id), en),
        )
    };
    let place = match (&article.course, &article.chapter, article.lesson) {
        (Some(course), Some(chapter), Some(lesson)) => {
            Place::find(state, course, chapter, lesson).await?
        }
        _ => None,
    };
    let time = |at: DateTime<Utc>| {
        format!(
            "<time datetime=\"{}\">{}</time>",
            at.to_rfc3339(),
            at.with_timezone(&state.config.timezone).format("%Y-%m-%d")
        )
    };
    // A lesson tells its place and how current it is; other articles their author and date.
    let meta = match (&place, article.lesson) {
        (Some(_), Some(lesson)) => choose(
            en,
            &format!(
                "第 {lesson} 课 · 最后更新 {} · 第 {} 版",
                time(article.updated_at),
                article.revision
            ),
            &format!(
                "Lesson {lesson} · updated {} · version {}",
                time(article.updated_at),
                article.revision
            ),
        )
        .to_owned(),
        _ => format!(
            "<a href=\"{}\">{}</a> · {}",
            escape(&by),
            escape(&article.author),
            time(date)
        ),
    };
    let mut content = format!(
        "{notice}<article class=\"read article\">{}{}<header class=read-heading><div class=read-title><h1>{}</h1></div><div class=item-meta>{meta} · {views}</div></header><div class=read-body>{body}</div></article>",
        place
            .as_ref()
            .map(|place| place.crumbs(en))
            .unwrap_or_default(),
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
    );
    match &place {
        Some(place) => {
            content.push_str(&place.steps(en));
            content = learn::columns(&place.contents(en), &content);
        }
        None => {
            let _ = write!(
                content,
                "<div class=read-actions><a href=\"{}\">← {name}</a></div>",
                localized(list, en),
            );
        }
    }
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
    let (name, chrome) = match (&place, article.film) {
        (Some(_), _) => (learn::label(en), &LESSON),
        (None, true) => (name, &EXPLANATION),
        (None, false) => (name, &ARTICLE),
    };
    Ok(crate::site::layout(
        state,
        en,
        &path,
        &format!("{title} · {name}"),
        &article.excerpt,
        &content,
        Some(structured),
        chrome,
    ))
}
