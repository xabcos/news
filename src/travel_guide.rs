//! M4: a durable world guide for the adopted travel girl.
//!
//! Growth events and memories remain the source of truth. This module derives a
//! bounded personality projection, selects three daily stories, and asks the AI
//! for a structured guide. News facts and the girl's opinion are stored separately;
//! X context is explicitly kept in `culture_context` and cannot become evidence.

use crate::{
    auth::User,
    error::{AppError, Result},
    state::AppState,
};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};
use uuid::Uuid;

const PROMPT_VERSION: &str = "m4-girl-guide-v1";
const MAX_CANDIDATES: i64 = 80;
const MAX_MEMORY_ITEMS: usize = 12;
const GUIDE_LEASE_MINUTES: i64 = 20;
const GUIDE_RETRY_MINUTES: i64 = 5;
const MAX_GUIDE_ATTEMPTS: i32 = 3;
const MAX_TRAIT_DRIFT_PER_BRIEF: f64 = 0.05;

#[derive(Debug, Clone, FromRow, Serialize)]
struct GuideNews {
    id: Uuid,
    topic_id: String,
    title: String,
    summary: String,
    title_en: String,
    summary_en: String,
    url: String,
    source: String,
    tags: Vec<String>,
    importance: i16,
    tone: String,
    location: Option<Value>,
    first_seen_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
struct MemoryRow {
    kind: String,
    memory_key: String,
    label: String,
    times: i32,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
struct SelectedNews {
    slot: String,
    item: GuideNews,
}

#[derive(Debug, Clone, Serialize)]
struct Profile {
    traits: BTreeMap<String, f64>,
    interests: Vec<Value>,
    voice: Value,
    evidence: Vec<Value>,
    source_event_count: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GuideDraft {
    opening: String,
    opening_en: String,
    items: Vec<GuideCard>,
    reflection: String,
    reflection_en: String,
    personality_phrase: String,
    personality_phrase_en: String,
    #[serde(default)]
    scene: SceneDirector,
}

/// A small, durable projection of the girl's current activity.  It is generated
/// together with the daily guide and stored in the existing world-brief JSONB,
/// so opening the home page never needs to call the model.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SceneDirector {
    action: String,
    emotion: String,
    bubble: String,
    prop: String,
}

impl Default for SceneDirector {
    fn default() -> Self {
        Self {
            action: "observe".into(),
            emotion: "好奇".into(),
            bubble: "让我先看看今天的世界。".into(),
            prop: "小笔记本".into(),
        }
    }
}

impl SceneDirector {
    fn bounded(mut self) -> Self {
        self.action = clip(&self.action, 80);
        self.emotion = clip(&self.emotion, 32);
        self.bubble = clip(&self.bubble, 140);
        self.prop = clip(&self.prop, 40);
        self
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GuideCard {
    item_index: usize,
    fact: String,
    fact_en: String,
    opinion: String,
    opinion_en: String,
    guidance: String,
    guidance_en: String,
    playful_line: String,
    playful_line_en: String,
    confidence: i16,
    source_indexes: Vec<usize>,
    #[serde(default)]
    culture_context: String,
}

#[derive(Debug, FromRow)]
struct StoredBrief {
    status: String,
    payload: Value,
    finished_at: Option<DateTime<Utc>>,
    attempts: i32,
}

pub(crate) async fn today(State(state): State<AppState>, user: User) -> Result<Response> {
    let girl_id =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM travel_girls WHERE owner_user_id = $1")
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(|| AppError::Conflict("请先领养一只旅行女孩".into()))?;
    crate::travel::touch_girl(&state.pool, girl_id).await?;
    let day = state.digest.today();

    if let Some(stored) = load_brief(&state.pool, girl_id, day).await?
        && stored.status == "ok"
    {
        let _ = sqlx::query(
            "UPDATE travel_messages SET read_at = COALESCE(read_at, NOW()) WHERE girl_id = $1 AND kind = 'guide' AND dedupe_key = $2",
        )
        .bind(girl_id)
        .bind(format!("guide:{girl_id}:{day}"))
        .execute(&state.pool)
        .await;
        return Ok(Json(stored.payload).into_response());
    }
    let permit = state
        .travel_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests("旅行女孩正在整理其他旅程，请稍后再试".into()))?;
    if !claim_brief(&state.pool, girl_id, day, &state.config.ai_default_model).await? {
        drop(permit);
        let stored = load_brief(&state.pool, girl_id, day)
            .await?
            .ok_or_else(|| AppError::Conflict("旅行女孩正在整理今天的世界，请稍后再试".into()))?;
        if stored.status == "ok" {
            return Ok(Json(stored.payload).into_response());
        }
        if stored.status == "failed" && stored.attempts >= MAX_GUIDE_ATTEMPTS {
            return Err(AppError::ServiceUnavailable(
                "今天的旅行女孩领读暂时不可用，明天会自动恢复".into(),
            ));
        }
        if stored.status == "failed"
            && stored.finished_at.is_some_and(|finished| {
                Utc::now() - finished < chrono::Duration::minutes(GUIDE_RETRY_MINUTES)
            })
        {
            return Err(AppError::ServiceUnavailable(
                "旅行女孩刚才没有整理好这封领读，请过几分钟再试".into(),
            ));
        }
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "status": "generating", "retry_after_seconds": 3 })),
        )
            .into_response());
    }

    let worker_state = state.clone();
    tokio::spawn(async move {
        if let Err(error) = finish_generation(&worker_state, girl_id, day).await {
            tracing::warn!(%error, %girl_id, %day, "travel girl guide generation failed");
        }
        drop(permit);
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "status": "generating", "retry_after_seconds": 3 })),
    )
        .into_response())
}

async fn load_brief(pool: &PgPool, girl_id: Uuid, day: NaiveDate) -> Result<Option<StoredBrief>> {
    Ok(sqlx::query_as::<_, StoredBrief>(
        "SELECT status, payload, finished_at, attempts FROM travel_world_briefs WHERE girl_id = $1 AND day = $2",
    )
    .bind(girl_id)
    .bind(day)
    .fetch_optional(pool)
    .await?)
}

async fn claim_brief(pool: &PgPool, girl_id: Uuid, day: NaiveDate, model: &str) -> Result<bool> {
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO travel_world_briefs (id, girl_id, day, status, model, prompt_version, attempts)
        VALUES ($1, $2, $3, 'running', $4, $5, 1)
        ON CONFLICT (girl_id, day) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(girl_id)
    .bind(day)
    .bind(model)
    .bind(PROMPT_VERSION)
    .fetch_optional(pool)
    .await?;
    if inserted.is_some() {
        return Ok(true);
    }
    sqlx::query(
        "UPDATE travel_world_briefs SET status = 'failed', finished_at = NOW(), error = '领读重试次数已用尽' WHERE girl_id = $1 AND day = $2 AND status = 'running' AND attempts >= $3 AND started_at < NOW() - ($4 * INTERVAL '1 minute')",
    )
    .bind(girl_id)
    .bind(day)
    .bind(MAX_GUIDE_ATTEMPTS)
    .bind(GUIDE_LEASE_MINUTES)
    .execute(pool)
    .await?;
    let reclaimed = sqlx::query_scalar::<_, Uuid>(
        "UPDATE travel_world_briefs SET status = 'running', started_at = NOW(), finished_at = NULL, error = NULL, attempts = attempts + 1, model = $3, prompt_version = $4 WHERE girl_id = $1 AND day = $2 AND attempts < $5 AND ((status = 'failed' AND COALESCE(finished_at, started_at) < NOW() - ($6 * INTERVAL '1 minute')) OR (status = 'running' AND started_at < NOW() - ($7 * INTERVAL '1 minute'))) RETURNING id",
    )
    .bind(girl_id)
    .bind(day)
    .bind(model)
    .bind(PROMPT_VERSION)
    .bind(MAX_GUIDE_ATTEMPTS)
    .bind(GUIDE_RETRY_MINUTES)
    .bind(GUIDE_LEASE_MINUTES)
    .fetch_optional(pool)
    .await?;
    Ok(reclaimed.is_some())
}

async fn finish_generation(state: &AppState, girl_id: Uuid, day: NaiveDate) -> Result<()> {
    match generate(state, girl_id, day).await {
        Ok((payload, items, snapshot_id, x_search_calls)) => {
            let updated = sqlx::query(
                "UPDATE travel_world_briefs SET status = 'ok', item_ids = $2, personality_snapshot_id = $3, payload = $4, x_search_calls = $5, finished_at = NOW(), error = NULL WHERE girl_id = $1 AND day = $6 AND status = 'running'",
            )
            .bind(girl_id)
            .bind(items.iter().map(|item| item.item.id).collect::<Vec<_>>())
            .bind(snapshot_id)
            .bind(&payload)
            .bind(x_search_calls)
            .bind(day)
            .execute(&state.pool)
            .await?;
            if updated.rows_affected() > 0 {
                crate::travel::record_message(
                    &state.pool,
                    girl_id,
                    "guide",
                    &format!("guide:{girl_id}:{day}"),
                    None,
                    payload,
                )
                .await?;
            }
            Ok(())
        }
        Err(error) => {
            let _ = sqlx::query(
                "UPDATE travel_world_briefs SET status = 'failed', finished_at = NOW(), error = $3 WHERE girl_id = $1 AND day = $2 AND status = 'running'",
            )
            .bind(girl_id)
            .bind(day)
            .bind(error.to_string())
            .execute(&state.pool)
            .await;
            Err(error)
        }
    }
}

async fn generate(
    state: &AppState,
    girl_id: Uuid,
    day: NaiveDate,
) -> Result<(Value, Vec<SelectedNews>, Uuid, i32)> {
    let profile = profile(&state.pool, girl_id).await?;
    let candidates = select_candidates(&state.pool, girl_id).await?;
    let selected = choose_news(candidates, &profile);
    if selected.is_empty() {
        return Err(AppError::ServiceUnavailable(
            "今天还没有可供旅行女孩领读的新闻，请稍后再试".into(),
        ));
    }
    let creative_memory = recent_creative_memory(&state.pool, girl_id).await?;
    let (draft, x_sources, x_search_calls) =
        request_guide(state, girl_id, day, &profile, &selected, &creative_memory).await?;
    let cards = normalize_cards(draft.items, &selected);
    let reflection = json!({
        "zh": clip(&draft.reflection, 800),
        "en": clip(&draft.reflection_en, 800),
    });
    let snapshot_payload = json!({
        "traits": profile.traits,
        "interests": profile.interests,
        "voice": profile.voice,
        "reflection": reflection,
        "evidence": profile.evidence,
        "source_event_count": profile.source_event_count,
    });
    let snapshot_id = Uuid::new_v4();
    let revision = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(revision), 0) + 1 FROM travel_personality_snapshots WHERE girl_id = $1",
    )
    .bind(girl_id)
    .fetch_one(&state.pool)
    .await?;
    sqlx::query(
        "INSERT INTO travel_personality_snapshots (id, girl_id, revision, traits, interests, voice, reflection, evidence, source_event_count, model, prompt_version) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(snapshot_id)
    .bind(girl_id)
    .bind(revision)
    .bind(&snapshot_payload["traits"])
    .bind(&snapshot_payload["interests"])
    .bind(&snapshot_payload["voice"])
    .bind(&snapshot_payload["reflection"])
    .bind(&snapshot_payload["evidence"])
    .bind(profile.source_event_count)
    .bind(&state.config.ai_default_model)
    .bind(PROMPT_VERSION)
    .execute(&state.pool)
    .await?;

    let payload = json!({
        "day": day,
        "generated_at": Utc::now(),
        "personality": snapshot_payload,
        "opening": clip(&draft.opening, 500),
        "opening_en": clip(&draft.opening_en, 500),
        "items": cards,
        "reflection": clip(&draft.reflection, 800),
        "reflection_en": clip(&draft.reflection_en, 800),
        "personality_phrase": clip(&draft.personality_phrase, 240),
        "personality_phrase_en": clip(&draft.personality_phrase_en, 240),
        "scene": serde_json::to_value(draft.scene.bounded()).unwrap_or_else(|_| json!({})),
        "culture_sources": x_sources,
    });
    Ok((payload, selected, snapshot_id, x_search_calls))
}

async fn profile(pool: &PgPool, girl_id: Uuid) -> Result<Profile> {
    let memories = sqlx::query_as::<_, MemoryRow>(
        "SELECT kind, memory_key, label, times, last_seen_at FROM travel_memories WHERE girl_id = $1 ORDER BY last_seen_at DESC, kind ASC, memory_key ASC LIMIT 120",
    )
    .bind(girl_id)
    .fetch_all(pool)
    .await?;
    let counts = sqlx::query_as::<_, (String, i64)>(
        "SELECT event_type, COUNT(*)::bigint FROM travel_growth_events WHERE girl_id = $1 AND event_type NOT LIKE 'care_%' GROUP BY event_type",
    )
    .bind(girl_id)
    .fetch_all(pool)
    .await?;
    let source_event_count = counts.iter().map(|(_, count)| *count).sum();
    let mut traits = BTreeMap::from([
        ("curiosity".to_owned(), 0.5),
        ("knowledge".to_owned(), 0.5),
        ("courage".to_owned(), 0.5),
        ("practicality".to_owned(), 0.5),
        ("foresight".to_owned(), 0.5),
        ("gentleness".to_owned(), 0.5),
    ]);
    let mut interest_rows = Vec::new();
    let now = Utc::now();
    for memory in memories {
        let days = now
            .signed_duration_since(memory.last_seen_at)
            .num_days()
            .max(0) as f64;
        let recency = 2_f64.powf(-days / 14.0);
        let weight = (f64::from(memory.times) * recency).min(12.0);
        if matches!(memory.kind.as_str(), "tag" | "topic" | "place") {
            interest_rows.push((weight, memory));
        }
    }
    interest_rows.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.kind.cmp(&b.1.kind))
            .then_with(|| a.1.memory_key.cmp(&b.1.memory_key))
    });
    let interests = interest_rows
        .iter()
        .take(MAX_MEMORY_ITEMS)
        .map(|(score, memory)| {
            json!({
                "kind": memory.kind,
                "key": memory.memory_key,
                "label": memory.label,
                "score": round(*score),
            })
        })
        .collect::<Vec<_>>();
    for (score, memory) in interest_rows.iter().take(MAX_MEMORY_ITEMS) {
        let boost = (*score / 120.0).min(0.12);
        match memory.kind.as_str() {
            "place" => {
                bump(&mut traits, "curiosity", boost * 1.4);
                bump(&mut traits, "courage", boost * 0.6);
            }
            "topic" => {
                bump(&mut traits, "knowledge", boost);
                bump(&mut traits, "foresight", boost * 0.7);
            }
            _ => {
                bump(&mut traits, "curiosity", boost);
                bump(&mut traits, "knowledge", boost * 0.8);
            }
        }
    }
    for (event_type, count) in counts {
        let boost = (count as f64 / 80.0).min(0.08);
        match event_type.as_str() {
            "trip_read" => {
                bump(&mut traits, "curiosity", boost);
                bump(&mut traits, "courage", boost * 0.7);
            }
            "reading_opened" => bump(&mut traits, "knowledge", boost),
            _ => {}
        }
    }
    if let Some(previous) = sqlx::query_scalar::<_, Value>(
        "SELECT traits FROM travel_personality_snapshots WHERE girl_id = $1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(girl_id)
    .fetch_optional(pool)
    .await?
    {
        for (key, value) in &mut traits {
            if let Some(previous) = previous.get(key).and_then(Value::as_f64) {
                let delta = (*value - previous).clamp(-MAX_TRAIT_DRIFT_PER_BRIEF, MAX_TRAIT_DRIFT_PER_BRIEF);
                *value = (previous + delta).clamp(0.0, 1.0);
            }
        }
    }
    for value in traits.values_mut() {
        *value = round(*value);
    }
    let evidence = interests.iter().take(8).cloned().collect();
    Ok(Profile {
        traits,
        interests,
        voice: json!({
            "warmth": 0.90,
            "rationality": 0.90,
            "playfulness": 0.35,
            "culture_context": "x_search 热梗只能作为语境，不得作为事实依据",
        }),
        evidence,
        source_event_count,
    })
}

async fn select_candidates(pool: &PgPool, girl_id: Uuid) -> Result<Vec<GuideNews>> {
    Ok(sqlx::query_as::<_, GuideNews>(
        r#"
        SELECT items.id, items.topic_id, items.title, items.summary, items.title_en,
               items.summary_en, items.url, items.source, items.tags, items.importance,
               items.tone, items.location, items.first_seen_at
        FROM items
        JOIN topics ON topics.id = items.topic_id
        JOIN travel_girls ON travel_girls.id = $1
        WHERE items.hidden = FALSE
          AND items.first_seen_at >= NOW() - INTERVAL '48 hours'
          AND (topics.owner_user_id IS NULL OR topics.owner_user_id = travel_girls.owner_user_id)
        ORDER BY items.importance DESC, items.first_seen_at DESC
        LIMIT $2
        "#,
    )
    .bind(girl_id)
    .bind(MAX_CANDIDATES)
    .fetch_all(pool)
    .await?)
}

fn choose_news(candidates: Vec<GuideNews>, profile: &Profile) -> Vec<SelectedNews> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let interest_keys = profile
        .interests
        .iter()
        .filter_map(|item| item.get("key").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let mut used = HashSet::new();
    let mut selected = Vec::new();
    if let Some(item) = candidates.first() {
        used.insert(item.id);
        selected.push(SelectedNews {
            slot: "important".into(),
            item: item.clone(),
        });
    }
    let interest = candidates
        .iter()
        .filter(|item| !used.contains(&item.id))
        .max_by_key(|item| {
            (interest_score(item, &interest_keys) * 100.0) as i64 + i64::from(item.importance)
        });
    if let Some(item) = interest {
        used.insert(item.id);
        selected.push(SelectedNews {
            slot: "interest".into(),
            item: item.clone(),
        });
    }
    let explore = candidates
        .iter()
        .filter(|item| !used.contains(&item.id))
        .max_by_key(|item| {
            let unfamiliar = if interest_score(item, &interest_keys) == 0.0 {
                30
            } else {
                0
            };
            unfamiliar + i64::from(item.importance) * 5
        });
    if let Some(item) = explore {
        selected.push(SelectedNews {
            slot: "explore".into(),
            item: item.clone(),
        });
    }
    selected
}

fn interest_score(item: &GuideNews, keys: &HashSet<String>) -> f64 {
    let mut score = if keys.contains(&item.topic_id) {
        1.0
    } else {
        0.0
    };
    score += item.tags.iter().filter(|tag| keys.contains(*tag)).count() as f64 * 0.8;
    score
}

/// Returns a compact creative memory for the scene director.  It deliberately
/// reads the existing JSONB projections instead of introducing another table:
/// factual memories remain authoritative while the model gets enough context
/// to avoid repeating the same prop, pose, or surprise.
pub(crate) async fn recent_creative_memory(pool: &PgPool, girl_id: Uuid) -> Result<Value> {
    let scenes = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT payload->'scene' FROM travel_world_briefs WHERE girl_id = $1 AND status = 'ok' AND payload ? 'scene' ORDER BY day DESC LIMIT 6",
    )
    .bind(girl_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let dynamic_scenes = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT payload - 'visual_plan' - 'postcard_url' FROM travel_messages WHERE girl_id = $1 AND payload ? 'scene_key' ORDER BY created_at DESC LIMIT 12",
    )
    .bind(girl_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let trips = sqlx::query_scalar::<_, Option<Value>>(
        "SELECT souvenir->'narrative' FROM travel_trips WHERE girl_id = $1 AND souvenir ? 'narrative' ORDER BY COALESCE(settled_at, returns_at) DESC LIMIT 6",
    )
    .bind(girl_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .flatten()
    .map(|value| {
        json!({
            "mood": value.get("mood"),
            "surprise": value.get("surprise"),
            "souvenir": value.get("souvenir"),
            "scene_hint": value.get("scene_hint"),
        })
    })
    .collect::<Vec<_>>();
    let visual_plans = sqlx::query_scalar::<_, Value>(
        "SELECT creative_plan FROM travel_postcards WHERE split_part(source_key, ':', 2) = $1 AND source_key LIKE 'travel:%' AND creative_plan <> '{}'::jsonb ORDER BY created_at DESC LIMIT 20",
    ).bind(girl_id.to_string()).fetch_all(pool).await?;
    Ok(
        json!({ "daily_scenes": scenes, "dynamic_scenes": dynamic_scenes, "travel_moments": trips, "visual_plans": visual_plans }),
    )
}

fn creative_seed(girl_id: Uuid, day: NaiveDate) -> String {
    let mut hasher = Sha256::new();
    hasher.update(girl_id.as_bytes());
    hasher.update(day.to_string().as_bytes());
    // A six-hour bucket gives retries a little room to surprise without
    // making every request produce a different scene.
    hasher.update((Utc::now().hour() / 6).to_string().as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn request_guide(
    state: &AppState,
    girl_id: Uuid,
    day: NaiveDate,
    profile: &Profile,
    selected: &[SelectedNews],
    creative_memory: &Value,
) -> Result<(GuideDraft, Vec<String>, i32)> {
    let model = &state.config.ai_default_model;
    let tools = if model.starts_with("grok") {
        json!([{ "type": "web_search" }, { "type": "x_search" }])
    } else {
        json!([{ "type": "web_search" }])
    };
    let stories = selected
        .iter()
        .enumerate()
        .map(|(index, selected)| {
            json!({
                "index": index,
                "slot": selected.slot,
                "title": selected.item.title,
                "summary": selected.item.summary,
                "source": selected.item.source,
                "url": selected.item.url,
                "tags": selected.item.tags,
                "importance": selected.item.importance,
                "tone": selected.item.tone,
                "location": selected.item.location,
            })
        })
        .collect::<Vec<_>>();
    let body = json!({
        "model": model,
        "reasoning": { "effort": if model.starts_with("grok") { "high" } else { "medium" } },
        "store": false,
        "tools": tools,
        "tool_choice": "auto",
        "instructions": instructions(),
        "input": format!(
            "日期：{}\nCREATIVE_SEED：{}\n性格投影：{}\n最近场景与纪念物（只用于避免重复，不是事实）：{}\n候选新闻：{}",
            day,
            creative_seed(girl_id, day),
            serde_json::to_string(profile).unwrap_or_default(),
            serde_json::to_string(creative_memory).unwrap_or_default(),
            serde_json::to_string(&stories).unwrap_or_default(),
        ),
        "text": { "format": {
            "type": "json_schema", "name": "travel_world_guide", "strict": true, "schema": schema(),
        } },
    });
    let reply = responses_request(state, &body).await?;
    let draft = parse_draft(&reply)?;
    let x_sources = citations(&reply)
        .into_iter()
        .filter(|url| is_x_url(url))
        .take(8)
        .collect::<Vec<_>>();
    let x_calls = reply["usage"]["server_side_tool_usage_details"]["x_search_calls"]
        .as_i64()
        .unwrap_or(0)
        .clamp(0, i64::from(i32::MAX)) as i32;
    Ok((draft, x_sources, x_calls))
}

pub(crate) async fn responses_request(state: &AppState, body: &Value) -> Result<Value> {
    let url = format!(
        "{}/v1/responses",
        state.config.ai_base_url.as_str().trim_end_matches('/')
    );
    let mut last_error = None;
    for attempt in 0..3 {
        let response = tokio::time::timeout(
            state.config.ai_response_timeout,
            state
                .http
                .post(&url)
                .bearer_auth(&state.config.ai_api_key)
                .json(body)
                .send(),
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("旅行女孩领读生成超时，请稍后再试".into()))?
        .map_err(|error| {
            AppError::ServiceUnavailable(format!("旅行女孩领读请求失败: {error}").into())
        })?;
        if response.status().is_success() {
            return Ok(response.json().await?);
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        last_error = Some(format!("{status}: {}", clip(&text, 300)));
        if !(status.as_u16() == 429 || status.is_server_error()) || attempt == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2_u64.pow(attempt))).await;
    }
    Err(AppError::ServiceUnavailable(
        format!(
            "旅行女孩领读暂时不可用: {}",
            last_error.unwrap_or_else(|| "unknown".into())
        )
        .into(),
    ))
}

fn parse_draft(reply: &Value) -> Result<GuideDraft> {
    if let Some(error) = reply.get("error").filter(|value| !value.is_null()) {
        return Err(AppError::Internal(
            format!("AI 接口错误: {}", clip(&error.to_string(), 300)).into(),
        ));
    }
    let text = reply["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|output| output["type"] == "message")
        .filter_map(|output| output["content"].as_array())
        .flatten()
        .find(|content| content["type"] == "output_text")
        .and_then(|content| content["text"].as_str())
        .ok_or_else(|| AppError::Internal("旅行女孩领读没有文本输出".into()))?;
    let text = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if start < end => &text[start..=end],
        _ => text,
    };
    serde_json::from_str(text).map_err(|_| AppError::Internal("旅行女孩领读输出结构无效".into()))
}

fn normalize_cards(mut cards: Vec<GuideCard>, selected: &[SelectedNews]) -> Vec<Value> {
    let mut by_index = HashMap::new();
    for card in cards.drain(..) {
        if card.item_index < selected.len() {
            by_index.entry(card.item_index).or_insert(card);
        }
    }
    let selected_len = selected.len();
    selected
        .iter()
        .enumerate()
        .map(|(index, selected)| {
            let mut card = by_index.remove(&index).unwrap_or_else(|| GuideCard {
                item_index: index,
                fact: selected.item.summary.clone(),
                fact_en: selected.item.summary_en.clone(),
                opinion: "我先把这件事放进我们的世界地图，等更多事实再下判断。".into(),
                opinion_en: "I’m putting this on our world map first; I’ll judge it more carefully as facts arrive.".into(),
                guidance: "可以先读完来源，再看看它和你关心的主题有什么联系。".into(),
                guidance_en: "Read the source first, then see how it connects with the themes you care about.".into(),
                playful_line: "先别急着给世界下结论，尾巴还在整理证据呢。".into(),
                playful_line_en: "Let’s not judge the whole world yet; my tail is still sorting the evidence.".into(),
                confidence: 3,
                source_indexes: vec![index],
                culture_context: String::new(),
            });
            card.item_index = index;
            card.confidence = card.confidence.clamp(1, 5);
            card.source_indexes.retain(|source| *source < selected_len);
            if card.source_indexes.is_empty() {
                card.source_indexes.push(index);
            }
            json!({
                "slot": selected.slot,
                "news_item_id": selected.item.id,
                "title": selected.item.title,
                "summary": selected.item.summary,
                "url": selected.item.url,
                "source": selected.item.source,
                "tone": selected.item.tone,
                "location": selected.item.location,
                "fact": clip(&card.fact, 700),
                "fact_en": clip(&card.fact_en, 700),
                "opinion": clip(&card.opinion, 700),
                "opinion_en": clip(&card.opinion_en, 700),
                "guidance": clip(&card.guidance, 700),
                "guidance_en": clip(&card.guidance_en, 700),
                "playful_line": clip(&card.playful_line, 300),
                "playful_line_en": clip(&card.playful_line_en, 300),
                "confidence": card.confidence,
                "source_indexes": card.source_indexes,
                "culture_context": clip(&card.culture_context, 300),
            })
        })
        .collect()
}

fn instructions() -> &'static str {
    r#"你是一个旅行女孩，带主人温柔地认识当天的世界。你的语气必须同时具备：温柔陪伴、理性分析、轻微俏皮。不要装成熟人类专家，也不要用夸张卖萌。

输出规则：
1. 每条新闻分成 fact、opinion、guidance、playful_line。fact 只能根据输入新闻写，source_indexes 必须引用输入新闻；opinion 是旅行女孩基于当前性格投影的判断；guidance 是邀请主人继续了解、比较或观察的建议，不是医疗、法律、投资或人生保证；playful_line 只能轻轻点缀。
2. 不要把旅行女孩观点写成新闻事实。不要创造数字、人物、因果或来源。confidence 表示证据强度。
3. 新闻候选固定分为 important、interest、explore 三条，不能把三条都写成同一种主题。explore 必须帮助主人走出兴趣回音壁。
4. 如果可用，可以调用一次 x_search 了解当天公开 X.com 语境中的热梗或表达方式。X 内容只能写入 culture_context，用作比喻、语境或轻微俏皮表达；绝不能作为 fact、source_indexes 或 guidance 的事实依据。不要引用私人账号、攻击性内容、歧视性内容或未经核实的传闻。没有合适热梗就留空。
5. 性格投影是长期倾向，不是本次新闻的临时情绪。性格语气固定为 warmth 0.90、rationality 0.90、playfulness 0.35。要让主人感觉旅行女孩在逐渐形成自己的判断，但不要突然改变性格。
6. 另外生成 scene，描述旅行女孩此刻正在做什么。action 必须从 observe、packing、reading、wandering、stretching、sleeping、daydreaming 中选择一个，emotion 是短情绪词，bubble 是旅行女孩可能说的一句短话，prop 是一个可见的小道具。scene 必须和当天内容有关但不能编造新闻事实；每次可以换一个自然的小动作，避免固定套话。输入中的最近场景与纪念物是“去重提示”，不要机械重复其中的姿势、道具、情绪或句式；CREATIVE_SEED 只用于稳定地产生当天的微小差异。
7. 中文自然，英文独立润色。严格只输出 JSON，不输出 Markdown 或解释。
"#
}

fn schema() -> Value {
    let card = json!({
        "type": "object", "additionalProperties": false,
        "required": ["item_index", "fact", "fact_en", "opinion", "opinion_en", "guidance", "guidance_en", "playful_line", "playful_line_en", "confidence", "source_indexes", "culture_context"],
        "properties": {
            "item_index": { "type": "integer", "minimum": 0, "maximum": 2 },
            "fact": { "type": "string" }, "fact_en": { "type": "string" },
            "opinion": { "type": "string" }, "opinion_en": { "type": "string" },
            "guidance": { "type": "string" }, "guidance_en": { "type": "string" },
            "playful_line": { "type": "string" }, "playful_line_en": { "type": "string" },
            "confidence": { "type": "integer", "minimum": 1, "maximum": 5 },
            "source_indexes": { "type": "array", "maxItems": 3, "items": { "type": "integer", "minimum": 0, "maximum": 2 } },
            "culture_context": { "type": "string" }
        }
    });
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["opening", "opening_en", "items", "reflection", "reflection_en", "personality_phrase", "personality_phrase_en", "scene"],
        "properties": {
            "opening": { "type": "string" }, "opening_en": { "type": "string" },
            "items": { "type": "array", "minItems": 1, "maxItems": 3, "items": card },
            "reflection": { "type": "string" }, "reflection_en": { "type": "string" },
            "personality_phrase": { "type": "string" }, "personality_phrase_en": { "type": "string" },
            "scene": {
                "type": "object", "additionalProperties": false,
                "required": ["action", "emotion", "bubble", "prop"],
                "properties": {
                    "action": { "type": "string", "enum": ["observe", "packing", "reading", "wandering", "stretching", "sleeping", "daydreaming"] },
                    "emotion": { "type": "string", "maxLength": 32 },
                    "bubble": { "type": "string", "maxLength": 140 },
                    "prop": { "type": "string", "maxLength": 40 }
                }
            }
        }
    })
}

fn citations(reply: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    reply["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|output| output["content"].as_array())
        .flatten()
        .filter_map(|content| content["annotations"].as_array())
        .flatten()
        .filter_map(|annotation| annotation["url"].as_str())
        .filter_map(|url| {
            let url = url.trim();
            (url.starts_with("https://") && url.len() <= 512 && seen.insert(url.to_owned()))
                .then(|| url.to_owned())
        })
        .collect()
}

fn is_x_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed.host_str().map(|host| {
                host.eq_ignore_ascii_case("x.com")
                    || host.ends_with(".x.com")
                    || host.eq_ignore_ascii_case("twitter.com")
                    || host.ends_with(".twitter.com")
            })
        })
        .unwrap_or(false)
}

fn bump(traits: &mut BTreeMap<String, f64>, key: &str, amount: f64) {
    if let Some(value) = traits.get_mut(key) {
        *value = (*value + amount).clamp(0.0, 1.0);
    }
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn clip(value: &str, max: usize) -> String {
    value.trim().chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::{is_x_url, round};

    #[test]
    fn accepts_only_x_hosts_for_culture_sources() {
        assert!(is_x_url("https://x.com/example/status/1"));
        assert!(is_x_url("https://twitter.com/example/status/1"));
        assert!(!is_x_url("https://example.com/x.com"));
    }

    #[test]
    fn rounds_profile_scores() {
        assert_eq!(round(0.456), 0.46);
    }
}
