use crate::{
    auth::User,
    error::{ApiJson, ApiQuery, AppError, Result},
    runner,
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use chrono::{
    DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

/// One editorial direction with its fetch cadence. Hours and weekdays are in the service
/// timezone; `weekdays` uses ISO numbering (1 = Monday).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Topic {
    pub id: String,
    pub name: String,
    pub name_en: String,
    pub description: String,
    pub brief: String,
    pub enabled: bool,
    pub interval_minutes: i32,
    pub window_start: i32,
    pub window_end: i32,
    pub weekdays: Vec<i16>,
    pub model: String,
    pub reasoning_effort: String,
    pub max_items: i32,
    pub lookback_hours: i32,
    pub daily_run_cap: i32,
    pub sort_weight: i32,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<String>,
    pub consecutive_failures: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
struct Run {
    id: Uuid,
    triggered_by: String,
    status: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    model: String,
    input_tokens: i32,
    cached_tokens: i32,
    output_tokens: i32,
    reasoning_tokens: i32,
    web_search_calls: i32,
    x_search_calls: i32,
    cost_ticks: i64,
    returned_count: i32,
    new_count: i32,
    dup_count: i32,
    dropped_count: i32,
    error: Option<String>,
}

const RUN_COLUMNS: &str = "id, triggered_by, status, started_at, finished_at, model, input_tokens, cached_tokens, output_tokens, reasoning_tokens, web_search_calls, x_search_calls, cost_ticks, returned_count, new_count, dup_count, dropped_count, error";

#[derive(Debug, Serialize, FromRow)]
struct Item {
    id: Uuid,
    topic_id: String,
    day: NaiveDate,
    title: String,
    summary: String,
    title_en: String,
    summary_en: String,
    url: String,
    source: String,
    published_at: Option<DateTime<Utc>>,
    tags: Vec<String>,
    importance: i16,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    seen_count: i32,
    hidden: bool,
    pinned: bool,
}

const ITEM_COLUMNS: &str = "id, topic_id, day, title, summary, title_en, summary_en, url, source, published_at, tags, importance, first_seen_at, last_seen_at, seen_count, hidden, pinned";

/// Every editable column, all optional: a PATCH body, or the settings part of a create.
#[derive(Debug, Default, Deserialize)]
struct TopicFields {
    name: Option<String>,
    name_en: Option<String>,
    description: Option<String>,
    brief: Option<String>,
    enabled: Option<bool>,
    interval_minutes: Option<i32>,
    window_start: Option<i32>,
    window_end: Option<i32>,
    weekdays: Option<Vec<i16>>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    max_items: Option<i32>,
    lookback_hours: Option<i32>,
    daily_run_cap: Option<i32>,
    sort_weight: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CreateTopic {
    id: String,
    #[serde(flatten)]
    fields: TopicFields,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateItem {
    hidden: Option<bool>,
    pinned: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RunsQuery {
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ItemsQuery {
    day: Option<NaiveDate>,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/topics", get(list_topics).post(create_topic))
        .route("/api/topics/{id}", patch(update_topic).delete(delete_topic))
        .route("/api/topics/{id}/run", post(run_topic))
        .route("/api/topics/{id}/runs", get(list_runs))
        .route("/api/topics/{id}/items", get(list_items))
        .route("/api/items/{id}", patch(update_item).delete(delete_item))
}

// ---------------------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------------------

/// When the topic should run next, in UTC. `last` is the run the cadence continues from;
/// `None` (a new or re-enabled topic) asks for the first run as soon as the window allows.
/// Failures back off exponentially from 15 minutes, never beyond the normal interval, and
/// a topic that has hit its daily cap waits for the next day's window.
pub fn next_run(
    topic: &Topic,
    tz: Tz,
    now: DateTime<Utc>,
    last: Option<DateTime<Utc>>,
    today_runs: i64,
) -> DateTime<Utc> {
    let local = |at: DateTime<Utc>| at.with_timezone(&tz).naive_local();
    let interval = Duration::minutes(i64::from(topic.interval_minutes));
    let delay = match topic.consecutive_failures {
        0 => interval,
        failures => {
            interval.min(Duration::minutes(15) * 2_i32.pow((failures.clamp(1, 7) - 1) as u32))
        }
    };
    let now = local(now);
    let mut candidate = last.map_or(now, |at| local(at) + delay).max(now);
    if today_runs >= i64::from(topic.daily_run_cap) {
        candidate = candidate.max(next_day(now.date()).and_time(NaiveTime::MIN));
    }
    let window_start = |day: NaiveDate| {
        day.and_hms_opt(topic.window_start as u32, 0, 0)
            .expect("hour below 24")
    };
    loop {
        let day = candidate.date();
        let weekday = day.weekday().number_from_monday() as i16;
        if !topic.weekdays.contains(&weekday) || candidate.hour() as i32 >= topic.window_end {
            candidate = window_start(next_day(day));
        } else if (candidate.hour() as i32) < topic.window_start {
            candidate = window_start(day);
        } else {
            break;
        }
    }
    to_utc(tz, candidate)
}

fn next_day(day: NaiveDate) -> NaiveDate {
    day.succ_opt().expect("date in range")
}

/// Resolves a wall-clock time in `tz`; a time skipped by a DST gap rolls forward an hour.
pub fn to_utc(tz: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    tz.from_local_datetime(&local)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(local + Duration::hours(1)))
                .earliest()
        })
        .expect("a local time exists within an hour")
        .with_timezone(&Utc)
}

// ---------------------------------------------------------------------------
// Admin API
// ---------------------------------------------------------------------------

async fn status(State(state): State<AppState>, _user: User) -> Result<Json<serde_json::Value>> {
    let running = state.in_flight().iter().cloned().collect::<Vec<_>>();
    let since = to_utc(
        state.config.timezone,
        state.digest.today().and_time(NaiveTime::MIN),
    );
    let (runs, cost_ticks) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*), COALESCE(SUM(cost_ticks), 0)::BIGINT FROM runs WHERE started_at >= $1",
    )
    .bind(since)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(json!({
        "running": running,
        "today": { "runs": runs, "cost_ticks": cost_ticks },
        "timezone": state.config.timezone.name(),
        "now": Utc::now(),
    })))
}

async fn list_topics(State(state): State<AppState>, _user: User) -> Result<Json<Vec<Topic>>> {
    Ok(Json(
        sqlx::query_as("SELECT * FROM topics ORDER BY sort_weight, created_at")
            .fetch_all(&state.pool)
            .await?,
    ))
}

async fn create_topic(
    State(state): State<AppState>,
    _user: User,
    ApiJson(request): ApiJson<CreateTopic>,
) -> Result<(StatusCode, Json<Topic>)> {
    let f = request.fields;
    let name = f
        .name
        .ok_or_else(|| AppError::BadRequest("需要 name".into()))?;
    let brief = f
        .brief
        .ok_or_else(|| AppError::BadRequest("需要 brief".into()))?;
    let topic = sqlx::query_as::<_, Topic>(
        r#"
        INSERT INTO topics (
            id, name, name_en, description, brief, enabled, interval_minutes, window_start,
            window_end, weekdays, model, reasoning_effort, max_items, lookback_hours,
            daily_run_cap, sort_weight
        ) VALUES (
            LOWER(BTRIM($1)), BTRIM($2), BTRIM(COALESCE($3, '')), BTRIM(COALESCE($4, '')), BTRIM($5),
            COALESCE($6, TRUE), COALESCE($7, 60), COALESCE($8, 6), COALESCE($9, 24),
            COALESCE($10, '{1,2,3,4,5}'), COALESCE(NULLIF(BTRIM($11), ''), $17), COALESCE($12, 'low'),
            COALESCE($13, 8), COALESCE($14, 24), COALESCE($15, 24), COALESCE($16, 0)
        )
        RETURNING *
        "#,
    )
    .bind(request.id)
    .bind(name)
    .bind(f.name_en)
    .bind(f.description)
    .bind(brief)
    .bind(f.enabled)
    .bind(f.interval_minutes)
    .bind(f.window_start)
    .bind(f.window_end)
    .bind(f.weekdays)
    .bind(f.model)
    .bind(f.reasoning_effort)
    .bind(f.max_items)
    .bind(f.lookback_hours)
    .bind(f.daily_run_cap)
    .bind(f.sort_weight)
    .bind(&state.config.ai_default_model)
    .fetch_one(&state.pool)
    .await
    .map_err(map_db_error)?;
    let topic = runner::reschedule(&state, topic).await?;
    state.digest.reload_topics(&state).await?;
    Ok((StatusCode::CREATED, Json(topic)))
}

async fn update_topic(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<String>,
    ApiJson(f): ApiJson<TopicFields>,
) -> Result<Json<Topic>> {
    // COALESCE merges the provided fields in the database, so no read-before-write is
    // needed and the schema's CHECK constraints validate exactly what changed.
    let topic = sqlx::query_as::<_, Topic>(
        r#"
        UPDATE topics SET
            name = COALESCE(BTRIM($2), name),
            name_en = COALESCE(BTRIM($3), name_en),
            description = COALESCE(BTRIM($4), description),
            brief = COALESCE(BTRIM($5), brief),
            enabled = COALESCE($6, enabled),
            interval_minutes = COALESCE($7, interval_minutes),
            window_start = COALESCE($8, window_start),
            window_end = COALESCE($9, window_end),
            weekdays = COALESCE($10, weekdays),
            model = COALESCE(NULLIF(BTRIM($11), ''), model),
            reasoning_effort = COALESCE($12, reasoning_effort),
            max_items = COALESCE($13, max_items),
            lookback_hours = COALESCE($14, lookback_hours),
            daily_run_cap = COALESCE($15, daily_run_cap),
            sort_weight = COALESCE($16, sort_weight),
            updated_at = NOW()
        WHERE id = $1
        RETURNING *
        "#,
    )
    .bind(&id)
    .bind(f.name)
    .bind(f.name_en)
    .bind(f.description)
    .bind(f.brief)
    .bind(f.enabled)
    .bind(f.interval_minutes)
    .bind(f.window_start)
    .bind(f.window_end)
    .bind(f.weekdays)
    .bind(f.model)
    .bind(f.reasoning_effort)
    .bind(f.max_items)
    .bind(f.lookback_hours)
    .bind(f.daily_run_cap)
    .bind(f.sort_weight)
    .fetch_optional(&state.pool)
    .await
    .map_err(map_db_error)?
    .ok_or_else(|| AppError::NotFound("主题不存在".into()))?;
    let topic = runner::reschedule(&state, topic).await?;
    state.digest.reload_topics(&state).await?;
    Ok(Json(topic))
}

async fn delete_topic(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<Response> {
    let deleted = sqlx::query("DELETE FROM topics WHERE id = $1")
        .bind(&id)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound("主题不存在".into()));
    }
    state.digest.forget_topic(&id);
    state.digest.reload_topics(&state).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn run_topic(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let topic = sqlx::query_as::<_, Topic>("SELECT * FROM topics WHERE id = $1")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("主题不存在".into()))?;
    let run_id = runner::start(&state, topic, "manual").await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "run_id": run_id }))))
}

async fn list_runs(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<RunsQuery>,
) -> Result<Json<Vec<Run>>> {
    let runs = sqlx::query_as::<_, Run>(&format!(
        "SELECT {RUN_COLUMNS} FROM runs WHERE topic_id = $1 ORDER BY started_at DESC LIMIT $2"
    ))
    .bind(&id)
    .bind(query.limit.unwrap_or(50).clamp(1, 200))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(runs))
}

async fn list_items(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<ItemsQuery>,
) -> Result<Json<Vec<Item>>> {
    let items = sqlx::query_as::<_, Item>(&format!(
        "SELECT {ITEM_COLUMNS} FROM items WHERE topic_id = $1 AND day = $2 \
         ORDER BY pinned DESC, published_at DESC NULLS LAST, first_seen_at DESC"
    ))
    .bind(&id)
    .bind(query.day.unwrap_or_else(|| state.digest.today()))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(items))
}

async fn update_item(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<Uuid>,
    ApiJson(request): ApiJson<UpdateItem>,
) -> Result<Json<Item>> {
    let item = sqlx::query_as::<_, Item>(&format!(
        "UPDATE items SET hidden = COALESCE($2, hidden), pinned = COALESCE($3, pinned) \
         WHERE id = $1 RETURNING {ITEM_COLUMNS}"
    ))
    .bind(id)
    .bind(request.hidden)
    .bind(request.pinned)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("条目不存在".into()))?;
    state
        .digest
        .refresh(&state, &item.topic_id, item.day)
        .await?;
    Ok(Json(item))
}

async fn delete_item(
    State(state): State<AppState>,
    _user: User,
    Path(id): Path<Uuid>,
) -> Result<Response> {
    let (topic_id, day) = sqlx::query_as::<_, (String, NaiveDate)>(
        "DELETE FROM items WHERE id = $1 RETURNING topic_id, day",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("条目不存在".into()))?;
    state.digest.refresh(&state, &topic_id, day).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Translates the schema's own guarantees back into the API's error contract, so the
/// checks live once, in the database.
fn map_db_error(error: sqlx::Error) -> AppError {
    let Some(db_error) = error.as_database_error() else {
        return AppError::Sqlx(error);
    };
    match (db_error.code().as_deref(), db_error.constraint()) {
        (Some("23505"), _) => AppError::Conflict("主题 ID 已存在".into()),
        (Some("23514"), constraint) => AppError::BadRequest(
            match constraint {
                Some("topics_id_valid") => "主题 ID 只能包含小写字母、数字和连字符，最长 32 位",
                Some("topics_name_valid") => "名称不能为空且不超过 60 字",
                Some("topics_name_en_valid") => "英文名称不超过 60 字",
                Some("topics_description_valid") => "说明不超过 200 字",
                Some("topics_brief_valid") => "方向描述不能为空且不超过 2000 字",
                Some("topics_interval_valid") => "间隔必须在 15 到 1440 分钟之间",
                Some("topics_window_valid") => "时间窗必须是 0 到 24 点之间的有效区间",
                Some("topics_weekdays_valid") => "星期必须是 1 到 7 的非空列表",
                Some("topics_model_valid") => "模型名称无效",
                Some("topics_effort_valid") => "reasoning_effort 必须是 low、medium 或 high",
                Some("topics_max_items_valid") => "每次条数必须在 1 到 30 之间",
                Some("topics_lookback_valid") => "回看时长必须在 1 到 72 小时之间",
                Some("topics_cap_valid") => "每日上限必须在 1 到 96 之间",
                _ => "请求内容不符合要求",
            }
            .into(),
        ),
        _ => AppError::Sqlx(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{Topic, next_run};
    use chrono::{DateTime, TimeZone, Utc};
    use chrono_tz::Asia::Shanghai;

    fn topic() -> Topic {
        Topic {
            id: "ai".into(),
            name: "AI".into(),
            name_en: String::new(),
            description: String::new(),
            brief: "AI".into(),
            enabled: true,
            interval_minutes: 60,
            window_start: 6,
            window_end: 24,
            weekdays: vec![1, 2, 3, 4, 5],
            model: "grok-4.6".into(),
            reasoning_effort: "low".into(),
            max_items: 8,
            lookback_hours: 24,
            daily_run_cap: 24,
            sort_weight: 0,
            next_run_at: None,
            last_run_at: None,
            last_status: None,
            consecutive_failures: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    // 2026-09-03 is a Thursday.
    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Shanghai
            .with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid time")
            .with_timezone(&Utc)
    }

    #[test]
    fn continues_the_cadence_inside_the_window() {
        let now = local(2026, 9, 3, 10, 5);
        assert_eq!(
            next_run(&topic(), Shanghai, now, Some(local(2026, 9, 3, 10, 0)), 5),
            local(2026, 9, 3, 11, 0)
        );
        // A first run happens right away.
        assert_eq!(next_run(&topic(), Shanghai, now, None, 0), now);
    }

    #[test]
    fn waits_for_the_window_and_skips_the_weekend() {
        // Late Thursday evening: the next slot is Friday 06:00.
        assert_eq!(
            next_run(
                &topic(),
                Shanghai,
                local(2026, 9, 3, 23, 30),
                Some(local(2026, 9, 3, 23, 30)),
                1
            ),
            local(2026, 9, 4, 6, 0)
        );
        // Friday night rolls to Monday.
        assert_eq!(
            next_run(
                &topic(),
                Shanghai,
                local(2026, 9, 4, 23, 30),
                Some(local(2026, 9, 4, 23, 30)),
                1
            ),
            local(2026, 9, 7, 6, 0)
        );
        // Before the window opens.
        assert_eq!(
            next_run(&topic(), Shanghai, local(2026, 9, 3, 4, 0), None, 0),
            local(2026, 9, 3, 6, 0)
        );
    }

    #[test]
    fn backs_off_after_failures_and_respects_the_daily_cap() {
        let mut failing = topic();
        failing.consecutive_failures = 1;
        let at = local(2026, 9, 3, 10, 0);
        assert_eq!(
            next_run(&failing, Shanghai, at, Some(at), 1),
            local(2026, 9, 3, 10, 15)
        );
        failing.consecutive_failures = 3;
        assert_eq!(
            next_run(&failing, Shanghai, at, Some(at), 1),
            local(2026, 9, 3, 11, 0)
        );

        assert_eq!(
            next_run(&topic(), Shanghai, at, Some(at), 24),
            local(2026, 9, 4, 6, 0)
        );
    }
}
