//! 分享: articles readers write in the workspace. Authors write under a pen name. An article is
//! public once an administrator gives it a ❤️, and an author's edit takes it back to review.
//! Pictures go to tmpfile as they were uploaded; their links appear only on public pages, and
//! until then the author sees them through this server. A read counts once a day per browser,
//! never the author's own. XABC, the site's own pen name, publishes one every hour (`xabc`).
mod pages;
pub(crate) mod xabc;

#[cfg(test)]
mod tests;

use crate::{
    auth::{self, User},
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
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// Articles an author may start in a day, by the site's date.
const DAILY_ARTICLES: i16 = 10;
/// Pictures an author may upload in a day.
const DAILY_IMAGES: i16 = 64;
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
/// Where 时代 lists XABC's profiles of the people this age talks about.
pub(crate) const PEOPLE: &str = "/people";

/// 文娱 · 解说's name, in the page's language.
pub(crate) fn commentary_label(en: bool) -> &'static str {
    crate::site::choose(en, "解说", "𝑬𝒙𝒑𝒍𝒂𝒊𝒏𝒆𝒅")
}

/// 时代's name, in the page's language.
pub(crate) fn people_label(en: bool) -> &'static str {
    crate::site::choose(en, "时代", "𝑷𝒆𝒐𝒑𝒍𝒆")
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
        .route("/api/articles/{id}/views", post(view))
        .route("/api/articles/views", get(views))
        .route("/api/admin/articles", get(queue))
        .route("/api/admin/articles/{id}", patch(review))
        .merge(pages::router())
}

/// An article in the author's list or the review queue.
#[derive(FromRow, Serialize)]
struct Listed {
    id: Uuid,
    title: String,
    author: String,
    status: String,
    revision: i32,
    views: i64,
    updated_at: DateTime<Utc>,
    published_at: Option<DateTime<Utc>>,
    /// Whether it is a 解说.
    film: bool,
    /// Whether it is a 时代 profile.
    person: bool,
}

const LISTED: &str = "SELECT a.id, a.title, w.name AS author, a.status, a.revision, a.views, a.updated_at, a.published_at, a.film IS NOT NULL AS film, a.person IS NOT NULL AS person
    FROM articles a JOIN authors w ON w.user_id = a.author_id JOIN users u ON u.id = a.author_id";

/// A search of 分享, 解说 or 时代: its words, as the page shows them, and the patterns each of
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
}

#[derive(FromRow, Serialize)]
struct Uploaded {
    id: Uuid,
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

/// The signed-in reader's pen name and articles, newest edit first.
async fn mine(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM authors WHERE user_id = $1")
        .bind(user.id)
        .fetch_optional(&state.pool)
        .await?;
    let articles: Vec<Listed> = sqlx::QueryBuilder::new(LISTED)
        .push(" WHERE a.author_id = ")
        .push_bind(user.id)
        .push(" ORDER BY a.updated_at DESC, a.id DESC")
        .build_query_as()
        .fetch_all(&state.pool)
        .await?;
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
    Ok(Json(owned(&state, &user, &id).await?))
}

/// Saves the revision the editor holds. An author's edit takes a pending article back to being
/// a draft and a public one back to review; an administrator's leaves its status as it was.
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
         WHERE id = $1 AND (author_id = $2 OR $8) AND revision = $3
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
    let (status, revision, updated_at) = match saved {
        Some(saved) => saved,
        None => return Err(stale(&state, &user, id).await),
    };
    Ok(Json(
        json!({ "status": status, "revision": revision, "updated_at": updated_at }),
    ))
}

/// Sends a draft to review; an administrator's own goes straight out.
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
    let (status, revision, published_at) =
        submitted.ok_or_else(|| AppError::Conflict("文章已修改或已提交，请刷新后再试".into()))?;
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
    let (content_type, (width, height)) = kind(&bytes)?;
    if !sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM articles WHERE id = $1 AND (author_id = $2 OR $3))",
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
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id, width, height",
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

/// An administrator's ❤️ publishes the revision they read; taking it back returns the article
/// to review.
async fn review(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Review>,
) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let sql = if request.published {
        "UPDATE articles SET status = 'published', published_at = COALESCE(published_at, now())
         WHERE id = $1 AND revision = $2 AND status = 'pending' RETURNING status, revision"
    } else {
        "UPDATE articles SET status = 'pending'
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

#[derive(Clone, Copy)]
enum Quota {
    Articles,
    Images,
}

/// Counts one more article or picture for today, or refuses once the day's allowance is used,
/// and returns the day it was counted on. The day only moves forward: a request that read the
/// date just before midnight counts toward the new day instead of starting the old one again.
async fn take<'c>(
    executor: impl sqlx::PgExecutor<'c>,
    author: i64,
    today: NaiveDate,
    quota: Quota,
) -> Result<NaiveDate> {
    let (sql, limit, refusal) = match quota {
        Quota::Articles => (
            "UPDATE authors SET articles = CASE WHEN day >= $2 THEN articles + 1 ELSE 1 END,
                 images = CASE WHEN day >= $2 THEN images ELSE 0 END, day = GREATEST(day, $2)
             WHERE user_id = $1 AND (day IS NULL OR day < $2 OR articles < $3) RETURNING day",
            DAILY_ARTICLES,
            format!("每天最多新建 {DAILY_ARTICLES} 篇文章，明天再来吧"),
        ),
        Quota::Images => (
            "UPDATE authors SET images = CASE WHEN day >= $2 THEN images + 1 ELSE 1 END,
                 articles = CASE WHEN day >= $2 THEN articles ELSE 0 END, day = GREATEST(day, $2)
             WHERE user_id = $1 AND (day IS NULL OR day < $2 OR images < $3) RETURNING day",
            DAILY_IMAGES,
            format!("每天最多上传 {DAILY_IMAGES} 张图片，明天再来吧"),
        ),
    };
    sqlx::query_scalar(sql)
        .bind(author)
        .bind(today)
        .bind(limit)
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

/// Why a save matched nothing: someone else's article, or a revision saved since.
async fn stale(state: &AppState, user: &User, id: Uuid) -> AppError {
    match sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM articles WHERE id = $1 AND (author_id = $2 OR $3))",
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
    let mut shown = pictures(body);
    if let Some(cover) = cover.filter(|cover| !shown.contains(cover)) {
        shown.push(cover);
    }
    if shown.len() > ARTICLE_IMAGES {
        return Err(AppError::BadRequest(
            format!("每篇最多 {ARTICLE_IMAGES} 张图，封面也算一张").into(),
        ));
    }
    Ok(Checked {
        title: title.to_owned(),
        body: body.to_owned(),
        excerpt: excerpt(body),
        cover,
    })
}

/// The pictures the body shows, each once, in order.
fn pictures(body: &str) -> Vec<Uuid> {
    let mut shown = Vec::new();
    for event in Parser::new_ext(body, MARKDOWN) {
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
/// captions, the italic line right under a picture (`reads.css` sets it apart as `img + em`).
fn words(body: &str) -> String {
    let mut text = String::new();
    // Inside a picture or its caption, and whether a picture just ended, a line break apart.
    let (mut picture, mut caption, mut pictured) = (false, false, false);
    for event in crate::site::bolded(Parser::new_ext(body, MARKDOWN)) {
        match event {
            Event::Start(Tag::Image { .. }) => picture = true,
            Event::End(TagEnd::Image) => {
                picture = false;
                pictured = true;
                continue;
            }
            Event::Start(Tag::Emphasis) if pictured => caption = true,
            Event::End(TagEnd::Emphasis) if caption => caption = false,
            Event::Text(part) | Event::Code(part) if !picture && !caption => text.push_str(&part),
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
fn kind(bytes: &[u8]) -> Result<(&'static str, (i32, i32))> {
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

/// Whether the browser already holds `tag`, as `If-None-Match` says.
fn unchanged(headers: &HeaderMap, tag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate.strip_prefix("W/").unwrap_or(candidate) == tag
            })
        })
}
