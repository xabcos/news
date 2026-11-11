//! 分享: articles readers write in the workspace. Authors write under a pen name. An article is
//! public once the AI finds nothing to hold it for (`check`) or an administrator gives it a ❤️,
//! and an author's edit takes it back to review.
//! Pictures go to tmpfile as they were uploaded, which can't delete them, so their links go only
//! to public pages, administrators' previews and text the author copies out of the editor; the
//! author's own pages show them through this server. A read counts once a day per browser,
//! never the author's own. XABC and XAI, the site's own pen names, each publish one every hour
//! (`xabc`), and XABC writes up what readers ask 调研 to look into under their pen names
//! (`research`) and keeps the Codex & Claude handbook (`handbook`).
mod check;
pub(crate) mod handbook;
mod pages;
pub(crate) mod research;
pub(crate) mod xabc;

#[cfg(test)]
mod tests;

use crate::{
    auth::{self, User},
    digest::unchanged,
    error::{ApiJson, AppError, Result},
    state::AppState,
};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State, rejection::BytesRejection},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use chrono::{DateTime, NaiveDate, Utc};
use pulldown_cmark::{Event, Options, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool};
use std::collections::HashMap;
use uuid::Uuid;

/// Articles an author may start in a day, by the site's date.
const DAILY_ARTICLES: i16 = 10;
/// Pictures an author may upload in a day.
const DAILY_IMAGES: i16 = 64;
/// AI reviews an author's articles get in a day; past them a submit waits for an administrator,
/// so no one keeps the AI busy.
const DAILY_REVIEWS: i16 = 30;
/// Pictures one article may show, its cover included.
const ARTICLE_IMAGES: usize = 128;
/// The largest picture: nginx lets ten megabytes through (`client_max_body_size 10m`).
const IMAGE_BYTES: usize = 10 * 1024 * 1024;
const TITLE_CHARS: usize = 100;
const BODY_CHARS: usize = 320_000;
/// A save carries the body as JSON, which stays below this even at four bytes a character;
/// nginx lets as much through (`client_max_body_size 2m`).
const SAVE_BYTES: usize = 2 * 1024 * 1024;
const EXCERPT_CHARS: usize = 120;
/// How the body names one of the article's pictures: `![…](image:<id>)`.
const IMAGE: &str = "image:";
/// The Markdown readers write, the dialect Reads renders.
const MARKDOWN: Options = Options::ENABLE_STRIKETHROUGH.union(Options::ENABLE_TABLES);

/// Where 文娱 · 解说 lists XABC's articles on films and series, under Culture.
const COMMENTARY: &str = "/culture/commentary";
/// Where 时代 lists XABC's profiles of the people and the companies of this age.
pub(crate) const PEOPLE: &str = "/people";
/// Where 追踪 lists XABC's articles following the events the whole web talks about.
pub(crate) const TRACKER: &str = "/tracker";
/// Where 数码 · 评测 lists XABC's reviews of the products 数码 collects, under 数码.
const REVIEWS: &str = "/gadgets/reviews";

/// 文娱 · 解说's name, in the page's language.
pub(crate) fn commentary_label(en: bool) -> &'static str {
    crate::site::choose(en, "解说", "𝑬𝒙𝒑𝒍𝒂𝒊𝒏𝒆𝒅")
}

/// 时代's name, in the page's language.
pub(crate) fn people_label(en: bool) -> &'static str {
    crate::site::choose(en, "时代", "𝑷𝒆𝒐𝒑𝒍𝒆")
}

/// 追踪's name, in the page's language.
pub(crate) fn tracker_label(en: bool) -> &'static str {
    crate::site::choose(en, "追踪", "𝑻𝒓𝒂𝒄𝒌𝒆𝒓")
}

/// 数码 · 评测's name, in the page's language.
pub(crate) fn reviews_label(en: bool) -> &'static str {
    crate::site::choose(en, "评测", "𝑹𝒆𝒗𝒊𝒆𝒘𝒔")
}

/// What an article is, as `articles.kind` keeps it: one of 分享, or of a line that gives it a
/// subject (`articles.subject`), which one article has, though a question may be asked twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
enum Kind {
    /// 分享, which readers, XABC and XAI write: no subject.
    Share,
    /// 文娱 · 解说 explains a film or series: the work's key in 影视 (`catalog::work_key`), so
    /// either finds the other.
    Film,
    /// 时代 profiles a person or a company: `xabc::key` of the English name, after `company:`
    /// for a company.
    Person,
    /// 追踪 follows an event: `xabc::key` of the slug the searches give it.
    Event,
    /// 数码 · 评测 reviews a product: its `catalog_entries.key`, wherever 数码 files it.
    Product,
    /// 调研 answers a reader's question: the question as asked.
    Question,
    /// A chapter of the Codex & Claude handbook: its key (`handbook::CHAPTERS`).
    Chapter,
}

/// The section's name, in the page's language.
pub(crate) fn label(en: bool) -> &'static str {
    crate::site::choose(en, "分享", "𝑨𝒓𝒕𝒊𝒄𝒍𝒆𝒔")
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/author", put(name))
        .route(
            "/api/articles",
            get(mine)
                .post(create)
                .layer(DefaultBodyLimit::max(SAVE_BYTES)),
        )
        .route(
            "/api/articles/{id}",
            get(edit)
                .patch(save)
                .delete(remove)
                .layer(DefaultBodyLimit::max(SAVE_BYTES)),
        )
        .route("/api/articles/{id}/submit", post(submit))
        .route(
            "/api/articles/{id}/images",
            post(upload).layer(DefaultBodyLimit::max(IMAGE_BYTES)),
        )
        .route("/api/articles/{id}/images/{image}", get(picture))
        .route(
            "/api/articles/{id}/render",
            post(rendered).layer(DefaultBodyLimit::max(SAVE_BYTES)),
        )
        .route("/api/articles/{id}/views", post(view))
        .route("/api/articles/views", get(views))
        .route("/api/admin/articles", get(queue))
        .route("/api/admin/articles/{id}", patch(review))
        .route(
            "/api/admin/people",
            post(|user, ApiJson(request)| ask(user, request, xabc::Line::People)),
        )
        .route(
            "/api/admin/tracker",
            post(|user, ApiJson(request)| ask(user, request, xabc::Line::Tracker)),
        )
        .merge(research::router())
        .merge(handbook::router())
        .merge(pages::router())
}

/// An article in the author's list or the review queue.
#[derive(FromRow, Serialize)]
struct Listed {
    id: Uuid,
    title: String,
    author: String,
    status: String,
    /// Why it waits for an administrator, when it does.
    held: Option<String>,
    revision: i32,
    views: i64,
    updated_at: DateTime<Utc>,
    published_at: Option<DateTime<Utc>>,
    kind: Kind,
}

const LISTED: &str = "SELECT a.id, a.title, w.name AS author, a.status, a.held, a.revision, a.views, a.updated_at, a.published_at, a.kind
    FROM articles a JOIN authors w ON w.user_id = a.author_id JOIN users u ON u.id = a.author_id";

/// A search of 分享 or one of XABC's lists: its words, as the page shows them, and the patterns each of
/// which an article's title or opening must hold.
struct Search {
    words: String,
    patterns: Vec<String>,
}

impl Search {
    fn new(q: Option<&str>) -> Self {
        let words = crate::site::search_words(q);
        let patterns = words
            .split_whitespace()
            .map(crate::site::containing)
            .collect();
        Self { words, patterns }
    }

    fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Orders a list newest first by `newest`, after the articles whose titles hold every word.
    fn order(&self, query: &mut sqlx::QueryBuilder<sqlx::Postgres>, newest: &str) {
        query.push(" ORDER BY ");
        if !self.is_empty() {
            query
                .push("a.title ILIKE ALL(")
                .push_bind(self.patterns.clone())
                .push(") DESC, ");
        }
        query.push(newest).push(" DESC, a.id DESC");
    }

    /// Narrows `query`, already filtering, to the articles holding every word.
    fn narrow(&self, query: &mut sqlx::QueryBuilder<sqlx::Postgres>) {
        if !self.is_empty() {
            query
                .push(" AND concat_ws(' ', a.title, a.excerpt) ILIKE ALL(")
                .push_bind(self.patterns.clone())
                .push(")");
        }
    }
}

/// An article as its author edits it.
#[derive(FromRow, Serialize)]
struct Draft {
    id: Uuid,
    title: String,
    body: String,
    cover: Option<Uuid>,
    status: String,
    revision: i32,
    views: i64,
    updated_at: DateTime<Utc>,
    published_at: Option<DateTime<Utc>>,
    /// Where each of its pictures is stored, which text copied out of the editor carries; read
    /// only when the editor opens it.
    #[sqlx(skip)]
    images: HashMap<Uuid, String>,
}

#[derive(FromRow, Serialize)]
struct Uploaded {
    id: Uuid,
    url: String,
    width: i32,
    height: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Name {
    name: String,
}

/// A new article, and the author the writing page was opened for: a page left open while the
/// browser signs in as someone else must not write under that account.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Content {
    author: i64,
    title: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    revision: i32,
    title: String,
    body: String,
    cover: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    published: bool,
    revision: i32,
}

/// The signed-in reader's pen name and articles, newest edit first; their research is 调研's.
async fn mine(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM authors WHERE user_id = $1")
        .bind(user.id)
        .fetch_optional(&state.pool)
        .await?;
    let mut articles: Vec<Listed> = sqlx::QueryBuilder::new(LISTED)
        .push(" WHERE a.kind <> 'question' AND a.author_id = ")
        .push_bind(user.id)
        .push(" ORDER BY a.updated_at DESC, a.id DESC")
        .build_query_as()
        .fetch_all(&state.pool)
        .await?;
    // An author isn't told why an article waits: it would help word around the review.
    for article in &mut articles {
        article.held = None;
    }
    Ok(Json(json!({
        "author": name.map(|name| json!({ "id": user.id, "name": name })),
        "admin": user.is_admin(),
        "articles": articles,
    })))
}

/// Sets or changes the pen name readers see.
async fn name(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<Name>,
) -> Result<Json<Value>> {
    let name = pen_name(&request.name)?;
    sqlx::query(
        "INSERT INTO authors (user_id, name) VALUES ($1, $2)
         ON CONFLICT (user_id) DO UPDATE SET name = EXCLUDED.name",
    )
    .bind(user.id)
    .bind(&name)
    .execute(&state.pool)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(database) if database.is_unique_violation() => {
            AppError::Conflict("这个笔名已经有人用了".into())
        }
        _ => error.into(),
    })?;
    Ok(Json(json!({ "id": user.id, "name": name })))
}

/// Starts a draft, which counts against the day's allowance.
async fn create(
    State(state): State<AppState>,
    user: User,
    ApiJson(content): ApiJson<Content>,
) -> Result<(StatusCode, Json<Draft>)> {
    if content.author != user.id {
        return Err(AppError::Conflict("账户已切换，请刷新页面".into()));
    }
    let content = checked(&content.title, &content.body, None)?;
    let mut tx = state.pool.begin().await?;
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM authors WHERE user_id = $1)")
        .bind(user.id)
        .fetch_one(&mut *tx)
        .await?
    {
        return Err(AppError::Conflict("请先在工作台设置笔名".into()));
    }
    take(&mut *tx, user.id, state.digest.today(), Quota::Articles).await?;
    let article: Draft = sqlx::query_as(
        "INSERT INTO articles (id, author_id, title, body, excerpt) VALUES ($1, $2, $3, $4, $5)
         RETURNING id, title, body, cover, status, revision, views, updated_at, published_at",
    )
    .bind(Uuid::new_v4())
    .bind(user.id)
    .bind(&content.title)
    .bind(&content.body)
    .bind(&content.excerpt)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(article)))
}

async fn edit(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
) -> Result<Json<Draft>> {
    let mut article = owned(&state, &user, &id).await?;
    article.images = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, url FROM article_images WHERE article_id = $1",
    )
    .bind(article.id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect();
    Ok(Json(article))
}

/// Saves the revision the editor holds. An author's edit takes a pending article back to being
/// a draft and a public one back to review, which the AI gives at once; an administrator's leaves
/// its status as it was. A research article's words are XABC's: only an administrator edits them.
async fn save(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Save>,
) -> Result<Json<Value>> {
    let id = parse(&id)?;
    let content = checked(&request.title, &request.body, request.cover)?;
    let saved: Option<(String, i32, DateTime<Utc>)> = sqlx::query_as(
        "UPDATE articles SET title = $4, body = $5, excerpt = $6, cover = $7,
             revision = revision + 1, updated_at = now(),
             status = CASE WHEN $8 THEN status WHEN status = 'pending' THEN 'draft'
                           WHEN status = 'published' THEN 'pending' ELSE status END
         WHERE id = $1 AND (author_id = $2 AND kind <> 'question' OR $8) AND revision = $3
         RETURNING status, revision, updated_at",
    )
    .bind(id)
    .bind(user.id)
    .bind(request.revision)
    .bind(&content.title)
    .bind(&content.body)
    .bind(&content.excerpt)
    .bind(content.cover)
    .bind(user.is_admin())
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| match &error {
        sqlx::Error::Database(database) if database.is_foreign_key_violation() => {
            AppError::BadRequest("封面要用这篇文章里上传的图片".into())
        }
        _ => error.into(),
    })?;
    let (mut status, revision, updated_at) = match saved {
        Some(saved) => saved,
        None => return Err(stale(&state, &user, id).await),
    };
    if status == "pending" && !user.is_admin() {
        status = check::settled(&state, id, revision).await?.0;
    }
    Ok(Json(
        json!({ "status": status, "revision": revision, "updated_at": updated_at }),
    ))
}

/// Sends a draft to review, which the AI gives at once; an administrator's own goes straight
/// out.
async fn submit(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Revision>,
) -> Result<Json<Value>> {
    let article = owned(&state, &user, &id).await?;
    if article.title.is_empty() {
        return Err(AppError::BadRequest("请先写标题".into()));
    }
    if article.body.trim().is_empty() {
        return Err(AppError::BadRequest("请先写正文".into()));
    }
    let status = if user.is_admin() {
        "published"
    } else {
        "pending"
    };
    let submitted: Option<(String, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "UPDATE articles SET status = $4,
             published_at = CASE WHEN $4 = 'published' THEN COALESCE(published_at, now()) ELSE published_at END
         WHERE id = $1 AND author_id = $2 AND revision = $3 AND status = 'draft'
         RETURNING status, revision, published_at",
    )
    .bind(article.id)
    .bind(user.id)
    .bind(request.revision)
    .bind(status)
    .fetch_optional(&state.pool)
    .await?;
    let (mut status, revision, mut published_at) =
        submitted.ok_or_else(|| AppError::Conflict("文章已修改或已提交，请刷新后再试".into()))?;
    if status == "pending" {
        (status, published_at) = check::settled(&state, article.id, revision).await?;
    }
    Ok(Json(
        json!({ "status": status, "revision": revision, "published_at": published_at }),
    ))
}

async fn remove(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let deleted = sqlx::query("DELETE FROM articles WHERE id = $1 AND (author_id = $2 OR $3)")
        .bind(parse(&id)?)
        .bind(user.id)
        .bind(user.is_admin())
        .execute(&state.pool)
        .await?;
    if deleted.rows_affected() == 0 {
        return Err(missing());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Keeps one picture, sent as the request body, for the author's article.
async fn upload(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    body: std::result::Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Uploaded>)> {
    let article = parse(&id)?;
    let bytes = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            AppError::PayloadTooLarge("图片不能超过 10 MB".into())
        } else {
            AppError::BadRequest("图片没有传完整，请重试".into())
        }
    })?;
    let (content_type, (width, height)) = image_kind(&bytes)?;
    if !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM articles WHERE id = $1 AND (author_id = $2 AND kind <> 'question' OR $3))",
    )
    .bind(article)
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_one(&state.pool)
    .await?
    {
        return Err(missing());
    }
    // An administrator's pictures, often for XABC's articles, count against no allowance.
    let day = match user.is_admin() {
        true => None,
        false => Some(take(&state.pool, user.id, state.digest.today(), Quota::Images).await?),
    };
    let Some(stored) = crate::media::upload_image(&state, bytes.into(), content_type).await else {
        if let Some(day) = day {
            give_back(&state.pool, user.id, day).await;
        }
        return Err(AppError::ServiceUnavailable(
            "图片暂时存不下，请稍后重试".into(),
        ));
    };
    let image: Uploaded = sqlx::query_as(
        "INSERT INTO article_images (id, article_id, url, content_type, width, height, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id, url, width, height",
    )
    .bind(Uuid::new_v4())
    .bind(article)
    .bind(&stored.direct_link)
    .bind(content_type)
    .bind(width)
    .bind(height)
    .bind(stored.expires_at)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(image)))
}

/// What the writing page shows beside the editor: the body it holds now, as the article page
/// renders it, with the article's pictures through this server. Only its author or an
/// administrator may ask.
async fn rendered(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(text): ApiJson<Text>,
) -> Result<Json<Value>> {
    let article = owned(&state, &user, &id).await?;
    let images: HashMap<Uuid, (i32, i32)> = sqlx::query_as::<_, (Uuid, i32, i32)>(
        "SELECT id, width, height FROM article_images WHERE article_id = $1",
    )
    .bind(article.id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|(image, width, height)| (image, (width, height)))
    .collect();
    let html = crate::site::markdown(crate::site::parsed(&text.body, MARKDOWN), |destination| {
        let image = image_id(destination)?;
        Some(crate::site::Picture {
            src: format!("/api/articles/{}/images/{image}", article.id),
            size: Some(*images.get(&image)?),
        })
    });
    Ok(Json(json!({ "html": html })))
}

/// A body the writing page wants rendered.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    body: String,
}

/// A picture of the author's own article, read through this server so its tmpfile link stays
/// private. The browser keeps it and asks again each time, so leaving the account ends access.
async fn picture(
    State(state): State<AppState>,
    user: User,
    Path((id, image)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    let (article, image) = (parse(&id)?, parse(&image)?);
    let (url, content_type): (String, String) = sqlx::query_as(
        "SELECT i.url, i.content_type FROM article_images i JOIN articles a ON a.id = i.article_id
         WHERE i.id = $1 AND i.article_id = $2 AND (a.author_id = $3 OR $4)",
    )
    .bind(image)
    .bind(article)
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("图片不存在".into()))?;
    let tag = format!("\"{image}\"");
    let mut response = if unchanged(&headers, &tag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let stored = state.http.get(&url).send().await?;
        if !stored.status().is_success() {
            return Err(AppError::NotFound("图片已经失效".into()));
        }
        let bytes = stored.bytes().await?;
        let mut response = bytes.into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type)
                .map_err(|_| AppError::Internal("图片类型无效".into()))?,
        );
        response
    };
    let headers = response.headers_mut();
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&tag).map_err(|_| AppError::Internal("图片标识无效".into()))?,
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    Ok(response)
}

/// One more read of a public article, unless its author is the reader.
async fn view(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>> {
    let id = parse(&id)?;
    let reader = auth::viewer(&state, &headers).await?.map(|user| user.id);
    let counted: Option<i64> = sqlx::query_scalar(
        "UPDATE articles a SET views = a.views + 1 FROM users u
         WHERE a.id = $1 AND a.status = 'published' AND u.id = a.author_id AND u.status
           AND a.author_id IS DISTINCT FROM $2
         RETURNING a.views",
    )
    .bind(id)
    .bind(reader)
    .fetch_optional(&state.pool)
    .await?;
    let views = match counted {
        Some(views) => views,
        None => sqlx::query_scalar(
            "SELECT a.views FROM articles a JOIN users u ON u.id = a.author_id
             WHERE a.id = $1 AND a.status = 'published' AND u.status AND a.author_id = $2",
        )
        .bind(id)
        .bind(reader)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(missing)?,
    };
    Ok(Json(json!({ "views": views })))
}

/// Articles a list shows at once, at most: a page holds 24.
const COUNTED: usize = 50;

#[derive(Deserialize)]
struct Ids {
    ids: String,
}

/// The reads public articles have now, for pages that came from a cache: `?ids=` names them,
/// comma-separated. Like every `/api` answer it is never cached.
async fn views(State(state): State<AppState>, Query(request): Query<Ids>) -> Result<Json<Value>> {
    let ids: Vec<Uuid> = request
        .ids
        .split(',')
        .filter_map(|id| Uuid::parse_str(id.trim()).ok())
        .take(COUNTED)
        .collect();
    let counts: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT a.id, a.views FROM articles a JOIN users u ON u.id = a.author_id
         WHERE a.id = ANY($1) AND a.status = 'published' AND u.status",
    )
    .bind(&ids)
    .fetch_all(&state.pool)
    .await?;
    let views: serde_json::Map<String, Value> = counts
        .into_iter()
        .map(|(id, views)| (id.to_string(), json!(views)))
        .collect();
    Ok(Json(json!({ "views": views })))
}

#[derive(Deserialize)]
struct Find {
    day: Option<NaiveDate>,
    q: Option<String>,
}

/// What waits for a ❤️, oldest first, and the latest 30 public articles, or all of one day's or
/// of those holding some words, so an administrator finds an older one to copy.
async fn queue(
    State(state): State<AppState>,
    user: User,
    Query(find): Query<Find>,
) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let pending: Vec<Listed> = sqlx::QueryBuilder::new(LISTED)
        .push(" WHERE a.status = 'pending' AND u.status ORDER BY a.updated_at, a.id")
        .build_query_as()
        .fetch_all(&state.pool)
        .await?;
    let search = Search::new(find.q.as_deref());
    let mut query = sqlx::QueryBuilder::new(LISTED);
    query.push(" WHERE a.status = 'published' AND u.status");
    if let Some(day) = find.day {
        query
            .push(" AND (a.published_at AT TIME ZONE ")
            .push_bind(state.config.timezone.name())
            .push(")::date = ")
            .push_bind(day);
    }
    search.narrow(&mut query);
    let published: Vec<Listed> = query
        .push(" ORDER BY a.published_at DESC, a.id DESC LIMIT ")
        .push_bind(if find.day.is_some() || !search.is_empty() {
            200
        } else {
            30
        })
        .build_query_as()
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(json!({ "pending": pending, "published": published })))
}

/// An administrator's ❤️ publishes the revision they read and outdates any refusal the AI is
/// still giving; taking it back returns the article to review and holds it there, so the AI
/// doesn't pass it again whatever its author changes.
async fn review(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Review>,
) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let sql = if request.published {
        "UPDATE articles SET status = 'published', published_at = COALESCE(published_at, now()),
             held = NULL, approvals = approvals + 1
         WHERE id = $1 AND revision = $2 AND status = 'pending' RETURNING status, revision"
    } else {
        "UPDATE articles SET status = 'pending', held = '管理员下线'
         WHERE id = $1 AND revision = $2 AND status = 'published' RETURNING status, revision"
    };
    let (status, revision): (String, i32) = sqlx::query_as(sql)
        .bind(parse(&id)?)
        .bind(request.revision)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::Conflict("文章刚有修改或状态已变，请刷新后再看".into()))?;
    Ok(Json(json!({ "status": status, "revision": revision })))
}

/// Puts what an administrator names on the line's list, a person or a company on 时代's, an
/// event on 追踪's, which the line writes before its own rounds, and tells how many names wait
/// ahead.
async fn ask(user: User, request: Name, line: xabc::Line) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let name = request.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest(
            match line {
                xabc::Line::Tracker => "写上要追踪的事",
                _ => "写上人物或企业的名字",
            }
            .into(),
        ));
    }
    Ok(Json(json!({ "ahead": xabc::name(line, name.to_owned()) })))
}

/// What an author does a number of times a day: start articles, upload pictures, have the AI
/// review articles and ask for research, as many as their plan allows.
#[derive(Clone, Copy)]
enum Quota {
    Articles,
    Images,
    Reviews,
    Research(u8),
}

impl Quota {
    /// Its count among the author's, how many a day allows and what a refusal says.
    fn allowance(self) -> (&'static str, i16, String) {
        match self {
            Self::Articles => (
                "articles",
                DAILY_ARTICLES,
                format!("每天最多新建 {DAILY_ARTICLES} 篇文章，明天再来吧"),
            ),
            Self::Images => (
                "images",
                DAILY_IMAGES,
                format!("每天最多上传 {DAILY_IMAGES} 张图片，明天再来吧"),
            ),
            Self::Reviews => (
                "reviews",
                DAILY_REVIEWS,
                format!("每天最多由 AI 审核 {DAILY_REVIEWS} 次"),
            ),
            Self::Research(limit) => (
                "research",
                limit.into(),
                format!("今天的 {limit} 次调研已用完，明天再来吧"),
            ),
        }
    }
}

/// Counts one more for today, or refuses once the day's allowance is used, and returns the day
/// it was counted on. A new day starts every count again, and the day only moves forward: a
/// request that read the date just before midnight counts toward the new day instead of starting
/// the old one again.
async fn take<'c>(
    executor: impl sqlx::PgExecutor<'c>,
    author: i64,
    today: NaiveDate,
    quota: Quota,
) -> Result<NaiveDate> {
    let (count, limit, refusal) = quota.allowance();
    sqlx::query_scalar(
        "UPDATE authors SET
             articles = CASE WHEN day >= $2 THEN articles ELSE 0 END + ($4 = 'articles')::int,
             images = CASE WHEN day >= $2 THEN images ELSE 0 END + ($4 = 'images')::int,
             reviews = CASE WHEN day >= $2 THEN reviews ELSE 0 END + ($4 = 'reviews')::int,
             research = CASE WHEN day >= $2 THEN research ELSE 0 END + ($4 = 'research')::int,
             day = GREATEST(day, $2)
         WHERE user_id = $1 AND $3 > 0 AND (day IS NULL OR day < $2 OR CASE $4 WHEN 'articles' THEN articles
             WHEN 'images' THEN images WHEN 'reviews' THEN reviews ELSE research END < $3)
         RETURNING day",
    )
    .bind(author)
    .bind(today)
    .bind(limit)
    .bind(count)
    .fetch_optional(executor)
    .await?
    .ok_or_else(|| AppError::TooManyRequests(refusal.into()))
}

/// Gives back a picture counted on `day` that tmpfile did not keep.
async fn give_back(pool: &PgPool, author: i64, day: NaiveDate) {
    if let Err(error) = sqlx::query(
        "UPDATE authors SET images = images - 1 WHERE user_id = $1 AND day = $2 AND images > 0",
    )
    .bind(author)
    .bind(day)
    .execute(pool)
    .await
    {
        tracing::warn!(%error, author, "could not give back an unkept picture");
    }
}

/// The signed-in author's article; an administrator edits anyone's, XABC's above all.
async fn owned(state: &AppState, user: &User, id: &str) -> Result<Draft> {
    sqlx::query_as(
        "SELECT id, title, body, cover, status, revision, views, updated_at, published_at
         FROM articles WHERE id = $1 AND (author_id = $2 OR $3)",
    )
    .bind(parse(id)?)
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(missing)
}

/// Why a save matched nothing: someone else's article or research, or a revision saved since.
async fn stale(state: &AppState, user: &User, id: Uuid) -> AppError {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM articles WHERE id = $1 AND (author_id = $2 AND kind <> 'question' OR $3))",
    )
    .bind(id)
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_one(&state.pool)
    .await
    {
        Ok(true) => AppError::Conflict("文章已在别处修改，请刷新后再编辑".into()),
        Ok(false) => missing(),
        Err(error) => error.into(),
    }
}

fn parse(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id).map_err(|_| missing())
}

fn missing() -> AppError {
    AppError::NotFound("文章不存在".into())
}

/// A pen name: 2–24 characters, no spaces around it and no control characters.
fn pen_name(value: &str) -> Result<String> {
    let name = value.trim();
    if !(2..=24).contains(&name.chars().count()) || name.chars().any(char::is_control) {
        return Err(AppError::BadRequest("笔名要 2–24 个字".into()));
    }
    Ok(name.to_owned())
}

/// What a save writes.
struct Checked {
    title: String,
    body: String,
    excerpt: String,
    cover: Option<Uuid>,
}

fn checked(title: &str, body: &str, cover: Option<Uuid>) -> Result<Checked> {
    // Postgres text holds no NUL, which a model's JSON or a page's metadata may still carry.
    let (title, body) = (title.replace('\0', ""), body.replace('\0', ""));
    let title = title.trim();
    if title.chars().count() > TITLE_CHARS {
        return Err(AppError::BadRequest(
            format!("标题最多 {TITLE_CHARS} 个字").into(),
        ));
    }
    if body.chars().count() > BODY_CHARS {
        return Err(AppError::BadRequest(
            format!("正文最多 {BODY_CHARS} 个字").into(),
        ));
    }
    if pictures(&body, cover).len() > ARTICLE_IMAGES {
        return Err(AppError::BadRequest(
            format!("每篇最多 {ARTICLE_IMAGES} 张图，封面也算一张").into(),
        ));
    }
    Ok(Checked {
        title: title.to_owned(),
        excerpt: excerpt(&body),
        body,
        cover,
    })
}

/// The pictures readers see, each once: the cover, then the body's in order, found as the page
/// finds them (`site::parsed`).
fn pictures(body: &str, cover: Option<Uuid>) -> Vec<Uuid> {
    let mut shown: Vec<Uuid> = cover.into_iter().collect();
    for event in crate::site::parsed(body, MARKDOWN) {
        if let Event::Start(Tag::Image { dest_url, .. }) = event
            && let Some(id) = image_id(&dest_url)
            && !shown.contains(&id)
        {
            shown.push(id);
        }
    }
    shown
}

fn image_id(destination: &str) -> Option<Uuid> {
    destination
        .strip_prefix(IMAGE)
        .and_then(|id| Uuid::parse_str(id).ok())
}

/// The body's words as one line of plain text: no markup, no picture descriptions and no
/// captions, the italic line right under a picture (`reads.css` sets it apart as `img + em`);
/// formulas as `site::formula_text` writes them.
fn words(body: &str) -> String {
    let mut text = String::new();
    // Inside a picture, how deep inside its caption's emphasis, and whether a picture just ended,
    // a line break apart.
    let (mut picture, mut caption, mut pictured) = (false, 0, false);
    for event in crate::site::bolded(crate::site::parsed(body, MARKDOWN)) {
        match event {
            Event::Start(Tag::Image { .. }) => picture = true,
            Event::End(TagEnd::Image) => {
                picture = false;
                pictured = true;
                continue;
            }
            Event::Start(Tag::Emphasis) if pictured || caption > 0 => caption += 1,
            Event::End(TagEnd::Emphasis) if caption > 0 => caption -= 1,
            Event::Text(part) | Event::Code(part) if !picture && caption == 0 => {
                text.push_str(&part)
            }
            Event::InlineMath(tex) if !picture && caption == 0 => {
                text.push_str(&crate::site::formula_text(&tex, false))
            }
            Event::DisplayMath(tex) if !picture && caption == 0 => {
                text.push_str(&crate::site::formula_text(&tex, true))
            }
            Event::SoftBreak | Event::HardBreak => {
                text.push(' ');
                continue;
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock
                | TagEnd::Item
                | TagEnd::TableCell,
            ) => text.push(' '),
            _ => {}
        }
        pictured = false;
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The start of the body's words, for lists.
fn excerpt(body: &str) -> String {
    let line = words(body);
    if line.chars().count() <= EXCERPT_CHARS {
        return line;
    }
    let cut: String = line.chars().take(EXCERPT_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// A picture's type, read from its bytes, and its size: JPEG, PNG, WebP or GIF only. The bytes
/// are kept as uploaded; tmpfile serves them with this type and `nosniff`.
fn image_kind(bytes: &[u8]) -> Result<(&'static str, (i32, i32))> {
    let content_type = match image::guess_format(bytes) {
        Ok(image::ImageFormat::Jpeg) => "image/jpeg",
        Ok(image::ImageFormat::Png) => "image/png",
        Ok(image::ImageFormat::WebP) => "image/webp",
        Ok(image::ImageFormat::Gif) => "image/gif",
        _ => {
            return Err(AppError::UnsupportedMediaType(
                "只支持 JPEG、PNG、WebP 和 GIF 图片".into(),
            ));
        }
    };
    let size = crate::media::image_dimensions(bytes)
        .and_then(|(width, height)| Some((i32::try_from(width).ok()?, i32::try_from(height).ok()?)))
        .filter(|(width, height)| *width > 0 && *height > 0)
        .ok_or_else(|| AppError::BadRequest("这张图片读不出来".into()))?;
    Ok((content_type, size))
}
