use crate::{
    auth::{self, User},
    error::{ApiQuery, AppError, Result},
    reading::{self, Reader},
    site::escape,
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
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
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
        Arc, Mutex as StdMutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicU64, Ordering},
    },
};
use tower_http::compression::CompressionLayer;
use uuid::Uuid;

// API default; the homepage passes its smaller first-page limit explicitly.
const DEFAULT_LIMIT: usize = 32;
const MAX_LIMIT: usize = 128;
const MAX_DAYS: i64 = 366;
const DEFAULT_DAYS_LIMIT: i64 = 30;
const FEED_LIMIT: usize = 50;
/// Pre-rendered `/digest` and `/days` bodies are kept per query shape. The bounds stop a
/// scan over every `limit` value from turning the two maps into a memory sink.
const HOT_VARIANTS: usize = 8;
const DAYS_VARIANTS: usize = 16;
const MAX_CURSOR_LEN: usize = 256;
const PUBLIC_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=60, stale-while-revalidate=300");

#[derive(Debug, Clone, FromRow)]
pub struct TopicMeta {
    pub id: String,
    pub name: String,
    pub name_en: String,
    pub description: String,
    pub visible: bool,
}

#[derive(Default)]
struct Catalogue {
    public: Arc<Vec<TopicMeta>>,
    all: Arc<Vec<TopicMeta>>,
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
    pub tags_en: Vec<String>,
    pub importance: i16,
    pub pinned: bool,
    pub image: Option<serde_json::Value>,
    /// A travel moment's photographs and destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sqlx(default)]
    pub travel: Option<serde_json::Value>,
    /// A catalogue item's role and, once written up, its entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[sqlx(default)]
    pub catalog: Option<serde_json::Value>,
    /// Set only in an administrator's preview of 𝑾, on photographs readers don't see.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[sqlx(default)]
    pub hidden: bool,
}

/// One topic on one day: the visible items in display order, as PostgreSQL returned them.
/// Today and yesterday stay in memory.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub updated_at: Option<DateTime<Utc>>,
    /// Changes whenever the topic's visible feed changes.
    pub revision: i64,
    pub items: Vec<PublicItem>,
    /// An administrator's view of 𝑾: the whole day in display order, hidden photographs
    /// included; empty while none is hidden.
    pub preview: Vec<PublicItem>,
}

impl Snapshot {
    /// The items `reader` sees, and when the newest of them was collected.
    fn view(&self, reader: Reader) -> (&[PublicItem], Option<DateTime<Utc>>) {
        if reader.is_admin() && !self.preview.is_empty() {
            (&self.preview, newest(&self.preview))
        } else {
            (&self.items, self.updated_at)
        }
    }
}

fn newest(items: &[PublicItem]) -> Option<DateTime<Utc>> {
    items.iter().map(|item| item.first_seen_at).max()
}

/// The display-order key of one item: `pinned DESC, published_at DESC NULLS LAST,
/// first_seen_at DESC, id DESC`. Deriving `Ord` over the fields in this order and comparing
/// descending yields exactly that order; `None` sorting before `Some` is what puts undated
/// items last. 𝑾's key leaves the post time out, so the newest run's photographs lead: an
/// old story found late shouldn't top a news feed, but a photograph is new to the gallery
/// when it arrives. A page starts strictly after a key, so the cursor remains unambiguous
/// while these sort keys stay stable; callers should refresh and de-duplicate when the feed
/// changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cursor {
    #[serde(rename = "p")]
    pinned: bool,
    #[serde(rename = "t")]
    published_at: Option<DateTime<Utc>>,
    #[serde(rename = "s")]
    first_seen_at: DateTime<Utc>,
    #[serde(rename = "i")]
    id: Uuid,
}

impl Cursor {
    pub fn of(topic: &str, item: &PublicItem) -> Self {
        Self {
            pinned: item.pinned,
            published_at: item
                .published_at
                .filter(|_| topic != crate::gallery::TOPIC_ID),
            first_seen_at: item.first_seen_at,
            id: item.id,
        }
    }

    /// The token handed out as `next` and accepted back as `after`.
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("cursor serialises"))
    }

    pub fn decode(token: &str) -> Result<Self> {
        let invalid = || AppError::BadRequest("after 参数无效".into());
        if token.is_empty() || token.len() > MAX_CURSOR_LEN {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?;
        serde_json::from_slice(&bytes).map_err(|_| invalid())
    }
}

/// One page of a topic's snapshot: the first `limit` items strictly after `after` in display
/// order, and the cursor of the page that follows, if there is one.
fn page<'a>(
    topic: &str,
    items: &'a [PublicItem],
    after: Option<Cursor>,
    limit: usize,
) -> (&'a [PublicItem], Option<String>) {
    let key = |item: &PublicItem| Cursor::of(topic, item);
    let start = after.map_or(0, |cursor| {
        items.partition_point(|item| key(item) >= cursor)
    });
    let end = start.saturating_add(limit).min(items.len());
    let next = (end > start && end < items.len()).then(|| key(&items[end - 1]).encode());
    (&items[start..end], next)
}

/// Archive counts depend on the reader; the topic-day content snapshots do not.
type DaysKey = (Reader, Option<String>, i64);

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

/// Memory layer of the digest. Today and yesterday are kept in memory and other days are read
/// from PostgreSQL when asked for; a write reloads its day from PostgreSQL.
#[derive(Clone)]
pub struct DigestCache(Arc<Inner>);

struct Inner {
    tz: Tz,
    topics: RwLock<Catalogue>,
    days: RwLock<HashMap<(String, NaiveDate), Arc<Snapshot>>>,
    /// PostgreSQL's answer for topics with nothing in the last two days, until the next write;
    /// the flag marks an administrator's answer for 𝑾, which counts hidden photographs too.
    latest: RwLock<HashMap<(String, bool), Option<NaiveDate>>>,
    /// The first page of the all-topics `/digest`, pre-rendered per `limit` until the next
    /// write. The public page and the API default ask for different limits.
    hot: RwLock<HashMap<usize, Arc<Rendered>>>,
    /// `/days` bodies per (topic, limit); the query behind them scans every visible item.
    archive: RwLock<HashMap<DaysKey, Arc<Rendered>>>,
    /// Bumped by every write. A load that began before a write must not be stored after it.
    generation: AtomicU64,
    /// Serializes cache misses and explicit refreshes. Without this small
    /// critical section, invalidation operations could race while replacing the
    /// topic catalogue or pruning archive data.
    cache_write: tokio::sync::Mutex<()>,
    /// Cache misses for different topics do not need to block each other. These
    /// locks are per topic rather than per day so archive requests cannot grow
    /// the lock map without bound.
    topic_writes: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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
            archive: RwLock::default(),
            generation: AtomicU64::new(0),
            cache_write: tokio::sync::Mutex::new(()),
            topic_writes: StdMutex::default(),
        }))
    }

    fn topic_write(&self, topic: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .0
            .topic_writes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        locks
            .entry(topic.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
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
        self.topics_for(Reader::Public)
    }

    pub(crate) fn topics_for(&self, reader: Reader) -> Arc<Vec<TopicMeta>> {
        let catalogue = read(&self.0.topics);
        Arc::clone(if reader.is_admin() {
            &catalogue.all
        } else {
            &catalogue.public
        })
    }

    pub async fn reload_topics(&self, state: &AppState) -> Result<()> {
        let _cache_guard = self.0.cache_write.lock().await;
        let topics = sqlx::query_as::<_, TopicMeta>(
            "SELECT id, name, name_en, description, visible FROM topics WHERE enabled AND owner_user_id IS NULL ORDER BY sort_weight, created_at",
        )
        .fetch_all(&state.pool)
        .await?;
        // Invalidate in-memory snapshots together with the topic catalogue.  This
        // matters when an administrator disables and later re-enables a topic:
        // the next request must re-read its current items.
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        *write(&self.0.topics) = Catalogue {
            public: Arc::new(
                topics
                    .iter()
                    .filter(|topic| topic.visible)
                    .cloned()
                    .collect(),
            ),
            all: Arc::new(topics),
        };
        write(&self.0.days).clear();
        write(&self.0.latest).clear();
        // A reader may have started between the first fence and the swap; fence it as well,
        // before the rendered bodies go, so none of its bodies is kept.
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        self.forget_rendered();
        // The briefing keeps only sources from this catalogue; render it again after the swap.
        state.insight.invalidate();
        Ok(())
    }

    /// Fills memory for every enabled topic so the first request already hits it.
    pub async fn warm(&self, state: &AppState) -> Result<()> {
        for topic in self.topics().iter() {
            for day in [self.today(), self.yesterday()] {
                self.snapshot(state, &topic.id, day).await?;
            }
        }
        Ok(())
    }

    /// Shared topic content. Reading routes select an authorized topic before calling this.
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
        let topic_guard = self.topic_write(topic);
        let _cache_guard = topic_guard.lock().await;
        // Another miss may have filled memory while this request waited for
        // the writer section.
        if let Some(hit) = read(&self.0.days).get(&key).cloned() {
            return Ok(hit);
        }
        let generation = self.0.generation.load(Ordering::Acquire);
        let snapshot = Arc::new(load(state, topic, day).await?);
        // A write may have invalidated this read while it was in flight.
        if self.is_recent(day) {
            let mut days = write(&self.0.days);
            if self.0.generation.load(Ordering::Acquire) == generation {
                days.insert(key, Arc::clone(&snapshot));
            }
        }
        Ok(snapshot)
    }

    /// Reloads one topic-day from PostgreSQL after a write.
    pub async fn refresh(&self, state: &AppState, topic: &str, day: NaiveDate) -> Result<()> {
        let topic_guard = self.topic_write(topic);
        let _cache_guard = topic_guard.lock().await;
        let enabled = sqlx::query_scalar::<_, bool>(
            "SELECT enabled FROM topics WHERE id = $1 AND owner_user_id IS NULL",
        )
        .bind(topic)
        .fetch_optional(&state.pool)
        .await?;
        if !enabled.unwrap_or(false) {
            // Disabled/deleted topics must not leave an empty snapshot behind
            // that would hide fresh items when the topic is enabled again.
            self.forget_topic_locked(topic);
            return Ok(());
        }
        // Answers computed before this write are not kept: the first bump covers those already
        // running, the second those that read the old snapshot while this one loaded.
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        let snapshot = Arc::new(load(state, topic, day).await?);
        if self.is_recent(day) {
            write(&self.0.days).insert((topic.to_owned(), day), snapshot);
        }
        write(&self.0.latest).retain(|(id, _), _| id != topic);
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        self.forget_rendered();
        Ok(())
    }

    /// Drops everything kept for the topic; its days load again from PostgreSQL when asked for.
    pub async fn forget_topic(&self, topic: &str) {
        let topic_guard = self.topic_write(topic);
        let _topic_guard = topic_guard.lock().await;
        let _cache_guard = self.0.cache_write.lock().await;
        self.forget_topic_locked(topic);
    }

    /// Retention pruning deleted old items: forget the answers derived from them.
    pub(crate) async fn forget_pruned(&self) {
        let _cache_guard = self.0.cache_write.lock().await;
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        write(&self.0.latest).clear();
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        self.forget_rendered();
    }

    fn forget_topic_locked(&self, topic: &str) {
        // As in `refresh`: the first bump stops loads in flight from storing what they read,
        // the second stops renders that read the old snapshot.
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        write(&self.0.days).retain(|(id, _), _| id != topic);
        write(&self.0.latest).retain(|(id, _), _| id != topic);
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        self.forget_rendered();
    }

    /// Runs once the local date changes: the day before yesterday leaves memory.
    pub fn rollover(&self) {
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        let keep_from = self.yesterday();
        write(&self.0.days).retain(|(_, day), _| *day >= keep_from);
        write(&self.0.latest).clear();
        self.0.generation.fetch_add(1, Ordering::AcqRel);
        self.forget_rendered();
    }

    /// The most recent day with items the reader sees in the topic.
    pub async fn latest_day(
        &self,
        state: &AppState,
        topic: &str,
        reader: Reader,
    ) -> Result<Option<NaiveDate>> {
        for day in [self.today(), self.yesterday()] {
            if !self
                .snapshot(state, topic, day)
                .await?
                .view(reader)
                .0
                .is_empty()
            {
                return Ok(Some(day));
            }
        }
        let include_hidden = reader.is_admin() && topic == crate::gallery::TOPIC_ID;
        let key = (topic.to_owned(), include_hidden);
        if let Some(cached) = read(&self.0.latest).get(&key).copied() {
            return Ok(cached);
        }
        let generation = self.0.generation.load(Ordering::Acquire);
        let day = sqlx::query_scalar::<_, Option<NaiveDate>>(
            "SELECT MAX(items.day) FROM items JOIN topics ON topics.id = items.topic_id WHERE items.topic_id = $1 AND topics.enabled AND topics.owner_user_id IS NULL AND (NOT items.hidden OR $2)",
        )
        .bind(topic)
        .bind(include_hidden)
        .fetch_one(&state.pool)
        .await?;
        // Checked under the lock: a write bumps the generation before it clears this map.
        let mut latest = write(&self.0.latest);
        if self.0.generation.load(Ordering::Acquire) == generation {
            latest.insert(key, day);
        }
        Ok(day)
    }

    /// Drops every pre-rendered body. Callers bump the generation first, and renders check it
    /// under these locks, so a body rendered before a write is never kept after it.
    fn forget_rendered(&self) {
        write(&self.0.hot).clear();
        write(&self.0.archive).clear();
    }

    /// The `/days` body: which days have visible items, newest first. PostgreSQL is asked
    /// once per (topic, limit) between writes.
    pub async fn days(
        &self,
        state: &AppState,
        topic: Option<&str>,
        limit: i64,
    ) -> Result<Arc<Rendered>> {
        self.days_for(state, topic, limit, Reader::Public).await
    }

    async fn days_for(
        &self,
        state: &AppState,
        topic: Option<&str>,
        limit: i64,
        reader: Reader,
    ) -> Result<Arc<Rendered>> {
        let key = (reader, topic.map(str::to_owned), limit);
        if let Some(hit) = read(&self.0.archive).get(&key).cloned() {
            return Ok(hit);
        }
        // Only catalogue topics are cached; an arbitrary id must not be able to fill the map.
        let cacheable =
            topic.is_none_or(|id| self.topics_for(reader).iter().any(|topic| topic.id == id));
        let generation = self.0.generation.load(Ordering::Acquire);
        // An administrator's counts include 𝑾's hidden photographs.
        let days = sqlx::query_as::<_, DayCount>(
            "SELECT items.day, COUNT(*) AS count FROM items \
             JOIN topics ON topics.id = items.topic_id \
             WHERE ($1::text IS NULL OR items.topic_id = $1) AND topics.enabled AND (topics.visible OR $3) AND topics.owner_user_id IS NULL \
             AND (NOT items.hidden OR ($3 AND items.topic_id = $4)) \
             GROUP BY items.day ORDER BY items.day DESC LIMIT $2",
        )
        .bind(topic)
        .bind(limit)
        .bind(reader.is_admin())
        .bind(crate::gallery::TOPIC_ID)
        .fetch_all(&state.pool)
        .await?;
        let rendered = Arc::new(Rendered::new(
            json!({ "topic": topic, "days": days }).to_string(),
        ));
        if cacheable {
            let mut archive = write(&self.0.archive);
            if self.0.generation.load(Ordering::Acquire) == generation
                && (archive.len() < DAYS_VARIANTS || archive.contains_key(&key))
            {
                archive.insert(key, Arc::clone(&rendered));
            }
        }
        Ok(rendered)
    }

    /// Returns typed public day counts for private-feed assembly. The rendered response is
    /// already cached by `(topic, limit)`, so authenticated pages do not rescan public items.
    async fn day_counts(&self, state: &AppState, topic: Option<&str>) -> Result<Vec<DayCount>> {
        let rendered = self.days(state, topic, MAX_DAYS).await?;
        let payload: CachedDays = serde_json::from_slice(&rendered.body)?;
        Ok(payload.days)
    }

    fn selected(
        &self,
        topic: Option<&str>,
        reader: Reader,
    ) -> Result<(Arc<Vec<TopicMeta>>, Vec<usize>)> {
        let topics = self.topics_for(reader);
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
    /// the selection is used, so a page never mixes days. `after` and `limit` select one
    /// page of each topic's items; `total` always reports the whole day and `next` names
    /// the page that follows.
    async fn render(
        &self,
        state: &AppState,
        topic: Option<&str>,
        day: Option<NaiveDate>,
        limit: usize,
        after: Option<Cursor>,
        reader: Reader,
    ) -> Result<Arc<Rendered>> {
        let hot_path = !reader.is_admin() && topic.is_none() && day.is_none() && after.is_none();
        if hot_path && let Some(hit) = read(&self.0.hot).get(&limit).cloned() {
            return Ok(hit);
        }
        let generation = self.0.generation.load(Ordering::Acquire);
        let (topics, selected) = self.selected(topic, reader)?;
        let mut latest = Vec::with_capacity(selected.len());
        for &index in &selected {
            latest.push(self.latest_day(state, &topics[index].id, reader).await?);
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
            topics: selected
                .iter()
                .zip(&latest)
                .zip(&snapshots)
                .map(|((&index, latest_day), snapshot)| {
                    let topic = &topics[index];
                    let (items, updated_at) = snapshot
                        .as_ref()
                        .map_or((&[][..], None), |s| s.view(reader));
                    let (page_items, next) = page(&topic.id, items, after, limit);
                    TopicDigest {
                        id: &topic.id,
                        name: &topic.name,
                        name_en: &topic.name_en,
                        description: &topic.description,
                        latest_day: *latest_day,
                        updated_at,
                        revision: snapshot.as_ref().map_or(0, |s| s.revision),
                        total: items.len(),
                        next,
                        items: page_items,
                    }
                })
                .collect(),
        };
        let rendered = Arc::new(Rendered::new(serde_json::to_string(&body)?));
        if hot_path {
            let mut hot = write(&self.0.hot);
            if self.0.generation.load(Ordering::Acquire) == generation
                && (hot.len() < HOT_VARIANTS || hot.contains_key(&limit))
            {
                hot.insert(limit, Arc::clone(&rendered));
            }
        }
        Ok(rendered)
    }
}

#[derive(Serialize)]
struct DigestBody<'a> {
    day: Option<NaiveDate>,
    timezone: &'a str,
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
    revision: i64,
    total: usize,
    next: Option<String>,
    items: &'a [PublicItem],
}

async fn load(state: &AppState, topic: &str, day: NaiveDate) -> Result<Snapshot> {
    // The revision is read first. Items committed in between only make the snapshot newer
    // than its revision, so a client that trusts the revision loads it again, instead of
    // keeping stale items under a revision that already covers the new ones.
    let revision = sqlx::query_scalar::<_, i64>("SELECT feed_revision FROM topics WHERE id = $1")
        .bind(topic)
        .fetch_one(&state.pool)
        .await?;
    let mut query = sqlx::QueryBuilder::new(
        "SELECT items.id, items.title, items.summary, items.title_en, items.summary_en, items.url, items.source, items.published_at, items.first_seen_at, items.tags, items.tags_en, items.importance, items.pinned, items.hidden, items.image, ",
    );
    let section = crate::catalog::Section::of(topic);
    query
        .push(crate::travel::MOMENT_JSON)
        .push(" AS travel, ")
        .push(section.map_or_else(|| "NULL::jsonb".to_owned(), crate::catalog::item_json))
        .push(" AS catalog FROM items JOIN topics ON topics.id = items.topic_id ")
        .push(crate::travel::MOMENT_JOIN);
    if section.is_some() {
        query.push(" ").push(crate::catalog::ITEM_JOIN);
    }
    query
        .push(" WHERE items.topic_id = ")
        .push_bind(topic)
        .push(" AND items.day = ")
        .push_bind(day)
        .push(" AND topics.enabled AND topics.owner_user_id IS NULL");
    // 𝑾's hidden photographs are read for the administrator's preview (`gallery`), and its
    // order leaves the post time out (`Cursor`).
    query.push(if topic == crate::gallery::TOPIC_ID {
        " ORDER BY items.pinned DESC, items.first_seen_at DESC, items.id DESC"
    } else {
        " AND NOT items.hidden ORDER BY items.pinned DESC, items.published_at DESC NULLS LAST, items.first_seen_at DESC, items.id DESC"
    });
    let mut items = query
        .build_query_as::<PublicItem>()
        .fetch_all(&state.pool)
        .await?;
    for item in &mut items {
        item.image = public_image(item.image.take());
    }
    let (items, preview) = if items.iter().any(|item| item.hidden) {
        (
            items.iter().filter(|item| !item.hidden).cloned().collect(),
            items,
        )
    } else {
        (items, Vec::new())
    };
    Ok(Snapshot {
        updated_at: newest(&items),
        revision,
        items,
        preview,
    })
}

// ---------------------------------------------------------------------------
// Public routes
// ---------------------------------------------------------------------------

pub(crate) fn router() -> Router<AppState> {
    // Keep the authenticated feed outside the public-CORS layer.  Its response contains
    // account-owned topics and must never be readable by an arbitrary cross-origin page.
    let shared = Router::new()
        .route("/digest", get(digest))
        .route("/topics", get(topics))
        .route("/days", get(days));
    let public = reading::public(shared.clone())
        .route("/insight", get(crate::insight::latest))
        .route("/feed.xml", get(feed))
        .layer(CompressionLayer::new())
        .layer(middleware::from_fn(public_cors));
    let personal = Router::new()
        .route("/api/me/feed", get(me_feed))
        .layer(CompressionLayer::new());
    public
        .merge(reading::admin(shared).layer(CompressionLayer::new()))
        .merge(personal)
}

/// Applied outside the handlers so failures are readable cross-origin too; without it a
/// third-party `fetch` sees an opaque network error instead of the JSON error body.
pub(crate) async fn public_cors(request: Request, next: Next) -> Response {
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
    after: Option<String>,
}

/// `after` continues one topic's page sequence; without a topic there is nothing to continue.
fn cursor_from(query: &DigestQuery) -> Result<Option<Cursor>> {
    match &query.after {
        None => Ok(None),
        Some(_) if query.topic.is_none() => {
            Err(AppError::BadRequest("after 需要同时指定 topic".into()))
        }
        Some(token) => Cursor::decode(token).map(Some),
    }
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

#[derive(Debug, Serialize, Deserialize, FromRow)]
struct DayCount {
    day: NaiveDate,
    count: i64,
}

#[derive(Debug, Deserialize)]
struct CachedDays {
    days: Vec<DayCount>,
}

/// The small catalogue used by the authenticated home page. It intentionally contains
/// only display data and aggregate counts; ownership is enforced by the query, and only `own`
/// tells the reader which topics are theirs: the page has tabs for the site's and adds theirs.
#[derive(Debug, Serialize, FromRow)]
struct PersonalTopicCard {
    id: String,
    name: String,
    name_en: String,
    description: String,
    latest_day: Option<NaiveDate>,
    today_count: i64,
    updated_at: Option<DateTime<Utc>>,
    revision: i64,
    own: bool,
}

#[derive(Debug, FromRow)]
struct PersonalDayStats {
    topic_id: String,
    total: i64,
    updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct PersonalItemsRow {
    topic_id: String,
    items: serde_json::Value,
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
    revision: i64,
    total: usize,
    next: Option<String>,
    items: Vec<PublicItem>,
}

async fn digest(
    State(state): State<AppState>,
    reader: Reader,
    ApiQuery(query): ApiQuery<DigestQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let after = cursor_from(&query)?;
    let rendered = state
        .digest
        .render(
            &state,
            query.topic.as_deref(),
            query.day,
            limit,
            after,
            reader,
        )
        .await?;
    Ok(conditional(&headers, &rendered, "application/json"))
}

async fn topics(
    State(state): State<AppState>,
    reader: Reader,
    headers: HeaderMap,
) -> Result<Response> {
    let cache = &state.digest;
    let today = cache.today();
    let mut list = Vec::new();
    for topic in cache.topics_for(reader).iter() {
        let snapshot = cache.snapshot(&state, &topic.id, today).await?;
        let (items, updated_at) = snapshot.view(reader);
        list.push(json!({
            "id": topic.id,
            "name": topic.name,
            "name_en": topic.name_en,
            "description": topic.description,
            "latest_day": cache.latest_day(&state, &topic.id, reader).await?,
            "today_count": items.len(),
            "updated_at": updated_at,
        }));
    }
    let rendered = Rendered::new(json!({ "topics": list }).to_string());
    Ok(conditional(&headers, &rendered, "application/json"))
}

async fn days(
    State(state): State<AppState>,
    reader: Reader,
    ApiQuery(query): ApiQuery<DaysQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let limit = query.limit.unwrap_or(DEFAULT_DAYS_LIMIT).clamp(1, MAX_DAYS);
    let rendered = state
        .digest
        .days_for(&state, query.topic.as_deref(), limit, reader)
        .await?;
    Ok(conditional(&headers, &rendered, "application/json"))
}

/// Returns the public catalogue plus the current user's enabled topics together with a
/// digest for that selection. Public rows are intentionally included here because the
/// authenticated home page is a personalized view of the public feed, not a replacement
/// for it.
///
/// Public topics are read through `DigestCache`; only the current user's personal topics are
/// queried from PostgreSQL. This keeps the authenticated view private without rescanning the
/// public catalogue on every request.
async fn me_feed(
    State(state): State<AppState>,
    user: User,
    ApiQuery(query): ApiQuery<DigestQuery>,
) -> Result<Response> {
    auth::require_personal(&user)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let after = cursor_from(&query)?;
    let cards = personal_topic_cards(&state, user.id).await?;
    let public_ids = state
        .digest
        .topics()
        .iter()
        .map(|topic| topic.id.clone())
        .collect::<std::collections::HashSet<_>>();

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

    let selected_public = query
        .topic
        .as_deref()
        .is_some_and(|id| public_ids.contains(id));
    let include_public = query.topic.is_none() || selected_public;
    let include_private = query.topic.is_none() || !selected_public;
    let public_days = if include_public {
        state
            .digest
            .day_counts(&state, query.topic.as_deref())
            .await?
    } else {
        Vec::new()
    };
    let private_days = if include_private {
        personal_days(&state, user.id, query.topic.as_deref()).await?
    } else {
        Vec::new()
    };
    let mut day_counts = HashMap::<NaiveDate, i64>::new();
    for entry in public_days.into_iter().chain(private_days) {
        *day_counts.entry(entry.day).or_default() += entry.count;
    }
    let mut days = day_counts
        .into_iter()
        .map(|(day, count)| DayCount { day, count })
        .collect::<Vec<_>>();
    days.sort_by_key(|entry| std::cmp::Reverse(entry.day));
    days.truncate(MAX_DAYS as usize);
    // As with `/digest`, an omitted day means the newest day that has content in the
    // selection.  An explicit empty day is retained so the UI can render its empty state.
    let selected_day = query.day.or_else(|| days.first().map(|entry| entry.day));
    let private_stats = if include_private {
        match selected_day {
            Some(day) => personal_day_stats(&state, user.id, query.topic.as_deref(), day).await?,
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let mut stats = private_stats
        .into_iter()
        .map(|entry| (entry.topic_id.clone(), entry))
        .collect::<HashMap<_, _>>();

    let private_topic_ids = selected
        .iter()
        .filter_map(|index| {
            let id = &cards[*index].id;
            (!public_ids.contains(id)).then(|| id.clone())
        })
        .collect::<Vec<_>>();
    let private_items = match selected_day {
        Some(day) if !private_topic_ids.is_empty() => {
            personal_items(&state, user.id, &private_topic_ids, day, limit, after).await?
        }
        _ => HashMap::new(),
    };

    let mut digest_topics = Vec::with_capacity(selected.len());
    for index in selected {
        let card = &cards[index];
        let ((items, next), total, updated_at) = match selected_day {
            Some(day) if public_ids.contains(&card.id) => {
                let snapshot = state.digest.snapshot(&state, &card.id, day).await?;
                let (items, next) = page(&card.id, &snapshot.items, after, limit);
                (
                    (items.to_vec(), next),
                    snapshot.items.len() as i64,
                    snapshot.updated_at,
                )
            }
            Some(_) => {
                let (items, next) = private_items.get(&card.id).cloned().unwrap_or_default();
                let entry = stats.remove(&card.id);
                (
                    (items, next),
                    entry.as_ref().map_or(0, |stats| stats.total),
                    entry.and_then(|stats| stats.updated_at),
                )
            }
            None => ((Vec::new(), None), 0, None),
        };
        digest_topics.push(PersonalTopicDigest {
            id: card.id.clone(),
            name: card.name.clone(),
            name_en: card.name_en.clone(),
            description: card.description.clone(),
            latest_day: card.latest_day,
            updated_at,
            revision: card.revision,
            total: usize::try_from(total).unwrap_or(usize::MAX),
            next,
            items,
        });
    }

    let payload = PersonalFeedResponse {
        topics: cards,
        days,
        digest: PersonalDigest {
            day: selected_day,
            timezone: state.config.timezone.name().to_owned(),
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
    let mut cards = Vec::new();
    for topic in state.digest.topics().iter() {
        let snapshot = state
            .digest
            .snapshot(state, &topic.id, state.digest.today())
            .await?;
        cards.push(PersonalTopicCard {
            id: topic.id.clone(),
            name: topic.name.clone(),
            name_en: topic.name_en.clone(),
            description: topic.description.clone(),
            latest_day: state
                .digest
                .latest_day(state, &topic.id, Reader::Public)
                .await?,
            today_count: snapshot.items.len() as i64,
            updated_at: snapshot.updated_at,
            revision: snapshot.revision,
            own: false,
        });
    }
    let personal = sqlx::query_as::<_, PersonalTopicCard>(
        r#"
        SELECT
            topics.id,
            topics.name,
            topics.name_en,
            topics.description,
            MAX(items.day) AS latest_day,
            COUNT(items.id) FILTER (WHERE items.day = $2) AS today_count,
            MAX(items.first_seen_at) FILTER (WHERE items.day = $2) AS updated_at,
            topics.feed_revision AS revision,
            TRUE AS own
        FROM topics
        LEFT JOIN items
          ON items.topic_id = topics.id
         AND NOT items.hidden
        WHERE topics.enabled AND topics.visible
          AND topics.owner_user_id = $1
        GROUP BY topics.id, topics.name, topics.name_en, topics.description,
                 topics.owner_user_id, topics.sort_weight, topics.created_at, topics.feed_revision
        ORDER BY topics.created_at, topics.id
        "#,
    )
    .bind(user_id)
    .bind(state.digest.today())
    .fetch_all(&state.pool)
    .await?;
    cards.extend(personal);
    Ok(cards)
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
        WHERE topics.enabled AND topics.visible
          AND topics.owner_user_id = $1
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
        WHERE topics.enabled AND topics.visible
          AND topics.owner_user_id = $1
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
    topics: &[String],
    day: NaiveDate,
    limit: usize,
    after: Option<Cursor>,
) -> Result<HashMap<String, (Vec<PublicItem>, Option<String>)>> {
    if topics.is_empty() {
        return Ok(HashMap::new());
    }
    // One row beyond the page tells whether a `next` cursor is due. The row comparison is
    // the SQL form of `Cursor`'s ordering; undated items compare as -infinity, as NULLS LAST
    // does for the ORDER BY.
    let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
    let rows = sqlx::query_as::<_, PersonalItemsRow>(
        r#"
        SELECT selected.topic_id,
               COALESCE(
                   jsonb_agg(item.payload ORDER BY item.pinned DESC, item.published_at DESC NULLS LAST, item.first_seen_at DESC, item.id DESC)
                       FILTER (WHERE item.id IS NOT NULL),
                   '[]'::jsonb
               ) AS items
        FROM unnest($1::text[]) AS selected(topic_id)
        LEFT JOIN LATERAL (
            SELECT items.id,
                   items.pinned,
                   items.published_at,
                   items.first_seen_at,
                   jsonb_build_object(
                       'id', items.id,
                       'title', items.title,
                       'summary', items.summary,
                       'title_en', items.title_en,
                       'summary_en', items.summary_en,
                       'url', items.url,
                       'source', items.source,
                       'published_at', items.published_at,
                       'first_seen_at', items.first_seen_at,
                       'tags', items.tags,
                       'tags_en', items.tags_en,
                       'importance', items.importance,
                       'pinned', items.pinned,
                       'image', items.image
                   ) AS payload
            FROM items
            JOIN topics ON topics.id = items.topic_id
            WHERE items.topic_id = selected.topic_id
              AND topics.owner_user_id = $2
              AND topics.enabled AND topics.visible
              AND items.day = $3
              AND NOT items.hidden
              AND ($5::boolean IS NULL OR
                   (items.pinned, COALESCE(items.published_at, '-infinity'::timestamptz), items.first_seen_at, items.id)
                   < ($5, COALESCE($6, '-infinity'::timestamptz), $7, $8))
            ORDER BY items.pinned DESC,
                     items.published_at DESC NULLS LAST,
                     items.first_seen_at DESC,
                     items.id DESC
            LIMIT $4
        ) AS item ON TRUE
        GROUP BY selected.topic_id
        "#,
    )
    .bind(topics)
    .bind(user_id)
    .bind(day)
    .bind(fetch)
    .bind(after.map(|cursor| cursor.pinned))
    .bind(after.and_then(|cursor| cursor.published_at))
    .bind(after.map(|cursor| cursor.first_seen_at))
    .bind(after.map(|cursor| cursor.id))
    .fetch_all(&state.pool)
    .await?;
    let mut result = HashMap::with_capacity(rows.len());
    for row in rows {
        let mut items = serde_json::from_value::<Vec<PublicItem>>(row.items)?;
        for item in &mut items {
            item.image = public_image(item.image.take());
        }
        let next = (items.len() > limit).then(|| {
            items.truncate(limit);
            Cursor::of(&row.topic_id, &items[limit - 1]).encode()
        });
        result.insert(row.topic_id, (items, next));
    }
    Ok(result)
}

/// Keep renewal metadata server-side. The public contract needs only the temporary display
/// link and the editorial fields; `origin_url` and `expires_at` remain in PostgreSQL so the
/// media refresher can renew them.
fn public_image(value: Option<serde_json::Value>) -> Option<serde_json::Value> {
    let serde_json::Value::Object(mut object) = value? else {
        return None;
    };
    object.remove("origin_url");
    object.remove("content_hash");
    object.remove("expires_at");
    Some(serde_json::Value::Object(object))
}

/// Atom 1.0 over the last two days of the selection, newest collected first.
async fn feed(
    State(state): State<AppState>,
    ApiQuery(query): ApiQuery<TopicQuery>,
    headers: HeaderMap,
) -> Result<Response> {
    let cache = &state.digest;
    let (topics, selected) = cache.selected(query.topic.as_deref(), Reader::Public)?;
    let mut snapshots = Vec::with_capacity(selected.len() * 2);
    for &index in &selected {
        for day in [cache.today(), cache.yesterday()] {
            snapshots.push((cache.snapshot(&state, &topics[index].id, day).await?, index));
        }
    }
    let mut entries = snapshots
        .iter()
        .flat_map(|(snapshot, index)| snapshot.items.iter().map(move |item| (item, *index)))
        .collect::<Vec<_>>();
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
        let url = if topics[*index].id == "reads" {
            format!("{base}reads/{}", item.id)
        } else {
            item.url.clone()
        };
        let _ = write!(
            xml,
            r#"<entry><id>urn:uuid:{}</id><title>{}</title><link rel="alternate" href="{}"/><updated>{}</updated>{}<author><name>{}</name></author><category term="{}"/>{}<summary>{}</summary></entry>"#,
            item.id,
            escape(&item.title),
            escape(&url),
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

#[cfg(test)]
mod tests {
    use super::{
        Cursor, DigestCache, PublicItem, Rendered, Snapshot, conditional, entity_tag, escape, page,
        read, write,
    };
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use chrono::{Duration, TimeZone, Utc};
    use uuid::Uuid;

    const NEWS: &str = "x";

    #[test]
    fn rollover_expires_old_snapshots_and_rendered_bodies() {
        let cache = DigestCache::new(chrono_tz::Asia::Shanghai);
        let yesterday = cache.yesterday();
        let old_day = yesterday.pred_opt().unwrap();
        write(&cache.0.days).insert(
            ("topic".into(), old_day),
            std::sync::Arc::new(Snapshot::default()),
        );
        write(&cache.0.days).insert(
            ("topic".into(), yesterday),
            std::sync::Arc::new(Snapshot::default()),
        );
        write(&cache.0.hot).insert(
            12,
            std::sync::Arc::new(Rendered::new("old response".into())),
        );
        cache.rollover();
        assert!(!read(&cache.0.days).contains_key(&("topic".into(), old_day)));
        assert!(read(&cache.0.days).contains_key(&("topic".into(), yesterday)));
        assert!(read(&cache.0.hot).is_empty());
    }

    /// Items are told apart by `importance`; `published_at` grows with `n`, so higher `n`
    /// is newer and sorts first among unpinned items.
    fn item(n: u8, pinned: bool) -> PublicItem {
        let base = Utc
            .with_ymd_and_hms(2026, 9, 7, 0, 0, 0)
            .single()
            .expect("valid");
        PublicItem {
            id: Uuid::from_u128(u128::from(n) + 1),
            title: String::new(),
            summary: String::new(),
            title_en: String::new(),
            summary_en: String::new(),
            url: String::new(),
            source: String::new(),
            published_at: Some(base + Duration::minutes(i64::from(n))),
            first_seen_at: base,
            tags: Vec::new(),
            tags_en: Vec::new(),
            importance: i16::from(n),
            pinned,
            image: None,
            travel: None,
            catalog: None,
            hidden: false,
        }
    }

    fn marks(items: &[PublicItem]) -> Vec<i16> {
        items.iter().map(|item| item.importance).collect()
    }

    #[test]
    fn cursors_round_trip_and_reject_garbage() {
        let cursor = Cursor::of(NEWS, &item(3, true));
        assert_eq!(
            Cursor::decode(&cursor.encode()).expect("valid cursor"),
            cursor
        );
        assert!(Cursor::decode("").is_err());
        assert!(Cursor::decode("not base64!").is_err());
        assert!(Cursor::decode(&URL_SAFE_NO_PAD.encode(br#"{"p":1}"#)).is_err());
        assert!(Cursor::decode(&"a".repeat(257)).is_err());
    }

    #[test]
    fn display_order_puts_pinned_first_and_undated_last() {
        let mut undated = item(9, false);
        undated.published_at = None;
        let mut snapshot = Snapshot {
            items: vec![item(1, false), undated, item(2, true), item(3, false)],
            ..Snapshot::default()
        };
        snapshot
            .items
            .sort_by_key(|item| std::cmp::Reverse(Cursor::of(NEWS, item)));
        assert_eq!(marks(&snapshot.items), vec![2, 3, 1, 9]);
    }

    #[test]
    fn pages_continue_after_a_key_even_when_the_snapshot_changed() {
        let mut snapshot = Snapshot {
            items: (0..7).map(|n| item(n, n % 5 == 0)).collect(),
            ..Snapshot::default()
        };
        snapshot
            .items
            .sort_by_key(|item| std::cmp::Reverse(Cursor::of(NEWS, item)));
        // Pinned 5 and 0 lead; the rest run newest first.
        assert_eq!(marks(&snapshot.items), vec![5, 0, 6, 4, 3, 2, 1]);

        let (first, next) = page(NEWS, &snapshot.items, None, 3);
        assert_eq!(marks(first), vec![5, 0, 6]);
        let cursor = Cursor::decode(next.as_deref().expect("more pages")).expect("valid");

        let (second, next) = page(NEWS, &snapshot.items, Some(cursor), 3);
        assert_eq!(marks(second), vec![4, 3, 2]);
        let last = Cursor::decode(next.as_deref().expect("one more page")).expect("valid");
        let (third, next) = page(NEWS, &snapshot.items, Some(last), 3);
        assert_eq!(marks(third), vec![1]);
        assert!(next.is_none());

        // An item from the first page is hidden and a newer one arrives: an offset would
        // now skip or repeat an item, the key does neither.
        snapshot.items.retain(|entry| entry.importance != 0);
        snapshot.items.push(item(8, false));
        snapshot
            .items
            .sort_by_key(|item| std::cmp::Reverse(Cursor::of(NEWS, item)));
        let (second_again, _) = page(NEWS, &snapshot.items, Some(cursor), 3);
        assert_eq!(marks(second_again), vec![4, 3, 2]);

        // A key past everything yields an empty page and no cursor.
        let (empty, next) = page(
            NEWS,
            &snapshot.items,
            Some(Cursor::of(NEWS, &item(1, false))),
            3,
        );
        assert!(empty.is_empty());
        assert!(next.is_none());
    }

    #[test]
    fn the_gallery_leads_with_its_newest_run_whatever_the_post_times() {
        // 1 and 2 came in one run, 1 with the newest post; 0, the oldest post, came in the
        // next run.
        let mut items = (0..3).map(|n| item(n, false)).collect::<Vec<_>>();
        items[1].published_at = items[1].published_at.map(|at| at + Duration::minutes(10));
        items[0].first_seen_at += Duration::minutes(32);
        let order = |topic: &str| {
            let mut items = items.clone();
            items.sort_by_key(|item| std::cmp::Reverse(Cursor::of(topic, item)));
            items
        };
        assert_eq!(marks(&order(NEWS)), vec![1, 2, 0]);
        // 𝑾 counts no post time, not even within a run, where the id settles it.
        let gallery = order(crate::gallery::TOPIC_ID);
        assert_eq!(marks(&gallery), vec![0, 2, 1]);

        let (first, next) = page(crate::gallery::TOPIC_ID, &gallery, None, 2);
        assert_eq!(marks(first), vec![0, 2]);
        let cursor = Cursor::decode(next.as_deref().expect("one more page")).expect("valid");
        let (second, next) = page(crate::gallery::TOPIC_ID, &gallery, Some(cursor), 2);
        assert_eq!(marks(second), vec![1]);
        assert!(next.is_none());
    }

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
