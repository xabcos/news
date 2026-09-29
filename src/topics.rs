use crate::{
    auth::{self, PlanLimits, User},
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

/// The local date boundary is shared by the user dashboard and scheduler.
pub(crate) fn day_start(state: &AppState) -> DateTime<Utc> {
    to_utc(
        state.config.timezone,
        state.digest.today().and_time(NaiveTime::MIN),
    )
}

/// One editorial direction with its fetch cadence. Hours and weekdays are in the service
/// timezone; `weekdays` uses ISO numbering (1 = Monday).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Topic {
    pub id: String,
    /// NULL marks one of the administrator-managed public topics.  Non-NULL
    /// topics belong to exactly one personal account.
    #[serde(skip_serializing)]
    pub owner_user_id: Option<i64>,
    /// Set only on personal API responses when the current plan no longer
    /// covers this retained topic. It is not persisted in the database.
    #[serde(skip_serializing_if = "is_false")]
    #[sqlx(default)]
    pub plan_locked: bool,
    pub name: String,
    pub name_en: String,
    pub description: String,
    pub brief: String,
    pub enabled: bool,
    pub visible: bool,
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

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy)]
enum TopicScope {
    Personal,
    Public,
}

impl TopicScope {
    fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }
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
    tags_en: Vec<String>,
    importance: i16,
    first_seen_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    seen_count: i32,
    hidden: bool,
    pinned: bool,
}

/// Every editable column, all optional: a PATCH body, or the settings part of a create.
#[derive(Debug, Default, Deserialize)]
struct TopicFields {
    name: Option<String>,
    name_en: Option<String>,
    description: Option<String>,
    brief: Option<String>,
    enabled: Option<bool>,
    visible: Option<bool>,
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
    #[serde(default)]
    id: Option<String>,
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

/// Returns the limits currently effective for a topic owner. Public topics are
/// administrator-managed and retain the original service-wide daily guard.
pub(crate) async fn limits_for_topic(state: &AppState, topic: &Topic) -> Result<PlanLimits> {
    let Some(owner_user_id) = topic.owner_user_id else {
        return Ok(PlanLimits {
            max_topics: i64::MAX,
            min_interval_minutes: 15,
            daily_run_cap: topic.daily_run_cap.max(1),
        });
    };
    let pro_until =
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>("SELECT pro_until FROM users WHERE id = $1")
            .bind(owner_user_id)
            .fetch_optional(&state.pool)
            .await?
            .flatten();
    Ok(if pro_until.is_some_and(|until| until > Utc::now()) {
        auth::PRO_LIMITS
    } else {
        auth::FREE_LIMITS
    })
}

/// A downgraded account keeps its old topics, but only the oldest topics that
/// fit the current plan remain runnable. Deleting one naturally promotes the
/// next topic without another subscription-state column.
pub(crate) async fn personal_topic_allowed(
    state: &AppState,
    topic: &Topic,
    max_topics: i64,
) -> Result<bool> {
    let Some(owner_user_id) = topic.owner_user_id else {
        return Ok(true);
    };
    let position = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM topics
         WHERE owner_user_id = $1
           AND (created_at < $2 OR (created_at = $2 AND id <= $3))",
    )
    .bind(owner_user_id)
    .bind(topic.created_at)
    .bind(&topic.id)
    .fetch_one(&state.pool)
    .await?;
    Ok(position <= max_topics)
}

/// Wakes the topics that fit a newly active plan. Topics that were retained
/// while a user was on Free have no due slot; setting only NULL slots avoids
/// disturbing an already scheduled cadence.
pub(crate) async fn wake_personal_topics(
    state: &AppState,
    user_id: i64,
    max_topics: i64,
) -> Result<()> {
    sqlx::query(
        "WITH allowed AS (
             SELECT id FROM topics
             WHERE owner_user_id = $1
             ORDER BY created_at, id
             LIMIT $2
         )
         UPDATE topics
         SET next_run_at = COALESCE(next_run_at, NOW())
         WHERE enabled AND id IN (SELECT id FROM allowed)",
    )
    .bind(user_id)
    .bind(max_topics)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn load_topic(state: &AppState, id: &str, user: &User, scope: TopicScope) -> Result<Topic> {
    let topic = if scope.is_public() {
        sqlx::query_as::<_, Topic>("SELECT * FROM topics WHERE id = $1 AND owner_user_id IS NULL")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
    } else {
        sqlx::query_as::<_, Topic>("SELECT * FROM topics WHERE id = $1 AND owner_user_id = $2")
            .bind(id)
            .bind(user.id)
            .fetch_optional(&state.pool)
            .await?
    };
    topic.ok_or_else(|| AppError::NotFound("主题不存在".into()))
}

fn generated_topic_id() -> String {
    // UUID's compact hexadecimal form fits the existing 32-character slug
    // constraint and prevents a personal topic from colliding with a public id.
    Uuid::new_v4().simple().to_string()
}

fn validate_interval(interval: Option<i32>, limits: PlanLimits) -> Result<()> {
    if interval.is_some_and(|minutes| minutes < limits.min_interval_minutes) {
        return Err(AppError::BadRequest(
            format!(
                "当前套餐的最小抓取间隔是 {} 分钟",
                limits.min_interval_minutes
            )
            .into(),
        ));
    }
    Ok(())
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/me/topics", get(me_list_topics).post(me_create_topic))
        .route(
            "/api/me/topics/{id}",
            patch(me_update_topic).delete(me_delete_topic),
        )
        .route("/api/me/topics/{id}/run", post(me_run_topic))
        .route("/api/me/topics/{id}/runs", get(me_list_runs))
        .route("/api/me/topics/{id}/items", get(me_list_items))
        .route(
            "/api/me/items/{id}",
            patch(me_update_item).delete(me_delete_item),
        )
        .route("/api/me/status", get(me_status))
        .route(
            "/api/admin/topics",
            get(admin_list_topics).post(admin_create_topic),
        )
        .route(
            "/api/admin/topics/{id}",
            patch(admin_update_topic).delete(admin_delete_topic),
        )
        .route("/api/admin/topics/{id}/run", post(admin_run_topic))
        .route("/api/admin/topics/{id}/runs", get(admin_list_runs))
        .route("/api/admin/topics/{id}/items", get(admin_list_items))
        .route(
            "/api/admin/items/{id}",
            patch(admin_update_item).delete(admin_delete_item),
        )
        .route("/api/admin/status", get(admin_status))
}

// ---------------------------------------------------------------------------
// Schedule
// ---------------------------------------------------------------------------

/// When the topic should run next, in UTC. `last` is the run the cadence continues from;
/// `None` (a new or re-enabled topic) asks for the first run as soon as the window allows.
/// Failures back off exponentially from 15 minutes, never beyond the normal interval, and
/// a topic that has hit its daily cap waits for the next day's window. When
/// `window_end` is earlier than `window_start`, the window crosses midnight and
/// weekdays are attached to the day on which the window starts.
pub fn next_run(
    topic: &Topic,
    tz: Tz,
    now: DateTime<Utc>,
    last: Option<DateTime<Utc>>,
    today_runs: i64,
) -> DateTime<Utc> {
    next_run_with_min_interval(topic, tz, now, last, today_runs, 15)
}

/// Variant used by personal topics after the plan floor is applied.  Keeping
/// the floor here prevents a failure backoff from scheduling a free topic at
/// 15 minutes and then being rejected by the same admission policy.
pub fn next_run_with_min_interval(
    topic: &Topic,
    tz: Tz,
    now: DateTime<Utc>,
    last: Option<DateTime<Utc>>,
    today_runs: i64,
    min_interval_minutes: i32,
) -> DateTime<Utc> {
    let local = |at: DateTime<Utc>| at.with_timezone(&tz).naive_local();
    let delay = Duration::minutes(effective_run_delay_minutes(
        topic.interval_minutes,
        topic.consecutive_failures,
        min_interval_minutes,
    ));
    let now = local(now);
    let mut candidate = last.map_or(now, |at| local(at) + delay).max(now);
    let overnight = topic.window_start > topic.window_end;
    if today_runs >= i64::from(topic.daily_run_cap) {
        let anchor_day = if overnight && (now.hour() as i32) < topic.window_end {
            now.date().pred_opt().expect("date in range")
        } else {
            now.date()
        };
        candidate = candidate.max(next_window_start(topic, next_day(anchor_day)));
    }
    loop {
        let day = candidate.date();
        let hour = candidate.hour() as i32;
        if overnight {
            // After midnight, the active window belongs to the previous day's
            // start date. The daytime gap rolls forward to the next start.
            let anchor_day = if hour < topic.window_end {
                day.pred_opt().expect("date in range")
            } else {
                day
            };
            let in_window = hour >= topic.window_start || hour < topic.window_end;
            if in_window
                && topic
                    .weekdays
                    .contains(&(anchor_day.weekday().number_from_monday() as i16))
            {
                break;
            }
            candidate = next_window_start(topic, day);
        } else {
            let weekday = day.weekday().number_from_monday() as i16;
            if !topic.weekdays.contains(&weekday) || hour >= topic.window_end {
                candidate = next_window_start(topic, next_day(day));
            } else if hour < topic.window_start {
                candidate = next_window_start(topic, day);
            } else {
                break;
            }
        }
    }
    to_utc(tz, candidate)
}

/// Returns the delay admission should enforce after the last run. Failed runs
/// retry with exponential backoff, while successful runs keep the configured
/// cadence. The plan floor applies to both paths.
pub(crate) fn effective_run_delay_minutes(
    interval_minutes: i32,
    consecutive_failures: i32,
    min_interval_minutes: i32,
) -> i64 {
    let interval = i64::from(interval_minutes.max(min_interval_minutes));
    if consecutive_failures <= 0 {
        return interval;
    }
    let backoff = 15_i64 * 2_i64.pow((consecutive_failures.clamp(1, 7) - 1) as u32);
    interval.min(backoff).max(i64::from(min_interval_minutes))
}

fn next_day(day: NaiveDate) -> NaiveDate {
    day.succ_opt().expect("date in range")
}

fn next_window_start(topic: &Topic, from: NaiveDate) -> NaiveDateTime {
    let mut day = from;
    loop {
        let weekday = day.weekday().number_from_monday() as i16;
        if topic.weekdays.contains(&weekday) {
            return day
                .and_hms_opt(topic.window_start as u32, 0, 0)
                .expect("hour below 24");
        }
        day = next_day(day);
    }
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

async fn status(
    State(state): State<AppState>,
    user: User,
    scope: TopicScope,
) -> Result<Json<serde_json::Value>> {
    let mut running = state.in_flight().iter().cloned().collect::<Vec<_>>();
    let since = day_start(&state);
    let (runs, cost_ticks) = if scope.is_public() {
        let public =
            sqlx::query_scalar::<_, String>("SELECT id FROM topics WHERE owner_user_id IS NULL")
                .fetch_all(&state.pool)
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
        running.retain(|id| public.contains(id));
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT COUNT(runs.*), COALESCE(SUM(runs.cost_ticks), 0)::BIGINT FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id IS NULL AND runs.started_at >= $1",
        )
        .bind(since)
        .fetch_one(&state.pool)
        .await?
    } else {
        let owned =
            sqlx::query_scalar::<_, String>("SELECT id FROM topics WHERE owner_user_id = $1")
                .bind(user.id)
                .fetch_all(&state.pool)
                .await?
                .into_iter()
                .collect::<std::collections::HashSet<_>>();
        running.retain(|id| owned.contains(id));
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT COUNT(*), COALESCE(SUM(runs.cost_ticks), 0)::BIGINT FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id = $1 AND runs.started_at >= $2",
        )
        .bind(user.id)
        .bind(since)
        .fetch_one(&state.pool)
        .await?
    };
    let mut response = json!({
        "running": running,
        "today": { "runs": runs, "cost_ticks": cost_ticks },
        "timezone": state.config.timezone.name(),
        "now": Utc::now(),
    });
    if scope.is_public() {
        // The total is an O(1) counter maintained by a database trigger.  The weekly
        // range uses the partial last-login index and is refreshed with the existing
        // admin status poll, so the browser does not need another request.
        let (registered, active_7d) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT registered_count, (SELECT COUNT(*) FROM users WHERE status AND last_login_at >= NOW() - INTERVAL '7 days') FROM user_metrics WHERE id = TRUE",
        )
        .fetch_one(&state.pool)
        .await?;
        response["users"] = json!({
            "active_7d": active_7d,
            "registered": registered,
        });
    }
    Ok(Json(response))
}

async fn list_topics(
    State(state): State<AppState>,
    user: User,
    scope: TopicScope,
) -> Result<Json<Vec<Topic>>> {
    let mut topics = if scope.is_public() {
        sqlx::query_as::<_, Topic>(
            "SELECT * FROM topics WHERE owner_user_id IS NULL ORDER BY sort_weight, created_at",
        )
        .fetch_all(&state.pool)
        .await?
    } else {
        sqlx::query_as::<_, Topic>(
            "SELECT * FROM topics WHERE owner_user_id = $1 ORDER BY created_at, id",
        )
        .bind(user.id)
        .fetch_all(&state.pool)
        .await?
    };
    if !scope.is_public() {
        let max_topics = user.limits().max_topics as usize;
        for (index, topic) in topics.iter_mut().enumerate() {
            topic.plan_locked = index >= max_topics;
        }
    }
    Ok(Json(topics))
}

async fn create_topic(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<CreateTopic>,
    scope: TopicScope,
) -> Result<(StatusCode, Json<Topic>)> {
    if !scope.is_public() && !user.can_create_personal_topic() {
        return Err(AppError::Forbidden(
            "创建抓取任务需要 Pro 套餐，请先购买".into(),
        ));
    }
    let f = request.fields;
    let name = f
        .name
        .ok_or_else(|| AppError::BadRequest("需要 name".into()))?;
    let brief = f
        .brief
        .ok_or_else(|| AppError::BadRequest("需要 brief".into()))?;
    let limits = user.limits();
    let interval = f.interval_minutes.unwrap_or(if scope.is_public() {
        60
    } else {
        limits.min_interval_minutes
    });
    if !scope.is_public() {
        validate_interval(Some(interval), limits)?;
    }
    let id = if scope.is_public() {
        request.id.unwrap_or_else(generated_topic_id)
    } else {
        generated_topic_id()
    };
    if matches!(
        id.trim().to_ascii_lowercase().as_str(),
        "insight" | "equity-insight"
    ) {
        return Err(AppError::BadRequest("该 ID 已用于研判入口".into()));
    }
    let owner_user_id = (!scope.is_public()).then_some(user.id);
    let model = if scope.is_public() {
        f.model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    } else {
        None
    };
    let reasoning_effort = if scope.is_public() {
        f.reasoning_effort
    } else {
        None
    };
    let max_items = if scope.is_public() { f.max_items } else { None };
    let lookback_hours = if scope.is_public() {
        f.lookback_hours
    } else {
        None
    };
    let daily_run_cap = if scope.is_public() {
        f.daily_run_cap
    } else {
        Some(limits.daily_run_cap)
    };
    let sort_weight = if scope.is_public() {
        f.sort_weight
    } else {
        Some(0)
    };
    let (window_start, window_end, weekdays) = if scope.is_public() {
        (f.window_start, f.window_end, f.weekdays)
    } else {
        // Personal scheduling stays deliberately small: run every day in the
        // normal daytime window; calendar controls remain administrator-only.
        (Some(6), Some(24), Some(vec![1, 2, 3, 4, 5, 6, 7]))
    };
    let mut transaction = state.pool.begin().await?;
    if !scope.is_public() {
        // Serialising the count and insert keeps two tabs from creating a
        // second topic at the same time without requiring a trigger.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(user.id)
            .execute(&mut *transaction)
            .await?;
        let count =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topics WHERE owner_user_id = $1")
                .bind(user.id)
                .fetch_one(&mut *transaction)
                .await?;
        if count >= limits.max_topics {
            return Err(AppError::TooManyRequests(
                format!("当前套餐最多创建 {} 个主题", limits.max_topics).into(),
            ));
        }
    }
    let topic = sqlx::query_as::<_, Topic>(
        r#"
        INSERT INTO topics (
            id, owner_user_id, name, name_en, description, brief, enabled, interval_minutes, window_start,
            window_end, weekdays, model, reasoning_effort, max_items, lookback_hours,
            daily_run_cap, sort_weight, visible
        ) VALUES (
            LOWER(BTRIM($1)), $2, BTRIM($3), BTRIM(COALESCE($4, '')), BTRIM(COALESCE($5, '')), BTRIM($6),
            COALESCE($7, TRUE), $8, COALESCE($9, 6), COALESCE($10, 24),
            COALESCE($11, '{1,2,3,4,5}'), COALESCE(NULLIF(BTRIM($12), ''), $18), COALESCE($13, 'low'),
            COALESCE($14, 8), COALESCE($15, 24), COALESCE($16, 24), COALESCE($17, 0), COALESCE($19, TRUE)
        )
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(owner_user_id)
    .bind(name)
    .bind(f.name_en)
    .bind(f.description)
    .bind(brief)
    .bind(f.enabled)
    .bind(interval)
    .bind(window_start)
    .bind(window_end)
    .bind(weekdays)
    .bind(model)
    .bind(reasoning_effort)
    .bind(max_items)
    .bind(lookback_hours)
    .bind(daily_run_cap)
    .bind(sort_weight)
    .bind(&state.config.ai_default_model)
    .bind(f.visible)
    .fetch_one(&mut *transaction)
    .await
    .map_err(map_db_error)?;
    transaction.commit().await?;
    let topic = runner::reschedule_or_defer(&state, &topic).await?;
    if scope.is_public() {
        state.digest.reload_topics(&state).await?;
    }
    Ok((StatusCode::CREATED, Json(topic)))
}

async fn update_topic(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(f): ApiJson<TopicFields>,
    scope: TopicScope,
) -> Result<Json<Topic>> {
    // Resolve ownership before applying the patch; an inaccessible id is
    // intentionally indistinguishable from a missing one.
    let current = load_topic(&state, &id, &user, scope).await?;
    let limits = if scope.is_public() {
        PlanLimits {
            max_topics: i64::MAX,
            min_interval_minutes: 15,
            daily_run_cap: current.daily_run_cap,
        }
    } else {
        user.limits()
    };
    validate_interval(f.interval_minutes, limits)?;
    let (model, reasoning_effort, max_items, lookback_hours, daily_run_cap, sort_weight) =
        if scope.is_public() {
            (
                f.model,
                f.reasoning_effort,
                f.max_items,
                f.lookback_hours,
                f.daily_run_cap,
                f.sort_weight,
            )
        } else {
            // Personal accounts control the editorial direction and cadence,
            // while model/cost knobs stay on the service defaults.
            (None, None, None, None, Some(limits.daily_run_cap), None)
        };
    let (window_start, window_end, weekdays) = if scope.is_public() {
        (f.window_start, f.window_end, f.weekdays)
    } else {
        // Keep personal topics on the simple every-day daytime schedule.
        (Some(6), Some(24), Some(vec![1, 2, 3, 4, 5, 6, 7]))
    };
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
            visible = COALESCE($19, visible),
            updated_at = NOW()
        WHERE id = $1 AND (($17 AND owner_user_id IS NULL) OR (NOT $17 AND owner_user_id = $18))
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
    .bind(window_start)
    .bind(window_end)
    .bind(weekdays)
    .bind(model)
    .bind(reasoning_effort)
    .bind(max_items)
    .bind(lookback_hours)
    .bind(daily_run_cap)
    .bind(sort_weight)
    .bind(scope.is_public())
    .bind(user.id)
    .bind(f.visible)
    .fetch_optional(&state.pool)
    .await
    .map_err(map_db_error)?
    .ok_or_else(|| AppError::NotFound("主题不存在".into()))?;
    let topic = runner::reschedule_or_defer(&state, &topic).await?;
    if scope.is_public() {
        if current.enabled != topic.enabled || current.visible != topic.visible {
            state.digest.forget_topic(&state, &id).await;
            state.insight.invalidate();
        }
        state.digest.reload_topics(&state).await?;
    }
    Ok(Json(topic))
}

async fn delete_topic(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    scope: TopicScope,
) -> Result<Response> {
    let mut transaction = state.pool.begin().await?;
    if !scope.is_public() {
        // Keep deletion and the quota-promotion wake-up ordered with create and
        // run admission for this account.
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(user.id)
            .execute(&mut *transaction)
            .await?;
    }
    let deleted = sqlx::query("DELETE FROM topics WHERE id = $1 AND (($2 AND owner_user_id IS NULL) OR (NOT $2 AND owner_user_id = $3))")
        .bind(&id)
        .bind(scope.is_public())
        .bind(user.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    if deleted == 0 {
        return Err(AppError::NotFound("主题不存在".into()));
    }
    transaction.commit().await?;
    if scope.is_public() {
        state.digest.forget_topic(&state, &id).await;
        state.digest.reload_topics(&state).await?;
    } else {
        if let Err(error) = wake_personal_topics(&state, user.id, user.limits().max_topics).await {
            tracing::warn!(%error, user_id = user.id, "failed to wake personal topics");
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn run_topic(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    scope: TopicScope,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let topic = load_topic(&state, &id, &user, scope).await?;
    let run_id = runner::start(&state, &topic, "manual").await?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "run_id": run_id }))))
}

async fn list_runs(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<RunsQuery>,
    scope: TopicScope,
) -> Result<Json<Vec<Run>>> {
    load_topic(&state, &id, &user, scope).await?;
    let runs = sqlx::query_as::<_, Run>(
        "SELECT id, triggered_by, status, started_at, finished_at, model,
                input_tokens, cached_tokens, output_tokens, reasoning_tokens,
                web_search_calls, x_search_calls, cost_ticks, returned_count,
                new_count, dup_count, dropped_count, error
         FROM runs WHERE topic_id = $1 ORDER BY started_at DESC LIMIT $2",
    )
    .bind(&id)
    .bind(query.limit.unwrap_or(50).clamp(1, 200))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(runs))
}

async fn list_items(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiQuery(query): ApiQuery<ItemsQuery>,
    scope: TopicScope,
) -> Result<Json<Vec<Item>>> {
    load_topic(&state, &id, &user, scope).await?;
    let day = match query.day {
        Some(day) => day,
        None => sqlx::query_scalar::<_, Option<NaiveDate>>(
            "SELECT MAX(day) FROM items WHERE topic_id = $1",
        )
        .bind(&id)
        .fetch_one(&state.pool)
        .await?
        .unwrap_or_else(|| state.digest.today()),
    };
    let items = sqlx::query_as::<_, Item>(
        "SELECT id, topic_id, day, title, summary, title_en, summary_en, url, source,
                published_at, tags, tags_en, importance, first_seen_at, last_seen_at,
                seen_count, hidden, pinned
         FROM items WHERE topic_id = $1 AND day = $2
         ORDER BY pinned DESC, published_at DESC NULLS LAST, first_seen_at DESC",
    )
    .bind(&id)
    .bind(day)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(items))
}

async fn update_item(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<Uuid>,
    ApiJson(request): ApiJson<UpdateItem>,
    scope: TopicScope,
) -> Result<Json<Item>> {
    let mut transaction = state.pool.begin().await?;
    let item = sqlx::query_as::<_, Item>(
        "UPDATE items SET hidden = COALESCE($2, hidden), pinned = COALESCE($3, pinned)
         WHERE id = $1 AND EXISTS (
             SELECT 1 FROM topics WHERE topics.id = items.topic_id
             AND (($4 AND topics.owner_user_id IS NULL) OR (NOT $4 AND topics.owner_user_id = $5))
         )
         RETURNING id, topic_id, day, title, summary, title_en, summary_en, url, source,
                   published_at, tags, tags_en, importance, first_seen_at, last_seen_at,
                   seen_count, hidden, pinned",
    )
    .bind(id)
    .bind(request.hidden)
    .bind(request.pinned)
    .bind(scope.is_public())
    .bind(user.id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| AppError::NotFound("条目不存在".into()))?;
    if request.hidden.is_some() || request.pinned.is_some() {
        sqlx::query("UPDATE topics SET feed_revision = feed_revision + 1 WHERE id = $1")
            .bind(&item.topic_id)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    if scope.is_public() {
        state
            .digest
            .refresh(&state, &item.topic_id, item.day)
            .await?;
    }
    Ok(Json(item))
}

async fn delete_item(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<Uuid>,
    scope: TopicScope,
) -> Result<Response> {
    let mut transaction = state.pool.begin().await?;
    let (topic_id, day) = sqlx::query_as::<_, (String, NaiveDate)>(
        "DELETE FROM items WHERE id = $1 AND EXISTS (SELECT 1 FROM topics WHERE topics.id = items.topic_id AND (($2 AND topics.owner_user_id IS NULL) OR (NOT $2 AND topics.owner_user_id = $3))) RETURNING topic_id, day",
    )
    .bind(id)
    .bind(scope.is_public())
    .bind(user.id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| AppError::NotFound("条目不存在".into()))?;
    sqlx::query("UPDATE topics SET feed_revision = feed_revision + 1 WHERE id = $1")
        .bind(&topic_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    if scope.is_public() {
        state.digest.refresh(&state, &topic_id, day).await?;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

// API wrappers make the data scope explicit: `/api/me` is one personal account,
// while `/api/admin` is limited to the public editorial catalogue.
async fn me_status(State(state): State<AppState>, user: User) -> Result<Json<serde_json::Value>> {
    auth::require_personal(&user)?;
    status(State(state), user, TopicScope::Personal).await
}

async fn me_list_topics(State(state): State<AppState>, user: User) -> Result<Json<Vec<Topic>>> {
    auth::require_personal(&user)?;
    list_topics(State(state), user, TopicScope::Personal).await
}

async fn me_create_topic(
    State(state): State<AppState>,
    user: User,
    request: ApiJson<CreateTopic>,
) -> Result<(StatusCode, Json<Topic>)> {
    auth::require_personal(&user)?;
    create_topic(State(state), user, request, TopicScope::Personal).await
}

async fn me_update_topic(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
    fields: ApiJson<TopicFields>,
) -> Result<Json<Topic>> {
    auth::require_personal(&user)?;
    update_topic(State(state), user, path, fields, TopicScope::Personal).await
}

async fn me_delete_topic(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
) -> Result<Response> {
    auth::require_personal(&user)?;
    delete_topic(State(state), user, path, TopicScope::Personal).await
}

async fn me_run_topic(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    auth::require_personal(&user)?;
    run_topic(State(state), user, path, TopicScope::Personal).await
}

async fn me_list_runs(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
    query: ApiQuery<RunsQuery>,
) -> Result<Json<Vec<Run>>> {
    auth::require_personal(&user)?;
    list_runs(State(state), user, path, query, TopicScope::Personal).await
}

async fn me_list_items(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
    query: ApiQuery<ItemsQuery>,
) -> Result<Json<Vec<Item>>> {
    auth::require_personal(&user)?;
    list_items(State(state), user, path, query, TopicScope::Personal).await
}

async fn me_update_item(
    State(state): State<AppState>,
    user: User,
    path: Path<Uuid>,
    request: ApiJson<UpdateItem>,
) -> Result<Json<Item>> {
    auth::require_personal(&user)?;
    update_item(State(state), user, path, request, TopicScope::Personal).await
}

async fn me_delete_item(
    State(state): State<AppState>,
    user: User,
    path: Path<Uuid>,
) -> Result<Response> {
    auth::require_personal(&user)?;
    delete_item(State(state), user, path, TopicScope::Personal).await
}

async fn admin_status(
    State(state): State<AppState>,
    user: User,
) -> Result<Json<serde_json::Value>> {
    auth::require_admin(&user)?;
    status(State(state), user, TopicScope::Public).await
}

async fn admin_list_topics(State(state): State<AppState>, user: User) -> Result<Json<Vec<Topic>>> {
    auth::require_admin(&user)?;
    list_topics(State(state), user, TopicScope::Public).await
}

async fn admin_create_topic(
    State(state): State<AppState>,
    user: User,
    request: ApiJson<CreateTopic>,
) -> Result<(StatusCode, Json<Topic>)> {
    auth::require_admin(&user)?;
    create_topic(State(state), user, request, TopicScope::Public).await
}

async fn admin_update_topic(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
    fields: ApiJson<TopicFields>,
) -> Result<Json<Topic>> {
    auth::require_admin(&user)?;
    update_topic(State(state), user, path, fields, TopicScope::Public).await
}

async fn admin_delete_topic(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
) -> Result<Response> {
    auth::require_admin(&user)?;
    delete_topic(State(state), user, path, TopicScope::Public).await
}

async fn admin_run_topic(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    auth::require_admin(&user)?;
    run_topic(State(state), user, path, TopicScope::Public).await
}

async fn admin_list_runs(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
    query: ApiQuery<RunsQuery>,
) -> Result<Json<Vec<Run>>> {
    auth::require_admin(&user)?;
    list_runs(State(state), user, path, query, TopicScope::Public).await
}

async fn admin_list_items(
    State(state): State<AppState>,
    user: User,
    path: Path<String>,
    query: ApiQuery<ItemsQuery>,
) -> Result<Json<Vec<Item>>> {
    auth::require_admin(&user)?;
    list_items(State(state), user, path, query, TopicScope::Public).await
}

async fn admin_update_item(
    State(state): State<AppState>,
    user: User,
    path: Path<Uuid>,
    request: ApiJson<UpdateItem>,
) -> Result<Json<Item>> {
    auth::require_admin(&user)?;
    update_item(State(state), user, path, request, TopicScope::Public).await
}

async fn admin_delete_item(
    State(state): State<AppState>,
    user: User,
    path: Path<Uuid>,
) -> Result<Response> {
    auth::require_admin(&user)?;
    delete_item(State(state), user, path, TopicScope::Public).await
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
                Some("topics_window_valid") => {
                    "时间窗必须有效；结束时间早于开始时间表示跨午夜，且两者不能相同"
                }
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
    use super::{Topic, effective_run_delay_minutes, next_run};
    use chrono::{DateTime, TimeZone, Utc};
    use chrono_tz::Asia::Shanghai;

    fn topic() -> Topic {
        Topic {
            id: "ai".into(),
            owner_user_id: None,
            plan_locked: false,
            name: "AI".into(),
            name_en: String::new(),
            description: String::new(),
            brief: "AI".into(),
            enabled: true,
            visible: true,
            interval_minutes: 60,
            window_start: 6,
            window_end: 24,
            weekdays: vec![1, 2, 3, 4, 5],
            model: "grok-4.7".into(),
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

    fn overnight_topic() -> Topic {
        let mut topic = topic();
        topic.id = "overnight".into();
        topic.window_start = 22;
        topic.window_end = 6;
        topic.interval_minutes = 60;
        topic
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
    fn supports_overnight_windows_and_assigns_after_midnight_to_start_day() {
        let overnight = overnight_topic();
        // Thursday's window starts at 22:00.
        assert_eq!(
            next_run(
                &overnight,
                Shanghai,
                local(2026, 9, 3, 21, 0),
                Some(local(2026, 9, 3, 21, 0)),
                1
            ),
            local(2026, 9, 3, 22, 0)
        );
        // Friday 02:00 still belongs to Thursday's allowed window.
        assert_eq!(
            next_run(
                &overnight,
                Shanghai,
                local(2026, 9, 4, 2, 0),
                Some(local(2026, 9, 4, 1, 0)),
                1
            ),
            local(2026, 9, 4, 2, 0)
        );
        // The daytime gap resumes at Friday 22:00.
        assert_eq!(
            next_run(
                &overnight,
                Shanghai,
                local(2026, 9, 4, 7, 0),
                Some(local(2026, 9, 4, 5, 0)),
                1
            ),
            local(2026, 9, 4, 22, 0)
        );
        // Saturday morning is still Friday's window; after it closes, skip to Monday.
        assert_eq!(
            next_run(
                &overnight,
                Shanghai,
                local(2026, 9, 5, 7, 0),
                Some(local(2026, 9, 5, 5, 0)),
                1
            ),
            local(2026, 9, 7, 22, 0)
        );
        // A cap reached after midnight waits for that day's next start, not the following day.
        let mut capped = overnight.clone();
        capped.daily_run_cap = 1;
        assert_eq!(
            next_run(
                &capped,
                Shanghai,
                local(2026, 9, 4, 2, 0),
                Some(local(2026, 9, 4, 1, 0)),
                1
            ),
            local(2026, 9, 4, 22, 0)
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

    #[test]
    fn admission_delay_matches_failure_backoff() {
        assert_eq!(effective_run_delay_minutes(120, 0, 15), 120);
        assert_eq!(effective_run_delay_minutes(120, 1, 15), 15);
        assert_eq!(effective_run_delay_minutes(120, 3, 15), 60);
        assert_eq!(effective_run_delay_minutes(60, 1, 120), 120);
    }
}
