//! Durable M0 state for Travel Cat.
//!
//! Adoption is intentionally a server-owned flow: the preview stores a random seed,
//! while confirmation accepts only its opaque draft id and derives the identity again.
//! This keeps the immutable identity and initial accessories outside client control.

use crate::{
    auth::User,
    error::{ApiJson, AppError, Result},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, NaiveDate, Timelike, Utc};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

const IDENTITY_VERSION: &str = "v1";
const MAX_NAME_CHARS: usize = 24;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfirmAdoptionRequest {
    draft_id: Uuid,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenameRequest {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActionRequest {
    action: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadingRequest {
    news_item_id: Uuid,
    #[serde(default = "default_reading_signal")]
    signal: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DepartRequest {
    news_item_id: Uuid,
    #[serde(default)]
    pack: Vec<String>,
}

#[derive(Debug, FromRow)]
struct DraftRow {
    id: Uuid,
    identity_version: String,
    identity_seed: Vec<u8>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct CatRow {
    id: Uuid,
    name: String,
    identity_version: String,
    identity: Value,
    initial_accessories: Value,
    adopted_at: DateTime<Utc>,
    name_updated_at: Option<DateTime<Utc>>,
    last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct GrowthCount {
    event_type: String,
    count: i64,
}

#[derive(Debug, FromRow)]
struct TripRow {
    id: Uuid,
    cat_id: Uuid,
    news_item_id: Option<Uuid>,
    day: NaiveDate,
    status: String,
    pack: Vec<String>,
    tone: String,
    location: Option<Value>,
    departed_at: DateTime<Utc>,
    returns_at: DateTime<Utc>,
    settled_at: Option<DateTime<Utc>>,
    read_at: Option<DateTime<Utc>>,
    postcard_id: Option<Uuid>,
    diary: Option<Value>,
    souvenir: Option<Value>,
    error: Option<String>,
    origin: String,
    attempts: i32,
    news_snapshot: Option<Value>,
    note: Option<Value>,
}

#[derive(Debug, FromRow)]
struct MessageRow {
    id: Uuid,
    kind: String,
    entity_id: Option<Uuid>,
    payload: Value,
    created_at: DateTime<Utc>,
    read_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct AutoCat {
    cat_id: Uuid,
}

#[derive(Debug, FromRow)]
struct AutoNews {
    id: Uuid,
    title: String,
    summary: String,
    url: String,
    source: String,
    tags: Vec<String>,
    tone: String,
    location: Option<Value>,
    image: Option<Value>,
    importance: i16,
}

#[derive(Debug, FromRow)]
struct TripNews {
    id: Uuid,
    title: String,
    summary: String,
    url: String,
    source: String,
    tags: Vec<String>,
    image: Option<Value>,
    tone: String,
    location: Option<Value>,
}

#[derive(Debug, FromRow)]
struct ReadingNews {
    id: Uuid,
    topic_id: String,
    title: String,
    tags: Vec<String>,
    location: Option<Value>,
}

#[derive(Debug, FromRow)]
struct AlbumRow {
    trip_id: Uuid,
    day: NaiveDate,
    status: String,
    news_item_id: Option<Uuid>,
    postcard_url: String,
    diary: Option<Value>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct UnlockRow {
    accessory_id: String,
    unlocked_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct MemoryRow {
    kind: String,
    memory_key: String,
    label: String,
    times: i32,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct DueTrip {
    id: Uuid,
    cat_id: Uuid,
    news_item_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
struct EncounterSummary {
    id: Uuid,
    world_key: String,
    day: NaiveDate,
    news_item_id: Option<Uuid>,
    member_count: i64,
    member_names: Vec<String>,
}

#[derive(Debug, Serialize)]
struct IdentityPreview {
    identity_version: String,
    identity: Value,
    initial_accessories: Value,
}

fn default_reading_signal() -> String {
    "open".into()
}

pub(crate) async fn state(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let cat = load_cat(&state.pool, user.id).await?;
    let draft = if cat.is_none() {
        sqlx::query_as::<_, DraftRow>(
            "SELECT id, identity_version, identity_seed, created_at FROM cat_adoption_drafts WHERE user_id = $1",
        )
        .bind(user.id)
        .fetch_optional(&state.pool)
        .await?
    } else {
        None
    };

    let cat_id = cat.as_ref().map(|cat| cat.id);
    let cat = match cat {
        Some(cat) => {
            touch_cat(&state.pool, cat.id).await?;
            Some(cat_view(&state.pool, cat).await?)
        }
        None => None,
    };
    let unread_count = match cat_id {
        Some(cat_id) => unread_messages(&state.pool, cat_id).await?,
        None => 0,
    };
    let draft = draft.map(draft_view);
    Ok(Json(json!({
        "adopted": cat.is_some(),
        "draft": draft,
        "cat": cat,
        "unread_count": unread_count,
    })))
}

pub(crate) async fn home(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let cat = load_cat(&state.pool, user.id)
        .await?
        .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))?;
    touch_cat(&state.pool, cat.id).await?;
    let messages = load_messages(&state.pool, cat.id, false, 5).await?;
    let unread_count = unread_messages(&state.pool, cat.id).await?;
    let cat_value = cat_view(&state.pool, cat).await?;
    Ok(Json(json!({
        "cat": cat_value,
        "presence": presence_for_cat(&cat_value, state.config.timezone),
        "unread_count": unread_count,
        "messages": messages,
    })))
}

pub(crate) async fn journal(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let cat_id = cat_id_for_user(&state.pool, user.id).await?;
    touch_cat(&state.pool, cat_id).await?;
    Ok(Json(json!({
        "items": load_messages(&state.pool, cat_id, false, 80).await?,
    })))
}

pub(crate) async fn inbox(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let cat_id = cat_id_for_user(&state.pool, user.id).await?;
    touch_cat(&state.pool, cat_id).await?;
    Ok(Json(json!({
        "unread_count": unread_messages(&state.pool, cat_id).await?,
        "items": load_messages(&state.pool, cat_id, true, 50).await?,
    })))
}

pub(crate) async fn read_message(
    State(state): State<AppState>,
    user: User,
    Path(message_id): Path<Uuid>,
) -> Result<Json<Value>> {
    let cat_id = cat_id_for_user(&state.pool, user.id).await?;
    let message = sqlx::query_as::<_, (String, Option<Uuid>)>(
        "UPDATE cat_messages SET read_at = COALESCE(read_at, NOW()) WHERE id = $1 AND cat_id = $2 RETURNING kind, entity_id",
    )
    .bind(message_id)
    .bind(cat_id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some((kind, Some(trip_id))) = message
        && kind == "postcard"
    {
        let updated = sqlx::query_scalar::<_, Uuid>(
            "UPDATE cat_trips SET status = 'home', read_at = COALESCE(read_at, NOW()) WHERE id = $1 AND cat_id = $2 AND status = 'returned_unread' RETURNING id",
        )
        .bind(trip_id)
        .bind(cat_id)
        .fetch_optional(&state.pool)
        .await?;
        if updated.is_some() {
            sqlx::query(
                r#"INSERT INTO cat_growth_events (id, cat_id, event_type, source_key, payload)
                   VALUES ($1, $2, 'trip_read', $3, $4)
                   ON CONFLICT (cat_id, event_type, source_key) DO NOTHING"#,
            )
            .bind(Uuid::new_v4())
            .bind(cat_id)
            .bind(trip_id.to_string())
            .bind(json!({ "trip_id": trip_id }))
            .execute(&state.pool)
            .await?;
        }
    }
    Ok(Json(json!({
        "unread_count": unread_messages(&state.pool, cat_id).await?,
    })))
}

pub(crate) async fn record_message(
    pool: &PgPool,
    cat_id: Uuid,
    kind: &str,
    dedupe_key: &str,
    entity_id: Option<Uuid>,
    payload: Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO cat_messages (id, cat_id, kind, dedupe_key, entity_id, payload) VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(cat_id)
    .bind(kind)
    .bind(dedupe_key)
    .bind(entity_id)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(())
}

async fn unread_messages(pool: &PgPool, cat_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM cat_messages WHERE cat_id = $1 AND read_at IS NULL",
    )
    .bind(cat_id)
    .fetch_one(pool)
    .await?)
}

async fn load_messages(
    pool: &PgPool,
    cat_id: Uuid,
    unread_only: bool,
    limit: i64,
) -> Result<Vec<Value>> {
    let rows = if unread_only {
        sqlx::query_as::<_, MessageRow>(
            "SELECT id, kind, entity_id, payload, created_at, read_at FROM cat_messages WHERE cat_id = $1 AND read_at IS NULL ORDER BY created_at DESC LIMIT $2",
        )
        .bind(cat_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as::<_, MessageRow>(
            "SELECT id, kind, entity_id, payload, created_at, read_at FROM cat_messages WHERE cat_id = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(cat_id)
        .bind(limit)
        .fetch_all(pool)
        .await?
    };
    let mut messages = Vec::with_capacity(rows.len());
    for row in rows {
        let mut payload = row.payload;
        if row.kind == "postcard"
            && let Some(trip_id) = row.entity_id
            && let Some(url) = sqlx::query_scalar::<_, String>(
                "SELECT p.url FROM cat_trips t JOIN cat_postcards p ON p.id = t.postcard_id WHERE t.id = $1",
            )
            .bind(trip_id)
            .fetch_optional(pool)
            .await?
            && let Some(object) = payload.as_object_mut()
        {
            object.insert("postcard_url".into(), json!(url));
        }
        messages.push(json!({
            "id": row.id,
            "kind": row.kind,
            "entity_id": row.entity_id,
            "payload": payload,
            "created_at": row.created_at,
            "read_at": row.read_at,
        }));
    }
    Ok(messages)
}

fn presence_for_cat(cat: &Value, timezone: chrono_tz::Tz) -> &'static str {
    let trip_status = cat
        .get("current_trip")
        .and_then(|trip| trip.get("status"))
        .and_then(Value::as_str);
    if matches!(trip_status, Some("away" | "settling")) {
        let returns_at = cat["current_trip"]["returns_at"]
            .as_str()
            .unwrap_or_default();
        if let Ok(returns_at) = returns_at.parse::<DateTime<Utc>>()
            && returns_at - Utc::now() <= chrono::Duration::minutes(30)
        {
            return "returning_soon";
        }
        return "away";
    }
    if trip_status == Some("returned_unread") {
        return "returned_unread";
    }
    let hour = Utc::now().with_timezone(&timezone).hour();
    if !(6..23).contains(&hour) {
        "home_sleeping"
    } else {
        "home_awake"
    }
}

pub(crate) async fn preview(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    if load_cat(&state.pool, user.id).await?.is_some() {
        return Err(AppError::Conflict("你已经领养了一只猫咪".into()));
    }

    let mut seed = [0_u8; 32];
    fill(&mut seed).map_err(|_| AppError::Internal("无法生成猫咪身份".into()))?;
    let draft_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO cat_adoption_drafts (user_id, id, identity_version, identity_seed)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (user_id) DO UPDATE SET
            id = EXCLUDED.id,
            identity_version = EXCLUDED.identity_version,
            identity_seed = EXCLUDED.identity_seed,
            created_at = NOW()
        "#,
    )
    .bind(user.id)
    .bind(draft_id)
    .bind(IDENTITY_VERSION)
    .bind(seed.as_slice())
    .execute(&state.pool)
    .await?;

    let preview = identity_preview(IDENTITY_VERSION, &seed);
    Ok(Json(json!({
        "draft_id": draft_id,
        "created_at": Utc::now(),
        "identity_version": preview.identity_version,
        "identity": preview.identity,
        "initial_accessories": preview.initial_accessories,
    })))
}

pub(crate) async fn confirm(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<ConfirmAdoptionRequest>,
) -> Result<Json<Value>> {
    let name = validate_name(&request.name)?;
    let mut transaction = state.pool.begin().await?;

    let draft = sqlx::query_as::<_, DraftRow>(
        r#"
        SELECT id, identity_version, identity_seed, created_at
        FROM cat_adoption_drafts
        WHERE user_id = $1
        FOR UPDATE
        "#,
    )
    .bind(user.id)
    .fetch_optional(&mut *transaction)
    .await?;

    let Some(draft) = draft else {
        let existing = sqlx::query_as::<_, CatRow>(cat_select_sql())
            .bind(user.id)
            .fetch_optional(&mut *transaction)
            .await?;
        transaction.commit().await?;
        return match existing {
            Some(cat) => Ok(Json(cat_view(&state.pool, cat).await?)),
            None => Err(AppError::Conflict("领养预览不存在或已过期".into())),
        };
    };
    if draft.id != request.draft_id {
        return Err(AppError::Conflict("领养预览已更新，请重新确认".into()));
    }

    if let Some(existing) = sqlx::query_as::<_, CatRow>(cat_select_sql())
        .bind(user.id)
        .fetch_optional(&mut *transaction)
        .await?
    {
        sqlx::query("DELETE FROM cat_adoption_drafts WHERE user_id = $1")
            .bind(user.id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        return Ok(Json(cat_view(&state.pool, existing).await?));
    }

    let preview = identity_preview(&draft.identity_version, &draft.identity_seed);
    let fingerprint = identity_fingerprint(
        &draft.identity_version,
        &draft.identity_seed,
        &preview.identity,
        &preview.initial_accessories,
    );
    let cat_id = Uuid::new_v4();
    let cat = sqlx::query_as::<_, CatRow>(
        r#"
        INSERT INTO cats (
            id, owner_user_id, name, identity_version, identity_seed,
            identity_fingerprint, identity, initial_accessories, last_seen_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
        ON CONFLICT (owner_user_id) DO NOTHING
            RETURNING id, name, identity_version,
                  identity, initial_accessories,
                  adopted_at, name_updated_at, last_seen_at
        "#,
    )
    .bind(cat_id)
    .bind(user.id)
    .bind(&name)
    .bind(&draft.identity_version)
    .bind(&draft.identity_seed)
    .bind(fingerprint)
    .bind(&preview.identity)
    .bind(&preview.initial_accessories)
    .fetch_optional(&mut *transaction)
    .await?;

    let cat = match cat {
        Some(cat) => cat,
        None => {
            let existing = sqlx::query_as::<_, CatRow>(cat_select_sql())
                .bind(user.id)
                .fetch_one(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM cat_adoption_drafts WHERE user_id = $1")
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            return Ok(Json(cat_view(&state.pool, existing).await?));
        }
    };

    sqlx::query(
        r#"
        INSERT INTO cat_growth_events (id, cat_id, event_type, source_key, payload)
        VALUES ($1, $2, 'adopted', $3, $4)
        ON CONFLICT (cat_id, event_type, source_key) DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(cat.id)
    .bind(format!("adoption:{}", cat.id))
    .bind(json!({ "identity_version": cat.identity_version }))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO cat_autonomy_state (cat_id, next_guide_at, next_trip_at) VALUES ($1, NOW() + INTERVAL '2 minutes', NOW() + INTERVAL '6 hours') ON CONFLICT (cat_id) DO NOTHING",
    )
    .bind(cat.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO cat_messages (id, cat_id, kind, dedupe_key, payload) VALUES ($1, $2, 'welcome', $3, $4) ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(cat.id)
    .bind(format!("welcome:{}", cat.id))
    .bind(json!({
        "title": "欢迎来到猫窝",
        "text": format!("{}记住了你们第一次相遇。接下来，它会自己读新闻、整理行囊，也会把旅途写成信寄回来。", cat.name),
    }))
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM cat_adoption_drafts WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    Ok(Json(cat_view(&state.pool, cat).await?))
}

pub(crate) async fn rename(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<RenameRequest>,
) -> Result<Json<Value>> {
    let name = validate_name(&request.name)?;
    let cat = sqlx::query_as::<_, CatRow>(
        "UPDATE cats SET name = $1, name_updated_at = NOW(), last_seen_at = NOW() WHERE owner_user_id = $2 RETURNING id, name, identity_version, identity, initial_accessories, adopted_at, name_updated_at, last_seen_at",
    )
    .bind(name)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))?;
    Ok(Json(cat_view(&state.pool, cat).await?))
}

pub(crate) async fn action(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<ActionRequest>,
) -> Result<Json<Value>> {
    let (event_type, label) = match request.action.trim() {
        "feed" => ("care_feed", "喂食"),
        "water" => ("care_water", "喝水"),
        "toy" => ("care_toy", "玩具"),
        _ => {
            return Err(AppError::BadRequest(
                "action 必须是 feed、water 或 toy".into(),
            ));
        }
    };
    let date = state.digest.today();
    let mut transaction = state.pool.begin().await?;
    let cat_id = sqlx::query_scalar::<_, Uuid>("SELECT id FROM cats WHERE owner_user_id = $1")
        .bind(user.id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))?;
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO cat_growth_events (id, cat_id, event_type, source_key, payload)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (cat_id, event_type, source_key) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(cat_id)
    .bind(event_type)
    .bind(date.to_string())
    .bind(json!({ "action": request.action.trim(), "day": date }))
    .fetch_optional(&mut *transaction)
    .await?
    .is_some();
    sqlx::query("UPDATE cats SET last_seen_at = NOW() WHERE id = $1")
        .bind(cat_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    let cat = load_cat(&state.pool, user.id)
        .await?
        .ok_or_else(|| AppError::Internal("猫咪状态不存在".into()))?;
    Ok(Json(json!({
        "action": request.action.trim(),
        "label": label,
        "applied": inserted,
        "cat": cat_view(&state.pool, cat).await?,
    })))
}

pub(crate) async fn depart(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<DepartRequest>,
) -> Result<Json<Value>> {
    let pack = validate_pack(request.pack)?;
    let news = sqlx::query_as::<_, TripNews>(
        "SELECT items.id, items.title, items.summary, items.url, items.source, items.tags, items.image, items.tone, items.location FROM items JOIN topics ON topics.id = items.topic_id WHERE items.id = $1 AND items.hidden = FALSE AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $2)",
    )
    .bind(request.news_item_id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("旅行新闻不存在或已下线".into()))?;

    let mut transaction = state.pool.begin().await?;
    let cat_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM cats WHERE owner_user_id = $1 FOR UPDATE")
            .bind(user.id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))?;
    let active = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM cat_trips WHERE cat_id = $1 AND status IN ('away', 'settling', 'returned_unread') FOR UPDATE",
    )
    .bind(cat_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if active.is_some() {
        return Err(AppError::Conflict("猫咪已经在旅途中或正在等你读信".into()));
    }

    let day = state.digest.today();
    let news_snapshot = json!({
        "id": news.id,
        "title": news.title,
        "summary": news.summary,
        "url": news.url,
        "source": news.source,
        "tags": news.tags,
        "image": news.image,
        "tone": news.tone,
        "location": news.location,
    });
    let trip = sqlx::query_as::<_, TripRow>(
        r#"
        INSERT INTO cat_trips (id, cat_id, news_item_id, day, status, pack, tone, location, returns_at, origin, news_snapshot)
        VALUES ($1, $2, $3, $4, 'away', $5, $6, $7, NOW() + INTERVAL '2 hours', 'manual', $8)
        RETURNING id, cat_id, news_item_id, day, status, pack, departed_at, returns_at,
                  tone, location, settled_at, read_at, postcard_id, diary, souvenir, error,
                  origin, attempts, news_snapshot, note
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(cat_id)
    .bind(request.news_item_id)
    .bind(day)
    .bind(&pack)
    .bind(&news.tone)
    .bind(&news.location)
    .bind(&news_snapshot)
    .fetch_one(&mut *transaction)
    .await?;

    let encounter_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO cat_encounters (id, world_key, day, news_item_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (world_key, day) DO UPDATE SET
            news_item_id = COALESCE(cat_encounters.news_item_id, EXCLUDED.news_item_id)
        RETURNING id
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(format!("item:{}", request.news_item_id))
    .bind(day)
    .bind(request.news_item_id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO cat_encounter_members (encounter_id, cat_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(encounter_id)
    .bind(cat_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    record_message(
        &state.pool,
        cat_id,
        "travel",
        &format!("travel:{}:departed", trip.id),
        Some(trip.id),
        json!({ "title": "猫咪出发了", "text": "它自己整理好行囊，去看看今天的世界。", "trip_id": trip.id }),
    )
    .await?;

    Ok(Json(json!({
        "trip": trip_view(&state.pool, trip).await?,
        "encounter_id": encounter_id,
    })))
}

pub(crate) async fn list_trips(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let rows = sqlx::query_as::<_, TripRow>(
        r#"
        SELECT t.id, t.cat_id, t.news_item_id, t.day, t.status, t.pack, t.tone, t.location,
               t.departed_at, t.returns_at, t.settled_at, t.read_at, t.postcard_id,
               t.diary, t.souvenir, t.error, t.origin, t.attempts, t.news_snapshot, t.note
        FROM cat_trips t
        JOIN cats c ON c.id = t.cat_id
        WHERE c.owner_user_id = $1
        ORDER BY t.departed_at DESC
        LIMIT 30
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    let mut trips = Vec::with_capacity(rows.len());
    for row in rows {
        trips.push(trip_view(&state.pool, row).await?);
    }
    Ok(Json(json!({ "trips": trips })))
}

pub(crate) async fn read_trip(
    State(state): State<AppState>,
    user: User,
    Path(trip_id): Path<Uuid>,
) -> Result<Json<Value>> {
    let mut transaction = state.pool.begin().await?;
    let cat_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM cats WHERE owner_user_id = $1 FOR UPDATE")
            .bind(user.id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))?;
    let updated = sqlx::query_scalar::<_, Uuid>(
        "UPDATE cat_trips SET status = 'home', read_at = NOW() WHERE id = $1 AND cat_id = $2 AND status = 'returned_unread' RETURNING id",
    )
    .bind(trip_id)
    .bind(cat_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if updated.is_none() {
        let status = sqlx::query_scalar::<_, String>(
            "SELECT status FROM cat_trips WHERE id = $1 AND cat_id = $2",
        )
        .bind(trip_id)
        .bind(cat_id)
        .fetch_optional(&mut *transaction)
        .await?;
        if status.as_deref() != Some("home") {
            return Err(AppError::NotFound("这封旅程来信还没有回来".into()));
        }
    } else {
        sqlx::query(
            r#"
            INSERT INTO cat_growth_events (id, cat_id, event_type, source_key, payload)
            VALUES ($1, $2, 'trip_read', $3, $4)
            ON CONFLICT (cat_id, event_type, source_key) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(cat_id)
        .bind(trip_id.to_string())
        .bind(json!({ "trip_id": trip_id }))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    sqlx::query(
        "UPDATE cat_messages SET read_at = COALESCE(read_at, NOW()) WHERE cat_id = $1 AND entity_id = $2 AND kind = 'postcard'",
    )
    .bind(cat_id)
    .bind(trip_id)
    .execute(&state.pool)
    .await?;
    let cat = load_cat(&state.pool, user.id)
        .await?
        .ok_or_else(|| AppError::Internal("猫咪状态不存在".into()))?;
    Ok(Json(cat_view(&state.pool, cat).await?))
}

pub(crate) async fn reading(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<ReadingRequest>,
) -> Result<Json<Value>> {
    let signal = request.signal.trim();
    if !matches!(signal, "open" | "view") {
        return Err(AppError::BadRequest("signal 必须是 open 或 view".into()));
    }
    let day = state.digest.today();
    let mut transaction = state.pool.begin().await?;
    let cat_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM cats WHERE owner_user_id = $1 FOR UPDATE")
            .bind(user.id)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))?;
    let item = sqlx::query_as::<_, ReadingNews>(
        "SELECT items.id, items.topic_id, items.title, items.tags, items.location FROM items JOIN topics ON topics.id = items.topic_id WHERE items.id = $1 AND items.hidden = FALSE AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $2)",
    )
    .bind(request.news_item_id)
    .bind(user.id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| AppError::NotFound("这条新闻不存在或已下线".into()))?;
    // A reading is one growth step per item and day. The signal is retained in
    // the event payload for future analytics, but opening and viewing the same
    // story must not create two growth steps.
    let source_key = format!("{day}:{}", item.id);
    let applied = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO cat_growth_events (id, cat_id, event_type, source_key, payload)
        SELECT $1, $2, 'reading_opened', $3, $4
        WHERE NOT EXISTS (
            SELECT 1
            FROM cat_growth_events
            WHERE cat_id = $2
              AND event_type = 'reading_opened'
              AND (source_key = $3 OR source_key LIKE $3 || ':%')
        )
        ON CONFLICT (cat_id, event_type, source_key) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(cat_id)
    .bind(&source_key)
    .bind(json!({ "news_item_id": item.id, "signal": signal, "day": day }))
    .fetch_optional(&mut *transaction)
    .await?
    .is_some();
    if applied {
        let mut memories = vec![
            ("item", item.id.to_string(), item.title.clone()),
            ("topic", item.topic_id.clone(), item.topic_id.clone()),
        ];
        memories.extend(
            item.tags
                .iter()
                .map(|tag| ("tag", tag.clone(), tag.clone())),
        );
        if let Some(location) = item
            .location
            .as_ref()
            .and_then(|value| value.get("name"))
            .and_then(Value::as_str)
        {
            memories.push(("place", location.to_owned(), location.to_owned()));
        }
        for (kind, key, label) in memories {
            sqlx::query(
                r#"
                INSERT INTO cat_memories (cat_id, kind, memory_key, label)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (cat_id, kind, memory_key) DO UPDATE SET
                    label = EXCLUDED.label, last_seen_at = NOW(), times = cat_memories.times + 1
                "#,
            )
            .bind(cat_id)
            .bind(kind)
            .bind(key)
            .bind(label)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query(
            r#"
            INSERT INTO cat_reading_daily (cat_id, day, topics, item_ids, read_count)
            VALUES ($1, $2, jsonb_build_object($3, 1), ARRAY[$4]::uuid[], 1)
            ON CONFLICT (cat_id, day) DO UPDATE SET
                topics = cat_reading_daily.topics || jsonb_build_object(
                    $3, COALESCE((cat_reading_daily.topics ->> $3)::integer, 0) + 1
                ),
                item_ids = CASE
                    WHEN $4 = ANY(cat_reading_daily.item_ids) THEN cat_reading_daily.item_ids
                    ELSE array_append(cat_reading_daily.item_ids, $4)
                END,
                read_count = cat_reading_daily.read_count + 1
            "#,
        )
        .bind(cat_id)
        .bind(day)
        .bind(&item.topic_id)
        .bind(item.id)
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query("UPDATE cats SET last_seen_at = NOW() WHERE id = $1")
        .bind(cat_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    let cat = load_cat(&state.pool, user.id)
        .await?
        .ok_or_else(|| AppError::Internal("猫咪状态不存在".into()))?;
    Ok(Json(json!({
        "applied": applied,
        "cat": cat_view(&state.pool, cat).await?,
    })))
}

pub(crate) async fn album(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let cat_id = cat_id_for_user(&state.pool, user.id).await?;
    let rows = sqlx::query_as::<_, AlbumRow>(
        r#"
        SELECT t.id AS trip_id, t.day, t.status, t.news_item_id, p.url AS postcard_url,
               t.diary, COALESCE(t.settled_at, t.departed_at) AS created_at
        FROM cat_trips t
        JOIN cat_postcards p ON p.id = t.postcard_id
        WHERE t.cat_id = $1
        ORDER BY COALESCE(t.settled_at, t.departed_at) DESC
        LIMIT 60
        "#,
    )
    .bind(cat_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!({
        "items": rows.into_iter().map(|row| json!({
            "trip_id": row.trip_id,
            "day": row.day,
            "status": row.status,
            "news_item_id": row.news_item_id,
            "postcard_url": row.postcard_url,
            "diary": row.diary,
            "created_at": row.created_at,
        })).collect::<Vec<_>>(),
    })))
}

pub(crate) async fn atlas(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let cat_id = cat_id_for_user(&state.pool, user.id).await?;
    sync_accessory_unlocks(&state.pool, cat_id).await?;
    Ok(Json(json!({
        "accessories": load_unlocks(&state.pool, cat_id).await?,
        "memories": load_memories(&state.pool, cat_id).await?,
    })))
}

pub(crate) async fn encounters(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let cat_id = cat_id_for_user(&state.pool, user.id).await?;
    Ok(Json(json!({
        "encounters": load_encounters(&state.pool, cat_id).await?,
    })))
}

/// Claims a small batch of autonomous cats. Claims are persisted so multiple
/// application instances cannot make the same cat travel twice.
pub(crate) async fn autonomous_tick(state: &AppState) -> Result<()> {
    let slots = state.cat_slots.available_permits().min(4);
    if slots == 0 {
        return Ok(());
    }
    let mut transaction = state.pool.begin().await?;
    let cats = sqlx::query_as::<_, AutoCat>(
        r#"
        SELECT a.cat_id
        FROM cat_autonomy_state a
        JOIN cats c ON c.id = a.cat_id
        WHERE (a.next_guide_at <= NOW() OR a.next_trip_at <= NOW())
          AND (a.lease_until IS NULL OR a.lease_until < NOW())
          AND COALESCE(c.last_seen_at, c.adopted_at) >= NOW() - INTERVAL '48 hours'
        ORDER BY LEAST(a.next_guide_at, a.next_trip_at)
        LIMIT $1
        FOR UPDATE OF a SKIP LOCKED
        "#,
    )
    .bind(slots as i64)
    .fetch_all(&mut *transaction)
    .await?;
    let mut claims = Vec::with_capacity(cats.len());
    for cat in cats {
        let token = Uuid::new_v4();
        sqlx::query(
            "UPDATE cat_autonomy_state SET lease_until = NOW() + INTERVAL '10 minutes', lease_token = $2, updated_at = NOW() WHERE cat_id = $1",
        )
        .bind(cat.cat_id)
        .bind(token)
        .execute(&mut *transaction)
        .await?;
        claims.push((cat.cat_id, token));
    }
    transaction.commit().await?;

    for (cat_id, token) in claims {
        let Ok(permit) = state.cat_slots.clone().try_acquire_owned() else {
            break;
        };
        let worker_state = state.clone();
        tokio::spawn(async move {
            let result = autonomous_one(&worker_state, cat_id).await;
            if let Err(error) = result {
                tracing::warn!(%error, %cat_id, "autonomous cat worker failed");
            }
            let _ = sqlx::query(
                "UPDATE cat_autonomy_state SET lease_until = NULL, lease_token = NULL, updated_at = NOW() WHERE cat_id = $1 AND lease_token = $2",
            )
            .bind(cat_id)
            .bind(token)
            .execute(&worker_state.pool)
            .await;
            drop(permit);
        });
    }
    Ok(())
}

async fn autonomous_one(state: &AppState, cat_id: Uuid) -> Result<()> {
    let due_guide = sqlx::query_scalar::<_, bool>(
        "SELECT next_guide_at <= NOW() FROM cat_autonomy_state WHERE cat_id = $1",
    )
    .bind(cat_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(false);
    if due_guide {
        let day = state.digest.today();
        match crate::cat_guide::ensure_today(state, cat_id, day).await {
            Ok(_) => {
                sqlx::query(
                    "UPDATE cat_autonomy_state SET next_guide_at = NOW() + INTERVAL '24 hours', last_guide_at = NOW(), failures = 0 WHERE cat_id = $1",
                )
                .bind(cat_id)
                .execute(&state.pool)
                .await?;
            }
            Err(error) => {
                tracing::debug!(%error, %cat_id, "autonomous guide deferred");
                sqlx::query(
                    "UPDATE cat_autonomy_state SET next_guide_at = NOW() + INTERVAL '30 minutes', failures = LEAST(failures + 1, 20) WHERE cat_id = $1",
                )
                .bind(cat_id)
                .execute(&state.pool)
                .await?;
            }
        }
    }

    let due_trip = sqlx::query_scalar::<_, bool>(
        "SELECT next_trip_at <= NOW() FROM cat_autonomy_state WHERE cat_id = $1",
    )
    .bind(cat_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or(false);
    if due_trip {
        match auto_depart(state, cat_id).await? {
            true => {
                let offset_hours = 48 + (cat_id.as_u128() % 25) as i64;
                sqlx::query(
                    "UPDATE cat_autonomy_state SET next_trip_at = NOW() + ($2 * INTERVAL '1 hour'), last_trip_at = NOW(), failures = 0 WHERE cat_id = $1",
                )
                .bind(cat_id)
                .bind(offset_hours)
                .execute(&state.pool)
                .await?;
            }
            false => {
                sqlx::query(
                    "UPDATE cat_autonomy_state SET next_trip_at = NOW() + INTERVAL '6 hours' WHERE cat_id = $1",
                )
                .bind(cat_id)
                .execute(&state.pool)
                .await?;
            }
        }
    }
    Ok(())
}

async fn auto_depart(state: &AppState, cat_id: Uuid) -> Result<bool> {
    let Some(news) = sqlx::query_as::<_, AutoNews>(
        r#"
        SELECT items.id, items.title, items.summary, items.url, items.source,
               items.tags, items.tone, items.location, items.image, items.importance
        FROM items
        JOIN topics ON topics.id = items.topic_id
        WHERE items.hidden = FALSE
          AND topics.owner_user_id IS NULL
          AND items.first_seen_at >= NOW() - INTERVAL '48 hours'
          AND items.tone IN ('joyful', 'curious')
          AND NOT EXISTS (
              SELECT 1 FROM cat_trips
              WHERE cat_id = $1 AND news_item_id = items.id
          )
        ORDER BY (items.image IS NOT NULL) DESC, items.importance DESC, items.first_seen_at DESC
        LIMIT 1
        "#,
    )
    .bind(cat_id)
    .fetch_optional(&state.pool)
    .await?
    else {
        return Ok(false);
    };

    let mut pack = vec!["food".to_owned(), "water".to_owned()];
    if news.image.is_some() {
        pack.push("camera".to_owned());
    } else if news.location.is_some() {
        pack.push("map".to_owned());
    } else {
        pack.push("notebook".to_owned());
    }
    let day = state.digest.today();
    let trip_id = Uuid::new_v4();
    let returns_minutes = 90 + (cat_id.as_u128() % 91) as i64;
    let snapshot = json!({
        "id": news.id,
        "title": news.title,
        "summary": news.summary,
        "url": news.url,
        "source": news.source,
        "tags": news.tags,
        "tone": news.tone,
        "location": news.location,
        "image": news.image,
        "importance": news.importance,
    });
    let note = json!({
        "zh": format!("我去看看{}，回来给你写信。", news.location.as_ref().and_then(|value| value.get("name")).and_then(Value::as_str).unwrap_or("这条消息的远方")),
        "en": "I am going to see this part of the world and write back.",
    });
    let mut transaction = state.pool.begin().await?;
    let active = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM cat_trips WHERE cat_id = $1 AND status IN ('away', 'settling', 'returned_unread') FOR UPDATE",
    )
    .bind(cat_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if active.is_some() {
        transaction.commit().await?;
        return Ok(false);
    }
    sqlx::query(
        r#"
        INSERT INTO cat_trips (
            id, cat_id, news_item_id, day, status, pack, returns_at,
            tone, location, origin, news_snapshot, note
        )
        VALUES ($1, $2, $3, $4, 'away', $5, NOW() + ($6 * INTERVAL '1 minute'), $7, $8, 'autonomous', $9, $10)
        "#,
    )
    .bind(trip_id)
    .bind(cat_id)
    .bind(news.id)
    .bind(day)
    .bind(&pack)
    .bind(returns_minutes)
    .bind(&news.tone)
    .bind(&news.location)
    .bind(&snapshot)
    .bind(&note)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO cat_growth_events (id, cat_id, event_type, source_key, payload) VALUES ($1, $2, 'trip_departed', $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(cat_id)
    .bind(trip_id.to_string())
    .bind(json!({ "trip_id": trip_id, "news_item_id": news.id, "origin": "autonomous" }))
    .execute(&mut *transaction)
    .await?;
    let encounter_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO cat_encounters (id, world_key, day, news_item_id) VALUES ($1, $2, $3, $4) ON CONFLICT (world_key, day) DO UPDATE SET news_item_id = COALESCE(cat_encounters.news_item_id, EXCLUDED.news_item_id) RETURNING id",
    )
    .bind(Uuid::new_v4())
    .bind(format!("item:{}", news.id))
    .bind(day)
    .bind(news.id)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO cat_encounter_members (encounter_id, cat_id, phrase_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(encounter_id)
    .bind(cat_id)
    .bind("wind_is_big")
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    record_message(
        &state.pool,
        cat_id,
        "travel",
        &format!("travel:{trip_id}:departed"),
        Some(trip_id),
        json!({ "title": "猫咪出发了", "text": note["zh"], "trip_id": trip_id, "news_snapshot": snapshot }),
    )
    .await?;
    Ok(true)
}

/// Claims a bounded batch of due trips. The maintenance loop calls this every minute;
/// the status transition makes a restart or a second worker harmless.
pub(crate) async fn settle_due(state: &AppState) -> Result<()> {
    let slots = state.cat_settle_slots.available_permits().min(4);
    if slots == 0 {
        return Ok(());
    }
    let ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM cat_trips WHERE (status = 'away' AND returns_at <= NOW()) OR (status = 'settling' AND settling_at <= NOW() - INTERVAL '15 minutes') ORDER BY returns_at LIMIT $1",
    )
    .bind(slots as i64)
    .fetch_all(&state.pool)
    .await?;
    for id in ids {
        let worker_state = state.clone();
        let Ok(permit) = state.cat_settle_slots.clone().try_acquire_owned() else {
            break;
        };
        tokio::spawn(async move {
            if let Err(error) = settle_one(&worker_state, id).await {
                tracing::warn!(%error, %id, "travel cat trip settlement failed");
                let attempts =
                    sqlx::query_scalar::<_, i32>("SELECT attempts FROM cat_trips WHERE id = $1")
                        .bind(id)
                        .fetch_optional(&worker_state.pool)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or(0);
                if attempts >= 3 {
                    let diary = json!({
                        "zh": "我已经回到猫窝了，但照片冲洗得有点慢。先把这封信交给你，等图片服务恢复后我再补上。",
                        "en": "I am back home. The photo lab is taking longer, so I am leaving you this letter first.",
                    });
                    let _ = sqlx::query(
                        "UPDATE cat_trips SET status = 'returned_unread', settled_at = NOW(), settling_at = NULL, error = $2, diary = COALESCE(diary, $3) WHERE id = $1 AND status = 'settling'",
                    )
                    .bind(id)
                    .bind(error.to_string())
                    .bind(&diary)
                    .execute(&worker_state.pool)
                    .await;
                    if let Some(cat_id) =
                        sqlx::query_scalar::<_, Uuid>("SELECT cat_id FROM cat_trips WHERE id = $1")
                            .bind(id)
                            .fetch_optional(&worker_state.pool)
                            .await
                            .ok()
                            .flatten()
                    {
                        let _ = record_message(
                            &worker_state.pool,
                            cat_id,
                            "postcard",
                            &format!("travel:{id}:returned"),
                            Some(id),
                            json!({ "title": "猫咪回来了", "text": diary["zh"], "trip_id": id, "diary": diary }),
                        )
                        .await;
                    }
                } else {
                    let _ = sqlx::query(
                        "UPDATE cat_trips SET status = 'away', settling_at = NULL, returns_at = GREATEST(returns_at, NOW() + INTERVAL '5 minutes'), error = $2 WHERE id = $1 AND status = 'settling'",
                    )
                    .bind(id)
                    .bind(error.to_string())
                    .execute(&worker_state.pool)
                    .await;
                }
            }
            drop(permit);
        });
    }
    Ok(())
}

async fn settle_one(state: &AppState, trip_id: Uuid) -> Result<()> {
    let claimed = sqlx::query_as::<_, DueTrip>(
        r#"
        UPDATE cat_trips
        SET status = 'settling', settling_at = NOW(), attempts = attempts + 1
        WHERE id = $1 AND (
            (status = 'away' AND returns_at <= NOW())
            OR (status = 'settling' AND settling_at <= NOW() - INTERVAL '15 minutes')
        )
        RETURNING id, cat_id, news_item_id
        "#,
    )
    .bind(trip_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(trip) = claimed else {
        return Ok(());
    };

    let snapshot =
        sqlx::query_scalar::<_, Option<Value>>("SELECT news_snapshot FROM cat_trips WHERE id = $1")
            .bind(trip.id)
            .fetch_one(&state.pool)
            .await?
            .unwrap_or_else(|| json!({}));

    let owner_id = sqlx::query_scalar::<_, i64>("SELECT owner_user_id FROM cats WHERE id = $1")
        .bind(trip.cat_id)
        .fetch_one(&state.pool)
        .await?;
    let mut postcard_id = None;
    let mut settlement_error = None;
    if let Some(news_item_id) = trip.news_item_id {
        let source_key = format!("cat:{}:news:{}", trip.cat_id, news_item_id);
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM cat_postcards WHERE source_key = $1 AND (expires_at IS NULL OR expires_at > NOW())",
        )
        .bind(&source_key)
        .fetch_optional(&state.pool)
        .await?;
        if existing.is_some() {
            postcard_id = existing;
        } else {
            let request = crate::cat_image::GenerateRequest {
                news_item_id,
                quality: Some("medium".into()),
                purpose: Some("回家纪念".into()),
            };
            match crate::cat_image::generate_for_owner(state, owner_id, &request, false, false)
                .await
            {
                Ok(_) => {
                    postcard_id = Some(
                        sqlx::query_scalar::<_, Uuid>(
                            "SELECT id FROM cat_postcards WHERE source_key = $1",
                        )
                        .bind(&source_key)
                        .fetch_one(&state.pool)
                        .await?,
                    );
                }
                Err(error) if retryable_settlement_error(&error) => return Err(error),
                Err(error) => settlement_error = Some(error.to_string()),
            }
        }
    }
    let title = snapshot
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("今天世界的一角");
    let location = snapshot
        .get("location")
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("远方");
    let source_url = snapshot.get("url").and_then(Value::as_str).unwrap_or("");
    let diary = json!({
        "zh": format!("我去了{location}，看见了《{title}》背后的世界。事实先放在桌上：这是一条来自 NEWS 的记录，我们可以沿着来源继续了解。"),
        "en": format!("I visited {location} and brought back a closer look at {title}. The fact comes from a NEWS source, so we can keep following the story."),
        "fact": title,
        "source_url": source_url,
        "location": location,
    });
    sqlx::query(
        r#"
        UPDATE cat_trips
        SET status = 'returned_unread', settled_at = NOW(), postcard_id = $2,
            settling_at = NULL, error = $3, diary = COALESCE(diary, $4)
        WHERE id = $1 AND status = 'settling'
        "#,
    )
    .bind(trip.id)
    .bind(postcard_id)
    .bind(settlement_error)
    .bind(&diary)
    .execute(&state.pool)
    .await?;
    record_message(
        &state.pool,
        trip.cat_id,
        "postcard",
        &format!("travel:{}:returned", trip.id),
        Some(trip.id),
        json!({
            "title": "猫咪回来了",
            "text": diary["zh"],
            "trip_id": trip.id,
            "postcard_id": postcard_id,
            "postcard_url": match postcard_id {
                Some(id) => sqlx::query_scalar::<_, String>("SELECT url FROM cat_postcards WHERE id = $1").bind(id).fetch_optional(&state.pool).await?,
                None => None,
            },
            "diary": diary,
        }),
    )
    .await?;
    Ok(())
}

fn retryable_settlement_error(error: &AppError) -> bool {
    matches!(
        error,
        AppError::TooManyRequests(_)
            | AppError::ServiceUnavailable(_)
            | AppError::Http(_)
            | AppError::Sqlx(_)
            | AppError::Redis(_)
    )
}

fn cat_select_sql() -> &'static str {
    "SELECT id, name, identity_version, identity, initial_accessories, adopted_at, name_updated_at, last_seen_at FROM cats WHERE owner_user_id = $1 FOR UPDATE"
}

async fn cat_id_for_user(pool: &PgPool, user_id: i64) -> Result<Uuid> {
    sqlx::query_scalar::<_, Uuid>("SELECT id FROM cats WHERE owner_user_id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))
}

pub(crate) async fn touch_cat(pool: &PgPool, cat_id: Uuid) -> Result<()> {
    sqlx::query(
        "UPDATE cats SET last_seen_at = NOW() WHERE id = $1 AND (last_seen_at IS NULL OR last_seen_at < NOW() - INTERVAL '30 minutes')",
    )
    .bind(cat_id)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO cat_autonomy_state (cat_id) VALUES ($1) ON CONFLICT (cat_id) DO NOTHING",
    )
    .bind(cat_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn load_cat(pool: &PgPool, user_id: i64) -> Result<Option<CatRow>> {
    Ok(
        sqlx::query_as::<_, CatRow>(cat_select_sql().trim_end_matches(" FOR UPDATE"))
            .bind(user_id)
            .fetch_optional(pool)
            .await?,
    )
}

fn draft_view(draft: DraftRow) -> Value {
    let preview = identity_preview(&draft.identity_version, &draft.identity_seed);
    json!({
        "draft_id": draft.id,
        "created_at": draft.created_at,
        "identity_version": preview.identity_version,
        "identity": preview.identity,
        "initial_accessories": preview.initial_accessories,
    })
}

const ACCESSORY_UNLOCKS: [(i64, &str, &str); 4] = [
    (4, "travel_cap", "旅行帽"),
    (8, "postcard_pouch", "明信片小挎包"),
    (12, "field_camera", "旅行相机"),
    (20, "star_beret", "星星贝雷帽"),
];

async fn sync_accessory_unlocks(pool: &PgPool, cat_id: Uuid) -> Result<()> {
    let progress = growth_progress(pool, cat_id).await?;
    for (threshold, accessory_id, label) in ACCESSORY_UNLOCKS {
        if progress < threshold {
            continue;
        }
        let source_key = format!("accessory:{accessory_id}");
        let event_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO cat_growth_events (id, cat_id, event_type, source_key, payload)
            VALUES ($1, $2, 'accessory_unlocked', $3, $4)
            ON CONFLICT (cat_id, event_type, source_key) DO NOTHING
            RETURNING id
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(cat_id)
        .bind(&source_key)
        .bind(json!({ "accessory_id": accessory_id, "name": label, "threshold": threshold }))
        .fetch_optional(pool)
        .await?;
        let event_id = match event_id {
            Some(id) => id,
            None => sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM cat_growth_events WHERE cat_id = $1 AND event_type = 'accessory_unlocked' AND source_key = $2",
            )
            .bind(cat_id)
            .bind(&source_key)
            .fetch_one(pool)
            .await?,
        };
        sqlx::query(
            "INSERT INTO cat_accessory_unlocks (cat_id, accessory_id, source_event) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(cat_id)
        .bind(accessory_id)
        .bind(event_id)
        .execute(pool)
        .await?;
        record_message(
            pool,
            cat_id,
            "growth",
            &format!("growth:accessory:{cat_id}:{accessory_id}"),
            Some(event_id),
            json!({ "title": format!("解锁了{}", label), "text": format!("{}把这件配饰收进了自己的旅途。", label) }),
        )
        .await?;
    }
    Ok(())
}

async fn growth_progress(pool: &PgPool, cat_id: Uuid) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"
        SELECT
            (SELECT COUNT(*)::bigint
             FROM cat_growth_events
             WHERE cat_id = $1
               AND event_type NOT IN ('accessory_unlocked', 'reading_opened'))
          + (SELECT COUNT(DISTINCT (payload ->> 'day') || ':' || (payload ->> 'news_item_id'))::bigint
             FROM cat_growth_events
             WHERE cat_id = $1
               AND event_type = 'reading_opened'
               AND payload ? 'day'
               AND payload ? 'news_item_id')
        "#,
    )
    .bind(cat_id)
    .fetch_one(pool)
    .await?)
}

async fn load_unlocks(pool: &PgPool, cat_id: Uuid) -> Result<Vec<Value>> {
    let rows = sqlx::query_as::<_, UnlockRow>(
        "SELECT accessory_id, unlocked_at FROM cat_accessory_unlocks WHERE cat_id = $1 ORDER BY unlocked_at",
    )
    .bind(cat_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.accessory_id,
                "unlocked_at": row.unlocked_at,
            })
        })
        .collect())
}

async fn load_memories(pool: &PgPool, cat_id: Uuid) -> Result<Vec<Value>> {
    let rows = sqlx::query_as::<_, MemoryRow>(
        "SELECT kind, memory_key, label, times, last_seen_at FROM cat_memories WHERE cat_id = $1 ORDER BY last_seen_at DESC LIMIT 60",
    )
    .bind(cat_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "kind": row.kind,
                "key": row.memory_key,
                "label": row.label,
                "times": row.times,
                "last_seen_at": row.last_seen_at,
            })
        })
        .collect())
}

async fn load_encounters(pool: &PgPool, cat_id: Uuid) -> Result<Vec<Value>> {
    let rows = sqlx::query_as::<_, EncounterSummary>(
        r#"
        SELECT e.id, e.world_key, e.day, e.news_item_id,
               COUNT(m.cat_id)::bigint AS member_count,
               COALESCE(
                   ARRAY_AGG(c.name ORDER BY c.name) FILTER (WHERE c.id IS NOT NULL),
                   ARRAY[]::text[]
               ) AS member_names
        FROM cat_encounters e
        JOIN cat_encounter_members mine ON mine.encounter_id = e.id AND mine.cat_id = $1
        JOIN cat_encounter_members m ON m.encounter_id = e.id
        JOIN cats c ON c.id = m.cat_id
        GROUP BY e.id, e.world_key, e.day, e.news_item_id
        ORDER BY e.day DESC, e.id DESC
        LIMIT 24
        "#,
    )
    .bind(cat_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            json!({
                "id": row.id,
                "world_key": row.world_key,
                "day": row.day,
                "news_item_id": row.news_item_id,
                "member_count": row.member_count,
                "member_names": row.member_names,
            })
        })
        .collect())
}

async fn cat_view(pool: &PgPool, cat: CatRow) -> Result<Value> {
    sync_accessory_unlocks(pool, cat.id).await?;
    let counts = sqlx::query_as::<_, GrowthCount>(
        "SELECT event_type, COUNT(*)::bigint AS count FROM cat_growth_events WHERE cat_id = $1 GROUP BY event_type ORDER BY event_type",
    )
    .bind(cat.id)
    .fetch_all(pool)
    .await?;
    let total_events = growth_progress(pool, cat.id).await?;
    let events = counts
        .into_iter()
        .map(|item| (item.event_type, json!(item.count)))
        .collect::<serde_json::Map<String, Value>>();
    let current_trip = sqlx::query_as::<_, TripRow>(
        r#"
        SELECT id, cat_id, news_item_id, day, status, pack, tone, location, departed_at,
               returns_at, settled_at, read_at, postcard_id, diary, souvenir, error,
               origin, attempts, news_snapshot, note
        FROM cat_trips
        WHERE cat_id = $1 AND status IN ('away', 'settling', 'returned_unread')
        ORDER BY departed_at DESC
        LIMIT 1
        "#,
    )
    .bind(cat.id)
    .fetch_optional(pool)
    .await?;
    let current_trip = match current_trip {
        Some(trip) => Some(trip_view(pool, trip).await?),
        None => None,
    };
    let encounters = load_encounters(pool, cat.id).await?;
    let unlocked_accessories = load_unlocks(pool, cat.id).await?;
    let memories = load_memories(pool, cat.id).await?;
    Ok(json!({
        "id": cat.id,
        "name": cat.name,
        "identity_version": cat.identity_version,
        "identity": cat.identity,
        "initial_accessories": cat.initial_accessories,
        "unlocked_accessories": unlocked_accessories,
        "memories": memories,
        "adopted_at": cat.adopted_at,
        "name_updated_at": cat.name_updated_at,
        "last_seen_at": cat.last_seen_at,
        "growth": {
            "stage": stage_for_events(total_events),
            "event_count": total_events,
            "events": events,
        },
        "current_trip": current_trip,
        "encounters": encounters,
    }))
}

fn validate_pack(pack: Vec<String>) -> Result<Vec<String>> {
    if pack.len() > 3 {
        return Err(AppError::BadRequest("一次最多准备三件行囊物品".into()));
    }
    let mut result = Vec::with_capacity(pack.len());
    for item in pack {
        let item = item.trim();
        if !matches!(
            item,
            "food" | "water" | "toy" | "camera" | "notebook" | "map"
        ) {
            return Err(AppError::BadRequest("行囊物品不在可用清单中".into()));
        }
        if !result.iter().any(|existing| existing == item) {
            result.push(item.to_owned());
        }
    }
    Ok(result)
}

async fn trip_view(pool: &PgPool, trip: TripRow) -> Result<Value> {
    let postcard_url = match trip.postcard_id {
        Some(postcard_id) => {
            sqlx::query_scalar::<_, String>("SELECT url FROM cat_postcards WHERE id = $1")
                .bind(postcard_id)
                .fetch_optional(pool)
                .await?
        }
        None => None,
    };
    Ok(json!({
        "id": trip.id,
        "cat_id": trip.cat_id,
        "news_item_id": trip.news_item_id,
        "day": trip.day,
        "status": trip.status,
        "pack": trip.pack,
        "tone": trip.tone,
        "location": trip.location,
        "departed_at": trip.departed_at,
        "returns_at": trip.returns_at,
        "settled_at": trip.settled_at,
        "read_at": trip.read_at,
        "postcard_id": trip.postcard_id,
        "postcard_url": postcard_url,
        "diary": trip.diary,
        "souvenir": trip.souvenir,
        "error": trip.error,
        "origin": trip.origin,
        "attempts": trip.attempts,
        "news_snapshot": trip.news_snapshot,
        "note": trip.note,
    }))
}

fn identity_preview(version: &str, seed: &[u8]) -> IdentityPreview {
    let digest = Sha256::digest(seed);
    let coats = ["orange_white", "cream_tabby", "silver_tabby", "black_white"];
    let eyes = ["amber", "green", "blue", "hazel"];
    let markings = ["star", "stripe", "cloud", "heart"];
    let accents = [
        ("bronze_bell", "铜色铃铛", "neck"),
        ("canvas_satchel", "帆布小挎包", "shoulder"),
        ("blue_ribbon", "晴蓝缎带", "neck"),
        ("round_glasses", "圆框眼镜", "face"),
    ];
    let accent = accents[usize::from(digest[3]) % accents.len()];
    IdentityPreview {
        identity_version: if version.is_empty() {
            IDENTITY_VERSION.to_owned()
        } else {
            // M0 has one derivation version. Unknown versions stay visible but do not
            // silently change into a different identity recipe.
            version.to_owned()
        },
        identity: json!({
            "species": "domestic_cat",
            "coat": coats[usize::from(digest[0]) % coats.len()],
            "eye_color": eyes[usize::from(digest[1]) % eyes.len()],
            "marking": markings[usize::from(digest[2]) % markings.len()],
            "signature": hex::encode(&digest[..6]),
            "identity_seed_version": version,
        }),
        initial_accessories: json!([
            { "id": "sage_scarf", "name": "鼠尾草绿围巾", "slot": "neck", "fixed": true },
            { "id": accent.0, "name": accent.1, "slot": accent.2, "fixed": true }
        ]),
    }
}

fn identity_fingerprint(
    version: &str,
    seed: &[u8],
    identity: &Value,
    initial_accessories: &Value,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update([0]);
    hasher.update(seed);
    hasher.update([0]);
    hasher.update(serde_json::to_vec(identity).expect("identity is serialisable"));
    hasher.update([0]);
    hasher.update(serde_json::to_vec(initial_accessories).expect("accessories are serialisable"));
    hasher.finalize().to_vec()
}

fn validate_name(value: &str) -> Result<String> {
    let name = value.trim();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(AppError::BadRequest("猫咪名字需为 1 至 24 个字符".into()));
    }
    Ok(name.to_owned())
}

fn stage_for_events(events: i64) -> &'static str {
    match events {
        0..=3 => "kitten",
        4..=8 => "young",
        9..=20 => "traveler",
        _ => "old_friend",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IDENTITY_VERSION, identity_fingerprint, identity_preview, stage_for_events, validate_name,
    };

    #[test]
    fn identity_is_deterministic_and_fingerprint_includes_seed() {
        let seed = [7_u8; 32];
        let first = identity_preview(IDENTITY_VERSION, &seed);
        let second = identity_preview(IDENTITY_VERSION, &seed);
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.initial_accessories, second.initial_accessories);
        assert_eq!(
            identity_fingerprint(
                IDENTITY_VERSION,
                &seed,
                &first.identity,
                &first.initial_accessories
            ),
            identity_fingerprint(
                IDENTITY_VERSION,
                &seed,
                &second.identity,
                &second.initial_accessories
            )
        );
        let other_seed = [8_u8; 32];
        assert_ne!(
            identity_fingerprint(
                IDENTITY_VERSION,
                &seed,
                &first.identity,
                &first.initial_accessories
            ),
            identity_fingerprint(
                IDENTITY_VERSION,
                &other_seed,
                &identity_preview(IDENTITY_VERSION, &other_seed).identity,
                &identity_preview(IDENTITY_VERSION, &other_seed).initial_accessories
            )
        );
    }

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name(" 小满 ").unwrap(), "小满");
        assert!(validate_name("").is_err());
        assert!(validate_name("猫".repeat(25).as_str()).is_err());
    }

    #[test]
    fn stages_are_derived_from_event_count() {
        assert_eq!(stage_for_events(1), "kitten");
        assert_eq!(stage_for_events(4), "young");
        assert_eq!(stage_for_events(9), "traveler");
        assert_eq!(stage_for_events(21), "old_friend");
    }
}
