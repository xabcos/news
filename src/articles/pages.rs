//! The 分享 pages: public articles newest first, one author's articles, an article, and the
//! previews its author and an administrator read before readers do. 调研, 解说, 时代 and 追踪
//! read on the same article page, and 调研's, 时代's and 追踪's lists are 分享's.
use super::{
    COMMENTARY, MARKDOWN, PEOPLE, REVIEWS, Search, TRACKER, commentary_label, image_id, label,
    missing, parse, people_label, research, reviews_label, tracker_label,
};
use crate::{
    auth::User,
    digest::{Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{
        Chrome, EYE, Picture, absolute, choose, escape, listing, localized, markdown, pagination,
        parsed, search_box,
    },
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
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{FromRow, Postgres, QueryBuilder};
use std::{collections::HashMap, fmt::Write};
use uuid::Uuid;

const PAGE_SIZE: i64 = 24;
/// How 热读 and 精选 rank articles: by reads, halving with each week since publication.
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
    stylesheet: "/reads.css?v=22",
    script: "/reads.js?v=12",
    feed: ("News", "/feed.xml"),
};
/// An article counts its reads, and its review takes a ❤️.
const ARTICLE: Chrome<'static> = Chrome {
    section: "reads articles",
    script: "/articles.js?v=5",
    ..LIST
};
/// 调研 has a tab of its own after 分享's.
const RESEARCH_LIST: Chrome<'static> = Chrome {
    current: "research",
    ..LIST
};
const RESEARCH: Chrome<'static> = Chrome {
    current: "research",
    ..ARTICLE
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
/// 时代 has a tab of its own, and its cards keep faces in frame.
const PEOPLE_LIST: Chrome<'static> = Chrome {
    current: "people",
    section: "reads articles article-cards people",
    ..LIST
};
const PROFILE: Chrome<'static> = Chrome {
    current: "people",
    ..ARTICLE
};
/// 追踪 has a tab of its own after 时代's.
const TRACKER_LIST: Chrome<'static> = Chrome {
    current: "tracker",
    ..LIST
};
const TRACKING: Chrome<'static> = Chrome {
    current: "tracker",
    ..ARTICLE
};
/// 数码 · 评测 belongs to 数码, whose tab it marks, and lists its reviews as 解说 lists its
/// articles.
const REVIEWED: Chrome<'static> = Chrome {
    current: "digital",
    section: "catalog gadgets catalog-commentary",
    ..EXPLAINED
};
const REVIEW: Chrome<'static> = Chrome {
    current: "digital",
    ..ARTICLE
};

/// The lists articles are on: 分享, which readers and XABC write, 调研, the research readers ask
/// for, and XABC's 解说, 时代, 追踪 and 评测.
#[derive(Clone, Copy, PartialEq)]
enum Shelf {
    Shared,
    Researched,
    Explained,
    People,
    Tracked,
    Reviewed,
}

impl Shelf {
    fn of(film: bool, person: bool, event: bool, product: bool, research: bool) -> Self {
        match (film, person, event, product, research) {
            (true, ..) => Self::Explained,
            (_, true, ..) => Self::People,
            (_, _, true, ..) => Self::Tracked,
            (_, _, _, true, _) => Self::Reviewed,
            (.., true) => Self::Researched,
            _ => Self::Shared,
        }
    }

    /// The condition keeping to the shelf's articles.
    fn articles(self) -> &'static str {
        match self {
            Self::Shared => {
                " AND a.film IS NULL AND a.person IS NULL AND a.event IS NULL AND a.product IS NULL AND a.question IS NULL"
            }
            Self::Researched => " AND a.question IS NOT NULL",
            Self::Explained => " AND a.film IS NOT NULL",
            Self::People => " AND a.person IS NOT NULL",
            Self::Tracked => " AND a.event IS NOT NULL",
            Self::Reviewed => " AND a.product IS NOT NULL",
        }
    }

    /// Where the shelf's list is, and its name.
    fn list(self) -> (&'static str, fn(bool) -> &'static str) {
        match self {
            Self::Shared => ("/articles", label),
            Self::Researched => (research::PATH, research::label),
            Self::Explained => (COMMENTARY, commentary_label),
            Self::People => (PEOPLE, people_label),
            Self::Tracked => (TRACKER, tracker_label),
            Self::Reviewed => (REVIEWS, reviews_label),
        }
    }

    /// Whether readers write the shelf's articles under their pen names, so its list shows their
    /// names and has a page for each; XABC alone writes the others.
    fn readers(self) -> bool {
        matches!(self, Self::Shared | Self::Researched)
    }

    /// Whether an article on the shelf is as new as its last update: XABC brings profiles,
    /// events and reviews up to date as they move.
    fn updated(self) -> bool {
        matches!(self, Self::People | Self::Tracked | Self::Reviewed)
    }

    /// What dates an article on the shelf, and orders its list.
    fn dated(self) -> &'static str {
        if self.updated() {
            "a.updated_at"
        } else {
            "a.published_at"
        }
    }

    /// How an article on the shelf shows its date: 追踪's to the minute, as events move by the
    /// hour.
    fn stamp(self) -> &'static str {
        match self {
            Self::Tracked => "%Y-%m-%d %H:%M",
            _ => "%Y-%m-%d",
        }
    }
}

pub(super) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route("/articles", get(index))
            .route(research::PATH, get(asked))
            .route("/articles/{id}", get(detail))
            .route(COMMENTARY, get(commentary))
            .route(PEOPLE, get(people))
            .route(TRACKER, get(tracker))
            .route(REVIEWS, get(reviews)),
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
    let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
    for list in [
        "/articles",
        research::PATH,
        COMMENTARY,
        PEOPLE,
        TRACKER,
        REVIEWS,
    ] {
        let _ = write!(
            xml,
            "<url><loc>{}</loc></url>",
            escape(&absolute(&state, list))
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

/// A public article on a list: the cover while tmpfile keeps it, and its author, title, dates,
/// reads and opening.
#[derive(FromRow)]
struct Card {
    id: Uuid,
    author_id: i64,
    author: String,
    title: String,
    excerpt: String,
    views: i64,
    published_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    cover: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

const CARD: &str =
    "SELECT a.id, a.author_id, w.name AS author, a.title, a.excerpt, a.views, a.published_at,
        a.updated_at, CASE WHEN i.expires_at > now() THEN i.url END AS cover, i.width, i.height";

/// The public articles on `shelf` holding the words searched; `select` names what to take of
/// them.
fn listed(select: &str, shelf: Shelf, by: Option<i64>, search: &Search) -> QueryBuilder<Postgres> {
    let mut query = QueryBuilder::new(select);
    query
        .push(
            " FROM articles a JOIN authors w ON w.user_id = a.author_id JOIN users u ON u.id = a.author_id
              LEFT JOIN article_images i ON i.id = a.cover
              WHERE u.status AND a.status = 'published'",
        )
        .push(shelf.articles());
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
    /// Whether it is a 时代 profile.
    person: bool,
    /// Whether it follows an event for 追踪.
    event: bool,
    /// Whether it reviews a product for 评测.
    product: bool,
    /// The reader's question it researches for 调研.
    question: Option<String>,
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
    readers(&state, Shelf::Shared, &params, &headers).await
}

// So is 调研's.
async fn asked(
    State(state): State<AppState>,
    _: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    readers(&state, Shelf::Researched, &params, &headers).await
}

/// 分享's list or 调研's: readers' articles newest first, 热读's, one author's or those holding
/// some words. 分享's leads to writing one; 调研's asks a question in the middle of its heading.
async fn readers(
    state: &AppState,
    shelf: Shelf,
    params: &Params,
    headers: &HeaderMap,
) -> Result<Response> {
    let en = params.english();
    let page = page(params)?;
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
    let (path, name) = shelf.list();
    let name = name(en);
    // What the list's articles are, what it says of itself and while it is empty, and what its
    // heading holds besides: 调研's question box in the middle, 分享's link to write beside the
    // search.
    let (noun, about, first, middle, beside) = if shelf == Shelf::Researched {
        (
            ("调研", "Research"),
            choose(
                en,
                "你想知道的，XABC 全网调研后写成文章",
                "Ask anything: XABC researches it across the web and writes it up",
            ),
            choose(
                en,
                "还没有公开的调研，在上面问第一个问题吧。",
                "No research yet. Ask the first question above.",
            ),
            ask_box(en),
            String::new(),
        )
    } else {
        (
            ("文章", "Articles"),
            choose(
                en,
                "读者分享的有趣好玩的东西",
                "Fun and interesting things our readers share",
            ),
            choose(
                en,
                "还没有公开的文章，<a href=\"/me/\">来写第一篇</a>。",
                "No articles yet. <a href=\"/me/\">Write the first one</a>.",
            ),
            String::new(),
            format!(
                "<a class=article-write href=\"/me/\">{}</a>",
                choose(en, "写文章", "Write")
            ),
        )
    };
    let mut query = listed(CARD, shelf, params.by, &search);
    if hot {
        query
            .push(" AND a.views > 0 AND a.published_at >= NOW() - INTERVAL '30 days' ORDER BY ")
            .push(HOT)
            .push(" DESC, a.published_at DESC, a.id DESC LIMIT 24");
    } else {
        search.order(&mut query, shelf.dated());
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
        Some(author) => choose(
            en,
            &format!("{author} 的{}", noun.0),
            &format!("{} by {author}", noun.1),
        )
        .to_owned(),
        None => about.to_owned(),
    };
    let found = found(
        &search,
        page,
        en,
        listed("SELECT count(*)", shelf, params.by, &search).build_query_scalar(),
        state,
    )
    .await?;
    // One author's page keeps to their articles in order.
    let views = if author.is_none() {
        format!(
            "<nav class=reads-views aria-label=\"{}\"><a href=\"{}\"{}>{}</a><a href=\"{}\"{}>{}</a></nav>",
            choose(en, "阅读排序", "Reading order"),
            localized(path, en),
            if hot || !search.is_empty() {
                ""
            } else {
                " aria-current=\"page\""
            },
            choose(en, "最新", "Latest"),
            escape(&localized(&format!("{path}?view=hot"), en)),
            if hot { " aria-current=\"page\"" } else { "" },
            choose(en, "热读", "Popular"),
        )
    } else {
        String::new()
    };
    let mut content = format!(
        "<header class=reads-heading><h1>{name}</h1><p>{}</p>{middle}<div class=reads-tools>{}{beside}{views}</div></header><div class=reads-list>",
        found.unwrap_or_else(|| escape(&description)),
        search_box(
            "reads-search",
            path,
            &search.words,
            choose(
                en,
                &format!("搜索{name}的标题和内容"),
                "Search titles and openings"
            ),
            en
        ),
    );
    for (index, row) in rows.iter().enumerate() {
        content.push_str(&card(
            row,
            en,
            hot.then_some(index + 1),
            shelf,
            state.config.timezone,
        ));
    }
    content.push_str("</div>");
    let latest = escape(&localized(path, en));
    if rows.is_empty() && !search.is_empty() {
        let _ = write!(
            content,
            "<p class=muted>{}</p>",
            choose(
                en,
                &format!(
                    "没有找到相关{0}，换个词试试，或看看<a href=\"{latest}\">最新{0}</a>。",
                    noun.0
                ),
                &format!(
                    "Nothing matches these words. Try others, or explore the <a href=\"{latest}\">latest ones</a>."
                ),
            )
        );
    } else if rows.is_empty() && hot {
        let _ = write!(
            content,
            "<p class=muted>{}</p>",
            choose(
                en,
                &format!(
                    "热读正在积累，先看看<a href=\"{latest}\">最新{}</a>。",
                    noun.0
                ),
                &format!(
                    "Popular reads are taking shape. Explore the <a href=\"{latest}\">latest ones</a>."
                ),
            )
        );
    } else if rows.is_empty() && author.is_some() {
        let _ = write!(
            content,
            "<p class=muted>{}</p>",
            choose(
                en,
                &format!("还没有公开的{}。", noun.0),
                "Nothing public yet."
            )
        );
    } else if rows.is_empty() {
        let _ = write!(content, "<p class=muted>{first}</p>");
    }
    content.push_str(&pagination(
        path,
        page,
        more,
        en,
        params.by,
        &search.words,
        "Older articles",
    ));
    let title = match (&author, page) {
        (Some(author), _) => format!("{author} · {name}"),
        (None, _) if hot => format!("{} · {name}", choose(en, "热读", "Popular")),
        (None, 1) => format!("{name} · 𝑵𝒆𝒘𝒔"),
        (None, page) => format!("{name} · {page} · 𝑵𝒆𝒘𝒔"),
    };
    let address = if hot {
        format!("{path}?view=hot")
    } else {
        listing(path, page, false, params.by, &search.words)
    };
    let html = crate::site::layout(
        state,
        en,
        &address,
        &title,
        &description,
        &content,
        None,
        if shelf == Shelf::Researched {
            &RESEARCH_LIST
        } else {
            &LIST
        },
    );
    Ok(listed_page(headers, html, &search))
}

/// 调研's question box, which the list script sends; its choice to keep the article private
/// shows for Pro readers.
fn ask_box(en: bool) -> String {
    format!(
        "<form class=research-ask><input name=question maxlength={} placeholder=\"{hint}\" aria-label=\"{hint}\" required><label class=research-public hidden><input type=checkbox name=public checked>{}</label><button type=submit>{}</button></form>",
        research::QUESTION_CHARS,
        choose(en, "公开", "Public"),
        choose(en, "调研", "Ask"),
        hint = choose(
            en,
            "想了解什么，调研一下你就知道！",
            "What do you want to know? Ask, and find out!"
        ),
    )
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

/// One of 精选's articles, as the home page script lays it out.
#[derive(FromRow, Serialize)]
struct Worth {
    id: Uuid,
    title: String,
    views: i64,
    film: bool,
    person: bool,
    event: bool,
    product: bool,
    research: bool,
    cover: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

/// 精选, which opens ALL: the four newest 分享 XABC and XAI wrote, newest first, then the four
/// hottest of the last 30 days of 分享 and XABC's other lists together, ranked as 热读 is, each
/// article once; while few are read, the newest fill the places. 解说 is among them, and its
/// list among the page's links, while 影视, whose works it explains, is public.
async fn worth(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let commentary = state
        .digest
        .topics()
        .iter()
        .any(|topic| topic.id == "movie");
    let articles: Vec<Worth> = QueryBuilder::<Postgres>::new(
        "WITH newest AS (
             SELECT a.id, a.published_at FROM articles a JOIN users u ON u.id = a.author_id
             WHERE u.email = ANY(",
    )
    .push_bind(super::xabc::SHARING)
    .push(
        ") AND u.status AND a.status = 'published' AND a.film IS NULL AND a.person IS NULL AND a.event IS NULL
               AND a.product IS NULL
             ORDER BY a.published_at DESC, a.id DESC LIMIT 4)
         SELECT a.id, a.title, a.views, a.film IS NOT NULL AS film, a.person IS NOT NULL AS person,
                a.event IS NOT NULL AS event, a.product IS NOT NULL AS product,
                a.question IS NOT NULL AS research,
                CASE WHEN i.expires_at > now() THEN i.url END AS cover, i.width, i.height
         FROM articles a JOIN users u ON u.id = a.author_id
         LEFT JOIN article_images i ON i.id = a.cover
         LEFT JOIN newest n ON n.id = a.id
         WHERE u.status AND a.status = 'published' AND a.published_at >= NOW() - INTERVAL '30 days'",
    )
    .push(if commentary { "" } else { " AND a.film IS NULL" })
    .push(" ORDER BY n.published_at DESC NULLS LAST, ")
    .push(HOT)
    .push(" DESC, a.published_at DESC, a.id DESC LIMIT 8")
    .build_query_as()
    .fetch_all(&state.pool)
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(json!({ "articles": articles, "commentary": commentary }).to_string()),
        "application/json",
    ))
}

/// A page of `shelf`'s public articles holding the words searched, newest first, and whether
/// more follow; a page past the last is not found.
async fn shelved(
    state: &AppState,
    shelf: Shelf,
    search: &Search,
    page: u32,
) -> Result<(Vec<Card>, bool)> {
    let mut query = listed(CARD, shelf, None, search);
    search.order(&mut query, shelf.dated());
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
    Ok((rows, more))
}

/// A list of XABC's articles on one of its lines, as picture cards: under a catalogue section's
/// tabs (文娱 · 解说, 数码 · 评测), or else heading a page of its own, tiled as 分享's are (时代,
/// 追踪). Its words are in Chinese and English.
struct List {
    shelf: Shelf,
    /// The catalogue section it sits under, if any.
    section: Option<crate::catalog::Section>,
    path: &'static str,
    name: fn(bool) -> &'static str,
    chrome: &'static Chrome<'static>,
    description: (&'static str, &'static str),
    /// What the search box asks for.
    search: (&'static str, &'static str),
    /// What the list says while it is empty.
    first: (&'static str, &'static str),
    /// What it says when a search finds nothing.
    nothing: (&'static str, &'static str),
}

/// 文娱 · 解说: XABC's articles on films and series.
const EXPLANATIONS: List = List {
    shelf: Shelf::Explained,
    section: Some(crate::catalog::Section::Culture),
    path: COMMENTARY,
    name: commentary_label,
    chrome: &EXPLAINED,
    description: (
        "好电影、好剧的完整解说：剧情、结局与幕后",
        "Good films and series explained: story, ending and the making",
    ),
    search: ("搜索解说的片名和内容", "Search titles and openings"),
    first: ("第一篇解说正在写。", "The first one is being written."),
    nothing: (
        "没有找到相关解说，换个词试试。",
        "Nothing explained matches these words. Try others.",
    ),
};

/// 数码 · 评测: XABC's reviews of the products 数码 collects.
const REVIEWS_LIST: List = List {
    shelf: Shelf::Reviewed,
    section: Some(crate::catalog::Section::Gadgets),
    path: REVIEWS,
    name: reviews_label,
    chrome: &REVIEWED,
    description: (
        "热门数码产品的综合评测：官方参数、各家评测与真实用户的体验",
        "Popular gadgets reviewed from their specifications, reviewers and owners",
    ),
    search: ("搜索评测的产品和内容", "Search products and openings"),
    first: ("第一篇评测正在写。", "The first review is being written."),
    nothing: (
        "没有找到相关评测，换个词试试。",
        "No reviews match these words. Try others.",
    ),
};

/// 时代: XABC's profiles of the people and the companies of this age.
const PROFILES: List = List {
    shelf: Shelf::People,
    section: None,
    path: PEOPLE,
    name: people_label,
    chrome: &PEOPLE_LIST,
    description: (
        "这个时代的人物和企业：他们是谁，做成了什么，现在怎样",
        "The people and companies of our time: who they are, what they did, where they stand now",
    ),
    search: ("搜索人物、企业和内容", "Search names and openings"),
    first: ("第一篇文章正在写。", "The first profile is being written."),
    nothing: (
        "没有找到相关的人物或企业，换个词试试。",
        "No profiles match these words. Try others.",
    ),
};

/// 追踪: XABC's articles following the events the whole web talks about.
const EVENTS: List = List {
    shelf: Shelf::Tracked,
    section: None,
    path: TRACKER,
    name: tracker_label,
    chrome: &TRACKER_LIST,
    description: (
        "全网最受关注的热点事件：怎么发生的，到了哪一步，接下来会怎样",
        "The events the world is talking about: how they began, where they stand, what comes next",
    ),
    search: ("搜索事件和内容", "Search events and openings"),
    first: ("第一篇追踪正在写。", "The first one is being written."),
    nothing: (
        "没有找到相关事件，换个词试试。",
        "No events match these words. Try others.",
    ),
};

async fn commentary(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    list(&state, reader, &EXPLANATIONS, &params, &headers).await
}

async fn reviews(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    list(&state, reader, &REVIEWS_LIST, &params, &headers).await
}

// 时代's and 追踪's pages are the same for everyone, but a preview of them still needs an
// administrator.
async fn people(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    list(&state, reader, &PROFILES, &params, &headers).await
}

async fn tracker(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    list(&state, reader, &EVENTS, &params, &headers).await
}

async fn list(
    state: &AppState,
    reader: Reader,
    list: &List,
    params: &Params,
    headers: &HeaderMap,
) -> Result<Response> {
    let en = params.english();
    let say = |(zh, english): (&'static str, &'static str)| choose(en, zh, english);
    let page = page(params)?;
    let search = Search::new(params.q.as_deref());
    let (rows, more) = shelved(state, list.shelf, &search, page).await?;
    let name = (list.name)(en);
    let description = say(list.description);
    let found = found(
        &search,
        page,
        en,
        listed("SELECT count(*)", list.shelf, None, &search).build_query_scalar(),
        state,
    )
    .await?;
    let search_form = |class| search_box(class, list.path, &search.words, say(list.search), en);
    // Under a catalogue section the list follows the section's tabs and is titled after it;
    // otherwise it heads a page of its own.
    let (mut content, empty, suffix) = match list.section {
        Some(section) => {
            let mut content = crate::catalog::articles_tabs(state, section, reader, en);
            content.push_str(&search_form("catalog-search"));
            if let Some(found) = found {
                let _ = write!(content, "<p class=\"commentary-found muted\">{found}</p>");
            }
            content.push_str("<div class=\"reads-list catalog-grid\">");
            (content, "empty", section.label(en))
        }
        None => (
            format!(
                "<header class=reads-heading><h1>{name}</h1><p>{}</p><div class=reads-tools>{}</div></header><div class=reads-list>",
                found.unwrap_or_else(|| escape(description)),
                search_form("reads-search"),
            ),
            "muted",
            "𝑵𝒆𝒘𝒔",
        ),
    };
    for row in &rows {
        content.push_str(&card(row, en, None, list.shelf, state.config.timezone));
    }
    content.push_str("</div>");
    if rows.is_empty() {
        let _ = write!(
            content,
            "<p class={empty}>{}</p>",
            say(if search.is_empty() {
                list.first
            } else {
                list.nothing
            })
        );
    }
    content.push_str(&pagination(
        list.path,
        page,
        more,
        en,
        None,
        &search.words,
        "Older articles",
    ));
    let title = match page {
        1 => format!("{name} · {suffix}"),
        page => format!("{name} · {page} · {suffix}"),
    };
    let html = crate::site::layout(
        state,
        en,
        &listing(list.path, page, false, None, &search.words),
        &title,
        description,
        &content,
        None,
        list.chrome,
    );
    Ok(listed_page(headers, html, &search))
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

/// An article's card: its cover, then 热读's place, the author on readers' lists (the others are
/// all XABC's), the date, the reads, the title and the opening.
fn card(
    item: &Card,
    en: bool,
    rank: Option<usize>,
    shelf: Shelf,
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
    let author = if shelf.readers() {
        format!(
            "<a href=\"{}\">{}</a> · ",
            escape(&localized(
                &format!("{}?by={}", shelf.list().0, item.author_id),
                en
            )),
            escape(&item.author)
        )
    } else {
        String::new()
    };
    let date = if shelf.updated() {
        item.updated_at
    } else {
        item.published_at
    };
    format!(
        "<article class=\"item image-news-card catalog-card\">{cover}<div class=catalog-card-body><p class=item-meta>{rank}{author}<time datetime=\"{}\">{}</time>{}</p><h2 class=item-title><a href=\"{href}\">{}</a></h2><p class=item-summary>{}</p></div></article>",
        date.to_rfc3339(),
        date.with_timezone(&timezone).format(shelf.stamp()),
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
        "SELECT a.id, a.author_id, w.name AS author, a.title, a.body, a.excerpt, a.cover, a.status, a.revision, a.views, a.updated_at, a.published_at, a.film IS NOT NULL AS film, a.person IS NOT NULL AS person, a.event IS NOT NULL AS event, a.product IS NOT NULL AS product, a.question",
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
    let body = markdown(parsed(&article.body, MARKDOWN), |destination| {
        image_id(destination).and_then(picture)
    });
    let cover = article.cover.and_then(picture);
    let title = if article.title.is_empty() {
        choose(en, "无标题", "Untitled")
    } else {
        &article.title
    };
    let shelf = Shelf::of(
        article.film,
        article.person,
        article.event,
        article.product,
        article.question.is_some(),
    );
    // A profile or an event is as new as its last update.
    let date = if shelf.updated() {
        article.updated_at
    } else {
        article.published_at.unwrap_or(article.updated_at)
    };
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
        // A research's words are XABC's: its reader previews it without editing.
        Mode::Author(author) => format!(
            "<div class=article-notice><span class=article-status>{} · {status}</span><span class=article-actions>{}{}</span></div>",
            choose(en, "预览", "Preview"),
            if author.is_some() && shelf == Shelf::Researched {
                String::new()
            } else {
                edit(choose(en, "继续编辑", "Edit"))
            },
            if author.is_none() {
                copies.as_str()
            } else {
                ""
            },
        ),
    };
    // The author's name leads to their articles on readers' lists; XABC's own lists lead back
    // to themselves: all their articles are XABC's.
    let (list, name) = shelf.list();
    let name = name(en);
    let by = if shelf.readers() {
        localized(&format!("{list}?by={}", article.author_id), en)
    } else {
        localized(list, en)
    };
    // A research article shows the question it answers, as its reader asked it.
    let asked = article
        .question
        .as_deref()
        .map(|question| {
            format!(
                "<p class=research-question><b>{}</b><span>{}</span></p>",
                choose(en, "问", "Q"),
                escape(question)
            )
        })
        .unwrap_or_default();
    let mut content = format!(
        "{notice}<article class=\"read article\">{}<header class=read-heading><div class=read-title><h1>{}</h1></div><div class=item-meta><a href=\"{}\">{}</a> · <time datetime=\"{}\">{}</time> · {views}</div>{asked}</header><div class=read-body>{body}</div></article>",
        // A profile's cover, its subject's portrait, stands for it on the lists, and so does an
        // event's, the first picture it shows; the page opens with the writer's own pictures.
        cover
            .as_ref()
            .filter(|_| !shelf.updated())
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
        date.with_timezone(&state.config.timezone).format(shelf.stamp()),
    );
    let _ = write!(
        content,
        "<div class=read-actions><a href=\"{}\">← {name}</a></div>",
        localized(list, en),
    );
    let path = match mode {
        Mode::Author(_) => format!("/api/articles/{}/preview", article.id),
        Mode::Public | Mode::Review => format!("/articles/{}", article.id),
    };
    let canonical = absolute(state, &localized(&path, en));
    let mut structured = json!({
        "@context": "https://schema.org", "@type": "Article", "headline": title,
        "description": article.excerpt, "url": canonical, "mainEntityOfPage": canonical,
        "datePublished": article.published_at.unwrap_or(article.updated_at).to_rfc3339(),
        "dateModified": article.updated_at.to_rfc3339(),
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
        en,
        &path,
        &format!("{title} · {name}"),
        &article.excerpt,
        &content,
        Some(structured),
        match shelf {
            Shelf::Shared => &ARTICLE,
            Shelf::Researched => &RESEARCH,
            Shelf::Explained => &EXPLANATION,
            Shelf::People => &PROFILE,
            Shelf::Tracked => &TRACKING,
            Shelf::Reviewed => &REVIEW,
        },
    ))
}
