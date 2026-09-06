use crate::{
    auth::{self, User},
    error::{ApiQuery, AppError, Result},
    state::AppState,
};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{
        Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicU64, Ordering},
    },
};
use tower_http::compression::CompressionLayer;
use uuid::Uuid;

const DEFAULT_LIMIT: usize = 30;
const MAX_LIMIT: usize = 128;
const MAX_DAYS: i64 = 366;
const FEED_LIMIT: usize = 50;
/// Today and yesterday live in memory and in Redis for two days; archive days only pass
/// through Redis briefly.
const HOT_TTL_SECS: u64 = 48 * 3600;
const COLD_TTL_SECS: u64 = 6 * 3600;
const PUBLIC_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=60, stale-while-revalidate=300");

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct TopicMeta {
    pub id: String,
    pub name: String,
    pub name_en: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PublicItem {
    pub id: Uuid,
    pub title: String,
    pub summary: String,
    pub title_en: String,
    pub summary_en: String,
    pub url: String,
    pub source: String,
    pub published_at: Option<DateTime<Utc>>,
    pub first_seen_at: DateTime<Utc>,
    pub tags: Vec<String>,
    pub importance: i16,
    pub pinned: bool,
}

/// One topic on one day: the visible items in display order. This is the unit that moves
/// between PostgreSQL, Redis and memory.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub updated_at: Option<DateTime<Utc>>,
    pub items: Vec<PublicItem>,
}

/// A response body with its validator, ready to serve.
pub struct Rendered {
    pub body: Bytes,
    pub etag: String,
}

impl Rendered {
    pub(crate) fn new(body: String) -> Self {
        Self {
            etag: entity_tag(&body),
            body: Bytes::from(body),
        }
    }
}

/// Memory layer of the digest. Reads fall through memory → Redis → PostgreSQL and fill the
/// layers above on the way back; writes go PostgreSQL → Redis → memory.
#[derive(Clone)]
pub struct DigestCache(Arc<Inner>);

struct Inner {
    tz: Tz,
    topics: RwLock<Arc<Vec<TopicMeta>>>,
    days: RwLock<HashMap<(String, NaiveDate), Arc<Snapshot>>>,
    /// PostgreSQL's answer for topics with nothing in the last two days, until the next write.
    latest: RwLock<HashMap<String, Option<NaiveDate>>>,
    /// The default `/digest` response, pre-rendered until the next write.
    hot: RwLock<Option<Arc<Rendered>>>,
    /// Bumped by every write. A load that began before a write must not be stored after it.
    generation: AtomicU64,
    /// Serializes cache misses and explicit refreshes. Without this small
    /// critical section, two concurrent refreshes could both observe a newer
    /// generation and leave Redis with neither snapshot.
    cache_write: tokio::sync::Mutex<()>,
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

impl DigestCache {
    pub fn new(tz: Tz) -> Self {
        Self(Arc::new(Inner {
            tz,
            topics: RwLock::default(),
            days: RwLock::default(),
            latest: RwLock::default(),
            hot: RwLock::default(),
            generation: AtomicU64::new(0),
            cache_write: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn today(&self) -> NaiveDate {
        Utc::now().with_timezone(&self.0.tz).date_naive()
    }

    fn yesterday(&self) -> NaiveDate {
        self.today().pred_opt().expect("date in range")
    }

    fn is_recent(&self, day: NaiveDate) -> bool {
        day == self.today() || day == self.yesterday()
    }

    pub fn topics(&self) -> Arc<Vec<TopicMeta>> {
        Arc::clone(&read(&self.0.topics))
    }

    pub async fn reload_topics(&self, state: &AppState) -> Result<()> {
        let _cache_guard = self.0.cache_write.lock().await;
        let topics = sqlx::query_as::<_, TopicMeta>(
            "SELECT id, name, name_en, description FROM topics WHERE enabled AND owner_user_id IS NULL ORDER BY sort_weight, created_at",
        )
        .fetch_all(&state.pool)
        .await?;
        // Invalidate in-memory snapshots together with the topic catalogue.  This
        // matters when an administrator disables and later re-enables a topic:
        // the next request must re-read its current items.
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        *write(&self.0.topics) = Arc::new(topics);
        write(&self.0.days).clear();
        write(&self.0.latest).clear();
        *write(&self.0.hot) = None;
        // A reader may have started in the small window between the first
        // fence and the catalogue swap; fence that reader as well.
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// Fills memory for every enabled topic so the first request already hits it.
    pub async fn warm(&self, state: &AppState) -> Result<()> {
        self.reload_topics(state).await?;
        for topic in self.topics().iter() {
            for day in [self.today(), self.yesterday()] {
                self.snapshot(state, &topic.id, day).await?;
            }
        }
        Ok(())
    }

    pub async fn snapshot(
        &self,
        state: &AppState,
        topic: &str,
        day: NaiveDate,
    ) -> Result<Arc<Snapshot>> {
        let key = (topic.to_owned(), day);
        if let Some(hit) = read(&self.0.days).get(&key).cloned() {
            return Ok(hit);
        }
        let _cache_guard = self.0.cache_write.lock().await;
        // Another miss may have filled memory while this request waited for
        // the writer section.
        if let Some(hit) = read(&self.0.days).get(&key).cloned() {
            return Ok(hit);
        }
        let generation = self.0.generation.load(Ordering::Acquire);
        let snapshot = match redis_get(state, topic, day).await {
            Some(snapshot) => snapshot,
            None => {
                let snapshot = load(state, topic, day).await?;
                // A write may have invalidated this read while it was in
                // flight.  Do not repopulate Redis with the old snapshot.
                if self.0.generation.load(Ordering::Acquire) == generation {
                    redis_set(state, topic, day, &snapshot, self.ttl(day)).await;
                }
                snapshot
            }
        };
        let snapshot = Arc::new(snapshot);
        if self.is_recent(day) {
            let mut days = write(&self.0.days);
            if self.0.generation.load(Ordering::Acquire) == generation {
                days.insert(key, Arc::clone(&snapshot));
            }
        }
        Ok(snapshot)
    }

    /// Rebuilds one topic-day from PostgreSQL after a write and pushes it through Redis
    /// into memory.
    pub async fn refresh(&self, state: &AppState, topic: &str, day: NaiveDate) -> Result<()> {
        let _cache_guard = self.0.cache_write.lock().await;
        let visible = sqlx::query_scalar::<_, bool>(
            "SELECT enabled FROM topics WHERE id = $1 AND owner_user_id IS NULL",
        )
        .bind(topic)
        .fetch_optional(&state.pool)
        .await?;
        if !visible.unwrap_or(false) {
            // Disabled/deleted topics must not leave an empty snapshot behind
            // that would hide fresh items when the topic is enabled again.
            self.forget_topic_locked(state, topic).await;
            return Ok(());
        }
        // Fence snapshots that started before this refresh.  The second bump
        // below fences reads that begin while PostgreSQL is being loaded.
        let generation = self.0.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let snapshot = Arc::new(load(state, topic, day).await?);
        if self.0.generation.load(Ordering::Acquire) == generation {
            redis_set(state, topic, day, &snapshot, self.ttl(day)).await;
        }
        let final_generation = self.0.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if self.is_recent(day) && self.0.generation.load(Ordering::Acquire) == final_generation {
            write(&self.0.days).insert((topic.to_owned(), day), snapshot);
        }
        write(&self.0.latest).remove(topic);
        *write(&self.0.hot) = None;
        Ok(())
    }

    pub async fn forget_topic(&self, state: &AppState, topic: &str) {
        let _cache_guard = self.0.cache_write.lock().await;
        self.forget_topic_locked(state, topic).await;
    }

    /// Drops cached archive snapshots that are no longer present in PostgreSQL after
    /// retention pruning. The memory generation fence prevents an in-flight read from
    /// repopulating a deleted day after this invalidation.
    pub(crate) async fn prune_before(&self, state: &AppState, keep_from: NaiveDate) -> Result<()> {
        let _cache_guard = self.0.cache_write.lock().await;
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        write(&self.0.days).retain(|(_, day), _| *day >= keep_from);
        write(&self.0.latest).clear();
        *write(&self.0.hot) = None;
        let result = if let Some(mut redis) = state.redis.clone() {
            redis_delete_before(&mut redis, keep_from).await
        } else {
            Ok(())
        };
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        result.map_err(Into::into)
    }

    async fn forget_topic_locked(&self, state: &AppState, topic: &str) {
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        write(&self.0.days).retain(|(id, _), _| id != topic);
        write(&self.0.latest).remove(topic);
        *write(&self.0.hot) = None;
        // A deleted topic may be recreated with the same id. Remove every
        // archive key as well as today's hot keys, otherwise stale items can
        // survive their normal Redis TTL.
        if let Some(mut redis) = state.redis.clone()
            && let Err(error) = redis_delete_topic(&mut redis, topic).await
        {
            tracing::warn!(%error, topic, "redis topic invalidation failed");
        }
        self.0.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Runs once the local date changes: the day before yesterday leaves memory.
    pub fn rollover(&self) {
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        let keep_from = self.yesterday();
        write(&self.0.days).retain(|(_, day), _| *day >= keep_from);
        write(&self.0.latest).clear();
        *write(&self.0.hot) = None;
    }

    /// The most recent day with visible items for the topic.
    pub async fn latest_day(&self, state: &AppState, topic: &str) -> Result<Option<NaiveDate>> {
        for day in [self.today(), self.yesterday()] {
            if !self.snapshot(state, topic, day).await?.items.is_empty() {
                return Ok(Some(day));
            }
        }
        if let Some(cached) = read(&self.0.latest).get(topic).copied() {
            return Ok(cached);
        }
        let generation = self.0.generation.load(Ordering::Acquire);
        let day = sqlx::query_scalar::<_, Option<NaiveDate>>(
            "SELECT MAX(items.day) FROM items JOIN topics ON topics.id = items.topic_id WHERE items.topic_id = $1 AND topics.enabled AND topics.owner_user_id IS NULL AND NOT items.hidden",
        )
        .bind(topic)
        .fetch_one(&state.pool)
        .await?;
        if self.0.generation.load(Ordering::Acquire) == generation {
            write(&self.0.latest).insert(topic.to_owned(), day);
        }
        Ok(day)
    }

    fn ttl(&self, day: NaiveDate) -> u64 {
        if self.is_recent(day) {
            HOT_TTL_SECS
        } else {
            COLD_TTL_SECS
        }
    }

    fn selected(&self, topic: Option<&str>) -> Result<(Arc<Vec<TopicMeta>>, Vec<usize>)> {
        let topics = self.topics();
        let selected = match topic {
            Some(id) => vec![
                topics
                    .iter()
                    .position(|topic| topic.id == id)
                    .ok_or_else(|| AppError::NotFound("主题不存在".into()))?,
            ],
            None => (0..topics.len()).collect(),
        };
        Ok((topics, selected))
    }

    /// The `/digest` body. Without a day, the most recent day that has content anywhere in
    /// the selection is used, so a page never mixes days.
    pub async fn render(
        &self,
        state: &AppState,
        topic: Option<&str>,
        day: Option<NaiveDate>,
        limit: usize,
    ) -> Result<Arc<Rendered>> {
        let hot_path = topic.is_none() && day.is_none() && limit == DEFAULT_LIMIT;
        if hot_path && let Some(hit) = read(&self.0.hot).clone() {
            return Ok(hit);
        }
        let generation = self.0.generation.load(Ordering::Acquire);
        let (topics, selected) = self.selected(topic)?;
        let mut latest = Vec::with_capacity(selected.len());
        for &index in &selected {
            latest.push(self.latest_day(state, &topics[index].id).await?);
        }
        let day = day.or_else(|| latest.iter().flatten().max().copied());
        let mut snapshots = Vec::with_capacity(selected.len());
        for &index in &selected {
            snapshots.push(match day {
                Some(day) => Some(self.snapshot(state, &topics[index].id, day).await?),
                None => None,
            });
        }
        let body = DigestBody {
            day,
            timezone: self.0.tz.name(),
            generated_at: Utc::now(),
            topics: selected
                .iter()
                .zip(&latest)
                .zip(&snapshots)
                .map(|((&index, latest_day), snapshot)| {
                    let topic = &topics[index];
                    let items = snapshot.as_ref().map_or(&[][..], |s| s.items.as_slice());
                    TopicDigest {
                        id: &topic.id,
                        name: &topic.name,
                        name_en: &topic.name_en,
                        description: &topic.description,
                        latest_day: *latest_day,
                        updated_at: snapshot.as_ref().and_then(|s| s.updated_at),
                        total: items.len(),
                        items: &items[..items.len().min(limit)],
                    }
                })
                .collect(),
        };
        let rendered = Arc::new(Rendered::new(serde_json::to_string(&body)?));
        if hot_path && self.0.generation.load(Ordering::Acquire) == generation {
            *write(&self.0.hot) = Some(Arc::clone(&rendered));
        }
        Ok(rendered)
    }
}

#[derive(Serialize)]
struct DigestBody<'a> {
    day: Option<NaiveDate>,
    timezone: &'a str,
    generated_at: DateTime<Utc>,
    topics: Vec<TopicDigest<'a>>,
}

#[derive(Serialize)]
struct TopicDigest<'a> {
    id: &'a str,
    name: &'a str,
    name_en: &'a str,
    description: &'a str,
    latest_day: Option<NaiveDate>,
    updated_at: Option<DateTime<Utc>>,
    total: usize,
    items: &'a [PublicItem],
}

async fn load(state: &AppState, topic: &str, day: NaiveDate) -> Result<Snapshot> {
    let items = sqlx::query_as::<_, PublicItem>(
        "SELECT items.id, items.title, items.summary, items.title_en, items.summary_en, items.url, items.source, items.published_at, items.first_seen_at, items.tags, items.importance, items.pinned \
         FROM items JOIN topics ON topics.id = items.topic_id \
         WHERE items.topic_id = $1 AND items.day = $2 AND topics.enabled AND topics.owner_user_id IS NULL AND NOT items.hidden \
         ORDER BY pinned DESC, published_at DESC NULLS LAST, first_seen_at DESC",
    )
    .bind(topic)
    .bind(day)
    .fetch_all(&state.pool)
    .await?;
    Ok(Snapshot {
        updated_at: items.iter().map(|item| item.first_seen_at).max(),
        items,
    })
}

fn redis_key(topic: &str, day: NaiveDate) -> String {
    format!("news:v1:{topic}:{day}")
}

async fn redis_get(state: &AppState, topic: &str, day: NaiveDate) -> Option<Snapshot> {
    let mut redis = state.redis.clone()?;
    match redis::cmd("GET")
        .arg(redis_key(topic, day))
        .query_async::<Option<String>>(&mut redis)
        .await
    {
        Ok(value) => value.and_then(|json| serde_json::from_str(&json).ok()),
        Err(error) => {
            tracing::warn!(%error, "redis read failed; falling back to PostgreSQL");
            None
        }
    }
}

async fn redis_set(state: &AppState, topic: &str, day: NaiveDate, snapshot: &Snapshot, ttl: u64) {
    let Some(mut redis) = state.redis.clone() else {
        return;
    };
    let json = serde_json::to_string(snapshot).expect("snapshot serialises");
    if let Err(error) = redis::cmd("SET")
        .arg(redis_key(topic, day))
        .arg(json)
        .arg("EX")
        .arg(ttl)
        .query_async::<()>(&mut redis)
        .await
    {
        tracing::warn!(%error, "redis write failed; memory and PostgreSQL stay authoritative");
    }
}

async fn redis_delete_topic(
    redis: &mut redis::aio::ConnectionManager,
    topic: &str,
) -> std::result::Result<(), redis::RedisError> {
    let pattern = format!("news:v1:{topic}:*");
    let mut cursor = 0_u64;
    loop {
        let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(1_000_u16)
            .query_async(redis)
            .await?;
        if !keys.is_empty() {
            let _: i64 = redis::cmd("DEL").arg(keys).query_async(redis).await?;
        }
        if next_cursor == 0 {
            return Ok(());
        }
        cursor = next_cursor;
    }
}

async fn redis_delete_before(
    redis: &mut redis::aio::ConnectionManager,
    keep_from: NaiveDate,
) -> std::result::Result<(), redis::RedisError> {
    let mut cursor = 0_u64;
    loop {
        let (next_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg("news:v1:*")
            .arg("COUNT")
            .arg(1_000_u16)
            .query_async(redis)
            .await?;
        let expired = keys
            .into_iter()
            .filter(|key| {
                key.rsplit_once(':')
                    .and_then(|(_, day)| day.parse::<NaiveDate>().ok())
                    .is_some_and(|day| day < keep_from)
            })
            .collect::<Vec<_>>();
        if !expired.is_empty() {
            let _: i64 = redis::cmd("DEL").arg(expired).query_async(redis).await?;
        }
        if next_cursor == 0 {
            return Ok(());
        }
        cursor = next_cursor;
    }
}

// ---------------------------------------------------------------------------
// Public routes
// ---------------------------------------------------------------------------

pub(crate) fn router() -> Router<AppState> {
    // Keep the authenticated feed outside the public-CORS layer.  Its response contains
    // account-owned topics and must never be readable by an arbitrary cross-origin page.
    let public = Router::new()
        .route("/digest", get(digest))
        .route("/insight", get(crate::insight::latest))
        .route("/topics", get(topics))
        .route("/days", get(days))
        .route("/feed.xml", get(feed))
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(public_cors));
    let personal = Router::new()
        .route("/api/me/feed", get(me_feed))
        .layer(CompressionLayer::new());
    public.merge(personal)
}

/// Applied outside the handlers so failures are readable cross-origin too; without it a
/// third-party `fetch` sees an opaque network error instead of the JSON error body.
async fn public_cors(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

#[derive(Debug, Deserialize)]
struct DigestQuery {
    topic: Option<String>,
    day: Option<NaiveDate>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TopicQuery {
    topic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DaysQuery {
    topic: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize, FromRow)]
struct DayCount {
    day: NaiveDate,
    count: i64,
}

/// The small catalogue used by the authenticated home page. It intentionally contains
/// only display data and aggregate counts; ownership is enforced by the query and is never
/// serialized into the response.
#[derive(Debug, Serialize, FromRow)]
struct PersonalTopicCard {
    id: String,
    name: String,
    name_en: String,
    description: String,
    latest_day: Option<NaiveDate>,
    today_count: i64,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct PersonalDayStats {
    topic_id: String,
    total: i64,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct PersonalFeedResponse {
    topics: Vec<PersonalTopicCard>,
    days: Vec<DayCount>,
    digest: PersonalDigest,
}

#[derive(Debug, Serialize)]
struct PersonalDigest {
    day: Option<NaiveDate>,
    timezone: String,
    generated_at: DateTime<Utc>,
    topics: Vec<PersonalTopicDigest>,
}

#[derive(Debug, Serialize)]
struct PersonalTopicDigest {
    id: String,
    name: String,
    name_en: String,
    description: String,
    latest_day: Option<NaiveDate>,
    updated_at: Option<DateTime<Utc>>,
    total: usize,
    items: Vec<PublicItem>,
}

async fn digest(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<DigestQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let rendered = state
        .digest
        .render(&state, query.topic.as_deref(), query.day, limit)
        .await?;
    Ok(conditional(&headers, &rendered, "application/json"))
}

async fn topics(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let cache = &state.digest;
    let today = cache.today();
    let mut list = Vec::new();
    for topic in cache.topics().iter() {
        let snapshot = cache.snapshot(&state, &topic.id, today).await?;
        list.push(json!({
            "id": topic.id,
            "name": topic.name,
            "name_en": topic.name_en,
            "description": topic.description,
            "latest_day": cache.latest_day(&state, &topic.id).await?,
            "today_count": snapshot.items.len(),
            "updated_at": snapshot.updated_at,
        }));
    }
    let rendered = Rendered::new(json!({ "topics": list }).to_string());
    Ok(conditional(&headers, &rendered, "application/json"))
}

async fn days(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<DaysQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let days = sqlx::query_as::<_, DayCount>(
        "SELECT items.day, COUNT(*) AS count FROM items \
         JOIN topics ON topics.id = items.topic_id \
         WHERE ($1::text IS NULL OR items.topic_id = $1) AND topics.enabled AND topics.owner_user_id IS NULL AND NOT items.hidden \
         GROUP BY items.day ORDER BY items.day DESC LIMIT $2",
    )
    .bind(&query.topic)
    .bind(query.limit.unwrap_or(30).clamp(1, MAX_DAYS))
    .fetch_all(&state.pool)
    .await?;
    let rendered = Rendered::new(json!({ "topic": query.topic, "days": days }).to_string());
    Ok(conditional(&headers, &rendered, "application/json"))
}

/// Returns the public catalogue plus the current user's enabled topics together with a
/// digest for that selection. Public rows are intentionally included here because the
/// authenticated home page is a personalized view of the public feed, not a replacement
/// for it.
///
/// This deliberately bypasses `DigestCache`: its Redis and in-memory keys are shared by
/// the public catalogue, so putting an owner dimension there would make a small feature
/// surprisingly easy to leak across accounts. The combined selection is small (the public
/// catalogue plus at most eight personal topics on Pro) and cheap to read directly from
/// PostgreSQL with a private, no-store response.
async fn me_feed(
    State(state): State<AppState>,
    user: User,
    ApiQuery(query): ApiQuery<DigestQuery>,
) -> Result<Response> {
    auth::require_personal(&user)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let cards = personal_topic_cards(&state, user.id).await?;

    // `/api/me/feed?topic=...` is a view over the public catalogue plus the caller's
    // enabled topics. An id outside that set is reported as not found; private topics owned
    // by another account never enter `cards`, so this does not create an existence oracle.
    let selected = match query.topic.as_deref() {
        Some(id) => vec![
            cards
                .iter()
                .position(|topic| topic.id == id)
                .ok_or_else(|| AppError::NotFound("主题不存在".into()))?,
        ],
        None => (0..cards.len()).collect(),
    };

    let days = personal_days(&state, user.id, query.topic.as_deref()).await?;
    // As with `/digest`, an omitted day means the newest day that has content in the
    // selection.  An explicit empty day is retained so the UI can render its empty state.
    let selected_day = query.day.or_else(|| days.first().map(|entry| entry.day));
    let stats = match selected_day {
        Some(day) => personal_day_stats(&state, user.id, query.topic.as_deref(), day).await?,
        None => Vec::new(),
    };
    let stats = stats
        .into_iter()
        .map(|entry| (entry.topic_id.clone(), entry))
        .collect::<std::collections::HashMap<_, _>>();

    let mut digest_topics = Vec::with_capacity(selected.len());
    for index in selected {
        let card = &cards[index];
        let (items, total, updated_at) = match selected_day {
            Some(day) => (
                personal_items(&state, user.id, &card.id, day, limit).await?,
                stats.get(&card.id).map_or(0, |entry| entry.total),
                stats.get(&card.id).and_then(|entry| entry.updated_at),
            ),
            None => (Vec::new(), 0, None),
        };
        digest_topics.push(PersonalTopicDigest {
            id: card.id.clone(),
            name: card.name.clone(),
            name_en: card.name_en.clone(),
            description: card.description.clone(),
            latest_day: card.latest_day,
            updated_at,
            total: usize::try_from(total).unwrap_or(usize::MAX),
            items,
        });
    }

    let payload = PersonalFeedResponse {
        topics: cards,
        days,
        digest: PersonalDigest {
            day: selected_day,
            timezone: state.config.timezone.name().to_owned(),
            generated_at: Utc::now(),
            topics: digest_topics,
        },
    };
    let mut response = axum::Json(payload).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::VARY, HeaderValue::from_static("Cookie"));
    Ok(response)
}

async fn personal_topic_cards(state: &AppState, user_id: i64) -> Result<Vec<PersonalTopicCard>> {
    Ok(sqlx::query_as::<_, PersonalTopicCard>(
        r#"
        SELECT
            topics.id,
            topics.name,
            topics.name_en,
            topics.description,
            MAX(items.day) AS latest_day,
            COUNT(items.id) FILTER (WHERE items.day = $2) AS today_count,
            MAX(items.first_seen_at) FILTER (WHERE items.day = $2) AS updated_at
        FROM topics
        LEFT JOIN items
          ON items.topic_id = topics.id
         AND NOT items.hidden
        WHERE topics.enabled
          AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $1)
        GROUP BY topics.id, topics.name, topics.name_en, topics.description,
                 topics.owner_user_id, topics.sort_weight, topics.created_at
        ORDER BY topics.owner_user_id NULLS FIRST,
                 topics.sort_weight, topics.created_at, topics.id
        "#,
    )
    .bind(user_id)
    .bind(state.digest.today())
    .fetch_all(&state.pool)
    .await?)
}

async fn personal_days(
    state: &AppState,
    user_id: i64,
    topic: Option<&str>,
) -> Result<Vec<DayCount>> {
    Ok(sqlx::query_as::<_, DayCount>(
        r#"
        SELECT items.day, COUNT(*) AS count
        FROM items
        JOIN topics ON topics.id = items.topic_id
        WHERE topics.enabled
          AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $1)
          AND NOT items.hidden
          AND ($2::text IS NULL OR items.topic_id = $2)
        GROUP BY items.day
        ORDER BY items.day DESC
        LIMIT $3
        "#,
    )
    .bind(user_id)
    .bind(topic)
    .bind(MAX_DAYS)
    .fetch_all(&state.pool)
    .await?)
}

async fn personal_day_stats(
    state: &AppState,
    user_id: i64,
    topic: Option<&str>,
    day: NaiveDate,
) -> Result<Vec<PersonalDayStats>> {
    Ok(sqlx::query_as::<_, PersonalDayStats>(
        r#"
        SELECT items.topic_id,
               COUNT(*) AS total,
               MAX(items.first_seen_at) AS updated_at
        FROM items
        JOIN topics ON topics.id = items.topic_id
        WHERE topics.enabled
          AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $1)
          AND NOT items.hidden
          AND items.day = $2
          AND ($3::text IS NULL OR items.topic_id = $3)
        GROUP BY items.topic_id
        "#,
    )
    .bind(user_id)
    .bind(day)
    .bind(topic)
    .fetch_all(&state.pool)
    .await?)
}

async fn personal_items(
    state: &AppState,
    user_id: i64,
    topic: &str,
    day: NaiveDate,
    limit: usize,
) -> Result<Vec<PublicItem>> {
    Ok(sqlx::query_as::<_, PublicItem>(
        r#"
        SELECT items.id,
               items.title,
               items.summary,
               items.title_en,
               items.summary_en,
               items.url,
               items.source,
               items.published_at,
               items.first_seen_at,
               items.tags,
               items.importance,
               items.pinned
        FROM items
        JOIN topics ON topics.id = items.topic_id
        WHERE items.topic_id = $1
          AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $2)
          AND topics.enabled
          AND items.day = $3
          AND NOT items.hidden
        ORDER BY items.pinned DESC,
                 items.published_at DESC NULLS LAST,
                 items.first_seen_at DESC
        LIMIT $4
        "#,
    )
    .bind(topic)
    .bind(user_id)
    .bind(day)
    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
    .fetch_all(&state.pool)
    .await?)
}

/// Atom 1.0 over the last two days of the selection, newest collected first.
async fn feed(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<TopicQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let cache = &state.digest;
    let (topics, selected) = cache.selected(query.topic.as_deref())?;
    let mut entries = Vec::new();
    for &index in &selected {
        for day in [cache.today(), cache.yesterday()] {
            let snapshot = cache.snapshot(&state, &topics[index].id, day).await?;
            entries.extend(snapshot.items.iter().cloned().map(|item| (item, index)));
        }
    }
    entries.sort_by_key(|(item, _)| std::cmp::Reverse(item.first_seen_at));
    entries.truncate(FEED_LIMIT);

    let base = state.config.public_base_url.as_str();
    let title = match query.topic {
        Some(_) => format!("{} · NEWS", topics[selected[0]].name),
        None => "NEWS".to_owned(),
    };
    let updated = entries
        .first()
        .map_or_else(Utc::now, |(item, _)| item.first_seen_at);
    let mut xml = String::with_capacity(entries.len() * 700 + 400);
    let _ = write!(
        xml,
        r#"<?xml version="1.0" encoding="utf-8"?><feed xmlns="http://www.w3.org/2005/Atom" xml:lang="zh-CN"><title>{}</title><id>{base}feed.xml</id><link rel="self" href="{base}feed.xml"/><link rel="alternate" href="{base}"/><updated>{}</updated>"#,
        escape(&title),
        updated.to_rfc3339(),
    );
    for (item, index) in &entries {
        let _ = write!(
            xml,
            r#"<entry><id>urn:uuid:{}</id><title>{}</title><link rel="alternate" href="{}"/><updated>{}</updated>{}<author><name>{}</name></author><category term="{}"/>{}<summary>{}</summary></entry>"#,
            item.id,
            escape(&item.title),
            escape(&item.url),
            item.first_seen_at.to_rfc3339(),
            item.published_at
                .map(|at| format!("<published>{}</published>", at.to_rfc3339()))
                .unwrap_or_default(),
            escape(&item.source),
            escape(&topics[*index].name),
            item.tags
                .iter()
                .map(|tag| format!(r#"<category term="{}"/>"#, escape(tag)))
                .collect::<String>(),
            escape(&item.summary),
        );
    }
    xml.push_str("</feed>");
    Ok(conditional(
        &headers,
        &Rendered::new(xml),
        "application/atom+xml; charset=utf-8",
    ))
}

/// Serves a rendered body, or 304 when the client already holds this exact version.
/// Proxies that recompress the body (Cloudflare does) downgrade the tag to W/"...";
/// If-None-Match is defined as a weak comparison anyway.
pub(crate) fn conditional(
    headers: &HeaderMap,
    rendered: &Rendered,
    content_type: &'static str,
) -> Response {
    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|candidate| {
                let candidate = candidate.trim();
                candidate == "*"
                    || candidate.strip_prefix("W/").unwrap_or(candidate) == rendered.etag
            })
        });
    let mut response = if unchanged {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            [(header::CONTENT_TYPE, content_type)],
            rendered.body.clone(),
        )
            .into_response()
    };
    let headers = response.headers_mut();
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&rendered.etag).expect("a quoted hex digest is a valid header"),
    );
    headers.insert(header::CACHE_CONTROL, PUBLIC_CACHE_CONTROL);
    response
}

/// A strong validator over the exact response bytes; 128 bits is far past the point where
/// an accidental collision matters for cache revalidation.
fn entity_tag(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    let mut tag = String::with_capacity(34);
    tag.push('"');
    for byte in digest.iter().take(16) {
        let _ = write!(tag, "{byte:02x}");
    }
    tag.push('"');
    tag
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Rendered, conditional, entity_tag, escape};
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};

    #[test]
    fn revalidates_with_strong_weak_and_wildcard_tags() {
        let rendered = Rendered::new(r#"{"day":"2026-09-03"}"#.into());
        let fresh = conditional(&HeaderMap::new(), &rendered, "application/json");
        assert_eq!(fresh.status(), StatusCode::OK);
        assert_eq!(
            fresh.headers().get(header::CACHE_CONTROL).expect("header"),
            "public, max-age=60, stale-while-revalidate=300"
        );
        let tag = fresh.headers().get(header::ETAG).expect("etag").clone();
        assert_eq!(tag, entity_tag(r#"{"day":"2026-09-03"}"#).as_str());

        for candidate in [
            tag.to_str().expect("ascii").to_owned(),
            format!("W/{}", tag.to_str().expect("ascii")),
            "*".to_owned(),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                header::IF_NONE_MATCH,
                HeaderValue::from_str(&candidate).expect("header"),
            );
            assert_eq!(
                conditional(&headers, &rendered, "application/json").status(),
                StatusCode::NOT_MODIFIED
            );
        }
        let mut stale = HeaderMap::new();
        stale.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"nope\""));
        assert_eq!(
            conditional(&stale, &rendered, "application/json").status(),
            StatusCode::OK
        );
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(escape(r#"a<b>&"c""#), "a&lt;b&gt;&amp;&quot;c&quot;");
    }
}
