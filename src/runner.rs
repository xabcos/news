use crate::{
    auth::{self, PlanLimits},
    error::{AppError, Result},
    fetch::{self, Known},
    insight,
    state::AppState,
    topics::{self, Topic},
};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use serde_json::Value;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;
use tokio::{task::JoinSet, time::MissedTickBehavior};
use uuid::Uuid;

const TICK: Duration = Duration::from_secs(30);
const TICK_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(300);
const MAINTENANCE_TICK: Duration = Duration::from_secs(60);
const CLEANUP_EVERY_TICKS: u32 = 60;
const KNOWN_LIMIT: i64 = 80;
const RUN_RETENTION_DAYS: i64 = 3;
const INSIGHT_RETENTION_DAYS: i64 = 32;
const CLEANUP_BATCH_SIZE: i64 = 500;
const CLEANUP_MAX_BATCHES: usize = 20;

/// Starts the scheduler (unless disabled) and the maintenance loop.
pub fn spawn(state: AppState) -> JoinSet<()> {
    let mut tasks = JoinSet::new();
    if state.config.scheduler_enabled {
        tasks.spawn(schedule_loop(state.clone()));
        tasks.spawn(maintenance_loop(state));
    } else {
        // Read-only mode still clears stale running markers from a previous
        // process, so the workspace does not remain stuck on "running".
        tasks.spawn(async move {
            if let Err(error) = recover(&state).await {
                tracing::error!(%error, "failed to recover scheduler state");
            }
            maintenance_loop(state).await;
        });
    }
    tasks
}

async fn schedule_loop(state: AppState) {
    if let Err(error) = recover(&state).await {
        tracing::error!(%error, "failed to recover scheduler state");
    }
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_tick_error: Option<(String, Instant)> = None;
    loop {
        interval.tick().await;
        match tick(&state).await {
            Ok(()) => last_tick_error = None,
            Err(error) => {
                let message = error.to_string();
                let should_log = match &last_tick_error {
                    Some((previous, logged_at)) => {
                        previous != &message || logged_at.elapsed() >= TICK_ERROR_LOG_INTERVAL
                    }
                    None => true,
                };
                if should_log {
                    tracing::warn!(%error, "scheduler tick failed");
                    last_tick_error = Some((message, Instant::now()));
                }
            }
        }
    }
}

/// Runs cut short by a restart are closed out, and every enabled topic gets a next slot.
async fn recover(state: &AppState) -> Result<()> {
    sqlx::query(
        "UPDATE runs SET status = 'interrupted', finished_at = NOW() WHERE status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    sqlx::query(
        "UPDATE ai_insights SET status = 'failed', finished_at = NOW(), error = '进程重启中断' WHERE status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    // `register_run` marks the topic as running before the async task starts.
    // Clear that marker on restart as well, otherwise the workspace would show
    // a permanently running topic even though its run was interrupted.
    sqlx::query(
        "UPDATE topics
         SET last_status = 'interrupted', consecutive_failures = consecutive_failures + 1
         WHERE last_status = 'running'",
    )
    .execute(&state.pool)
    .await?;
    let unscheduled =
        sqlx::query_as::<_, Topic>("SELECT * FROM topics WHERE enabled AND next_run_at IS NULL")
            .fetch_all(&state.pool)
            .await?;
    for topic in unscheduled {
        if let Err(error) = reschedule_or_defer(state, &topic).await {
            tracing::warn!(topic = %topic.id, %error, "failed to recover topic schedule");
        }
    }
    Ok(())
}

/// Recomputes and stores `next_run_at` from the topic's last run; disabled topics stay
/// unscheduled.
async fn reschedule(state: &AppState, topic: &Topic) -> Result<Topic> {
    let limits = topics::limits_for_topic(state, topic).await?;
    if topic.owner_user_id.is_some()
        && !topics::personal_topic_allowed(state, topic, limits.max_topics).await?
    {
        // Keep the topic visible for a later upgrade, but remove its due slot
        // so the scheduler does not repeatedly attempt a downgraded topic.
        return Ok(sqlx::query_as(
            "UPDATE topics SET next_run_at = NULL WHERE id = $1 RETURNING *",
        )
        .bind(&topic.id)
        .fetch_one(&state.pool)
        .await?);
    }
    let mut schedule_topic = topic.clone();
    // A plan change takes effect without rewriting every stored topic.  The
    // effective cadence/cap are applied at each scheduling boundary.
    if topic.owner_user_id.is_some() {
        schedule_topic.interval_minutes = schedule_topic
            .interval_minutes
            .max(limits.min_interval_minutes);
        schedule_topic.daily_run_cap = limits.daily_run_cap;
    }
    let next = match topic.enabled {
        true => Some(topics::next_run_with_min_interval(
            &schedule_topic,
            state.config.timezone,
            Utc::now(),
            topic.last_run_at,
            today_runs(state, topic).await?,
            limits.min_interval_minutes,
        )),
        false => None,
    };
    Ok(
        sqlx::query_as("UPDATE topics SET next_run_at = $2 WHERE id = $1 RETURNING *")
            .bind(&topic.id)
            .bind(next)
            .fetch_one(&state.pool)
            .await?,
    )
}

/// A topic is already committed before scheduling is recalculated (for
/// example after a create or a run finishes). If a transient lookup fails,
/// leave a short retry slot instead of a permanent NULL schedule.
pub(crate) async fn reschedule_or_defer(state: &AppState, topic: &Topic) -> Result<Topic> {
    match reschedule(state, topic).await {
        Ok(topic) => Ok(topic),
        Err(error) => {
            tracing::warn!(topic = %topic.id, %error, "topic reschedule failed; deferring retry");
            match sqlx::query_as::<_, Topic>(
                "UPDATE topics
                 SET next_run_at = COALESCE(next_run_at, NOW() + INTERVAL '1 minute')
                 WHERE id = $1
                 RETURNING *",
            )
            .bind(&topic.id)
            .fetch_optional(&state.pool)
            .await
            {
                Ok(Some(topic)) => Ok(topic),
                Ok(None) => Err(error),
                Err(defer_error) => {
                    tracing::warn!(%defer_error, topic = %topic.id, "failed to defer topic retry");
                    Err(error)
                }
            }
        }
    }
}

async fn today_runs(state: &AppState, topic: &Topic) -> Result<i64> {
    let since = topics::day_start(state);
    if let Some(owner_user_id) = topic.owner_user_id {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id = $1 AND runs.started_at >= $2",
        )
        .bind(owner_user_id)
        .bind(since)
        .fetch_one(&state.pool)
        .await?)
    } else {
        Ok(
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM runs WHERE topic_id = $1 AND started_at >= $2",
            )
            .bind(&topic.id)
            .bind(since)
            .fetch_one(&state.pool)
            .await?,
        )
    }
}

/// Claims due topics up to the free concurrency slots and starts them. Claiming clears
/// `next_run_at`, so a topic cannot be picked twice; the run reschedules it on completion.
async fn tick(state: &AppState) -> Result<()> {
    let slots = state.run_slots.available_permits();
    if slots == 0 {
        return Ok(());
    }
    let due = sqlx::query_as::<_, Topic>(
        r#"
        WITH due AS (
            SELECT id FROM topics
            WHERE enabled AND next_run_at <= NOW()
            ORDER BY next_run_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE topics SET next_run_at = NULL FROM due WHERE topics.id = due.id
        RETURNING topics.*
        "#,
    )
    .bind(slots as i64)
    .fetch_all(&state.pool)
    .await?;
    for topic in due {
        // A manual run may be in flight; it reschedules the topic when it finishes.
        if let Err(error) = start(state, &topic, "schedule").await {
            tracing::debug!(%error, "scheduled run skipped");
            // Claiming clears next_run_at.  If admission rejected the run,
            // put the topic back on a future slot instead of silently stopping
            // its schedule.
            if let Err(reschedule_error) = reschedule_or_defer(state, &topic).await {
                tracing::warn!(%reschedule_error, "failed to reschedule skipped topic");
            }
        }
    }
    insight::tick(state).await?;
    Ok(())
}

/// Registers a run and spawns the fetch. One run per topic at a time.
pub(crate) async fn start(
    state: &AppState,
    topic: &Topic,
    triggered_by: &'static str,
) -> Result<Uuid> {
    let permit = state
        .run_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests("当前运行任务较多，请稍后重试".into()))?;
    if !state.in_flight().insert(topic.id.clone()) {
        drop(permit);
        return Err(AppError::Conflict("该主题正在抓取中".into()));
    }
    let run_id = Uuid::new_v4();
    let started_at = Utc::now();
    let topic_id = topic.id.clone();
    let registered = register_run(state, topic, run_id, started_at, triggered_by).await;
    let topic = match registered {
        Ok(topic) => topic,
        Err(error) => {
            state.in_flight().remove(&topic_id);
            return Err(error);
        }
    };
    tokio::spawn(run(state.clone(), topic, run_id, started_at, permit));
    Ok(run_id)
}

/// Checks cadence/cost admission and inserts the running row in one transaction.
/// A user advisory lock makes the daily cap hard across several personal topics;
/// the topic row lock also prevents two instances from starting the same topic.
async fn register_run(
    state: &AppState,
    requested: &Topic,
    run_id: Uuid,
    started_at: DateTime<Utc>,
    triggered_by: &'static str,
) -> Result<Topic> {
    let mut transaction = state.pool.begin().await?;
    if let Some(owner_user_id) = requested.owner_user_id {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(owner_user_id)
            .execute(&mut *transaction)
            .await?;
    }
    let topic = sqlx::query_as::<_, Topic>("SELECT * FROM topics WHERE id = $1 FOR UPDATE")
        .bind(&requested.id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| AppError::NotFound("主题不存在".into()))?;
    if !topic.enabled {
        return Err(AppError::Conflict("该主题已停用".into()));
    }
    let limits = if let Some(owner_user_id) = topic.owner_user_id {
        let pro_until = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT pro_until FROM users WHERE id = $1",
        )
        .bind(owner_user_id)
        .fetch_one(&mut *transaction)
        .await?;
        if pro_until.is_some_and(|until| until > Utc::now()) {
            auth::PRO_LIMITS
        } else {
            auth::FREE_LIMITS
        }
    } else {
        PlanLimits {
            max_topics: i64::MAX,
            min_interval_minutes: 15,
            daily_run_cap: topic.daily_run_cap.max(1),
        }
    };
    if let Some(owner_user_id) = topic.owner_user_id {
        let position = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM topics
             WHERE owner_user_id = $1
               AND (created_at < $2 OR (created_at = $2 AND id <= $3))",
        )
        .bind(owner_user_id)
        .bind(topic.created_at)
        .bind(&topic.id)
        .fetch_one(&mut *transaction)
        .await?;
        if position > limits.max_topics {
            return Err(AppError::Conflict(
                "当前套餐只支持部分主题，请升级 Pro 或删除多余主题".into(),
            ));
        }
    }
    let interval = topics::effective_run_delay_minutes(
        topic.interval_minutes,
        topic.consecutive_failures,
        limits.min_interval_minutes,
    );
    if let Some(last) = topic.last_run_at {
        let elapsed = (Utc::now() - last).num_minutes();
        if elapsed < interval {
            return Err(AppError::TooManyRequests(
                format!("请在 {} 分钟后再运行该主题", interval - elapsed).into(),
            ));
        }
    }
    let since = topics::day_start(state);
    let runs_today = if let Some(owner_user_id) = topic.owner_user_id {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id = $1 AND runs.started_at >= $2",
        )
        .bind(owner_user_id)
        .bind(since)
        .fetch_one(&mut *transaction)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM runs WHERE topic_id = $1 AND started_at >= $2",
        )
        .bind(&topic.id)
        .bind(since)
        .fetch_one(&mut *transaction)
        .await?
    };
    if runs_today >= i64::from(limits.daily_run_cap) {
        return Err(AppError::TooManyRequests("今日运行次数已达到上限".into()));
    }
    sqlx::query(
        "INSERT INTO runs (id, topic_id, triggered_by, status, started_at, model) VALUES ($1, $2, $3, 'running', $4, $5)",
    )
    .bind(run_id)
    .bind(&topic.id)
    .bind(triggered_by)
    .bind(started_at)
    .bind(&topic.model)
    .execute(&mut *transaction)
    .await?;
    // Reserve the cadence before releasing the row lock. This also prevents a
    // second process from starting the same topic while the first fetch runs.
    sqlx::query("UPDATE topics SET last_run_at = $2, last_status = 'running' WHERE id = $1")
        .bind(&topic.id)
        .bind(started_at)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(topic)
}

async fn run(
    state: AppState,
    topic: Topic,
    run_id: Uuid,
    started_at: DateTime<Utc>,
    _permit: OwnedSemaphorePermit,
) {
    let clock = Instant::now();
    let outcome =
        match tokio::time::timeout(state.config.fetch_timeout, execute(&state, &topic, run_id))
            .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(AppError::Internal("抓取超时".into())),
        };
    let failure = outcome.as_ref().err().map(ToString::to_string);
    match finish(
        &state,
        &topic.id,
        topic.owner_user_id.is_none(),
        run_id,
        started_at,
        outcome,
    )
    .await
    {
        Ok(()) => match failure {
            Some(error) => tracing::warn!(
                topic = %topic.id,
                %error,
                elapsed_ms = clock.elapsed().as_millis() as u64,
                "run failed"
            ),
            None => tracing::info!(
                topic = %topic.id,
                elapsed_ms = clock.elapsed().as_millis() as u64,
                "run finished"
            ),
        },
        Err(error) => tracing::error!(topic = %topic.id, %error, "failed to record run"),
    }
    state.in_flight().remove(&topic.id);
}

struct Completed {
    response_id: Option<String>,
    usage: fetch::Usage,
    payload: Value,
    day: NaiveDate,
    returned: i32,
    new: i32,
    dup: i32,
    dropped: i32,
}

/// Fetches, validates, deduplicates against the last two days and stores the new stories.
async fn execute(state: &AppState, topic: &Topic, run_id: Uuid) -> Result<Completed> {
    let today = state.digest.today();
    let yesterday = today.pred_opt().expect("date in range");
    let known = sqlx::query_as::<_, Known>(
        "SELECT title, url, fingerprint FROM items WHERE topic_id = $1 AND day >= $2 ORDER BY first_seen_at DESC LIMIT $3",
    )
    .bind(&topic.id)
    .bind(yesterday)
    .bind(KNOWN_LIMIT)
    .fetch_all(&state.pool)
    .await?;
    let fetched = fetch::fetch(state, topic, &known).await?;

    let cutoff = Utc::now() - ChronoDuration::hours(i64::from(topic.lookback_hours) + 12);
    let mut fingerprints = known
        .iter()
        .map(|item| item.fingerprint.clone())
        .collect::<HashSet<_>>();
    let mut titles = known
        .iter()
        .map(|item| fetch::normalize_title(&item.title))
        .collect::<Vec<_>>();
    let returned = fetched.candidates.len() as i32;
    let (mut new, mut dup, mut dropped) = (0, 0, 0);
    let mut tx = state.pool.begin().await?;
    for candidate in fetched.candidates {
        let Some(item) = fetch::validate(candidate, cutoff) else {
            dropped += 1;
            continue;
        };
        let title = fetch::normalize_title(&item.title);
        if !fingerprints.insert(item.fingerprint.clone()) || fetch::similar_title(&titles, &title) {
            dup += 1;
            sqlx::query(
                "UPDATE items SET last_seen_at = NOW(), seen_count = seen_count + 1 WHERE topic_id = $1 AND day >= $2 AND fingerprint = $3",
            )
            .bind(&topic.id)
            .bind(yesterday)
            .bind(&item.fingerprint)
            .execute(&mut *tx)
            .await?;
            continue;
        }
        titles.push(title);
        sqlx::query(
            "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, title_en, summary_en, url, source, published_at, tags, importance, first_run_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) ON CONFLICT DO NOTHING",
        )
        .bind(Uuid::new_v4())
        .bind(&topic.id)
        .bind(today)
        .bind(&item.fingerprint)
        .bind(&item.title)
        .bind(&item.summary)
        .bind(&item.title_en)
        .bind(&item.summary_en)
        .bind(&item.url)
        .bind(&item.source)
        .bind(item.published_at)
        .bind(&item.tags)
        .bind(item.importance)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        new += 1;
    }
    tx.commit().await?;
    Ok(Completed {
        response_id: fetched.response_id,
        usage: fetched.usage,
        payload: fetched.payload,
        day: today,
        returned,
        new,
        dup,
        dropped,
    })
}

/// Records the outcome, updates the topic's run state and schedules the next run.
async fn finish(
    state: &AppState,
    topic_id: &str,
    public_topic: bool,
    run_id: Uuid,
    started_at: DateTime<Utc>,
    outcome: Result<Completed>,
) -> Result<()> {
    let (status, refreshed_day) = match outcome {
        Ok(done) => {
            let usage = &done.usage;
            if let Err(error) = sqlx::query(
                "UPDATE runs SET status = 'ok', finished_at = NOW(), response_id = $2, input_tokens = $3, cached_tokens = $4, \
                 output_tokens = $5, reasoning_tokens = $6, web_search_calls = $7, x_search_calls = $8, cost_ticks = $9, \
                 returned_count = $10, new_count = $11, dup_count = $12, dropped_count = $13, payload = $14 WHERE id = $1",
            )
            .bind(run_id)
            .bind(&done.response_id)
            .bind(usage.input_tokens)
            .bind(usage.input_tokens_details.cached_tokens)
            .bind(usage.output_tokens)
            .bind(usage.output_tokens_details.reasoning_tokens)
            .bind(usage.server_side_tool_usage_details.web_search_calls)
            .bind(usage.server_side_tool_usage_details.x_search_calls)
            .bind(usage.cost_in_usd_ticks)
            .bind(done.returned)
            .bind(done.new)
            .bind(done.dup)
            .bind(done.dropped)
            .bind(&done.payload)
            .execute(&state.pool)
            .await
            {
                defer_after_finish_error(state, topic_id).await;
                return Err(error.into());
            }
            ("ok", (done.new > 0).then_some(done.day))
        }
        Err(error) => {
            if let Err(update_error) = sqlx::query(
                "UPDATE runs SET status = 'failed', finished_at = NOW(), error = $2 WHERE id = $1",
            )
            .bind(run_id)
            .bind(error.to_string())
            .execute(&state.pool)
            .await
            {
                defer_after_finish_error(state, topic_id).await;
                return Err(update_error.into());
            }
            ("failed", None)
        }
    };
    let topic = match sqlx::query_as::<_, Topic>(
        "UPDATE topics SET last_run_at = $2, last_status = $3, \
         consecutive_failures = CASE WHEN $3 = 'ok' THEN 0 ELSE consecutive_failures + 1 END \
         WHERE id = $1 RETURNING *",
    )
    .bind(topic_id)
    .bind(started_at)
    .bind(status)
    .fetch_one(&state.pool)
    .await
    {
        Ok(topic) => topic,
        Err(error) => {
            // The claim cleared next_run_at before the fetch started. A transient
            // status-write failure must not turn the topic into a permanent orphan.
            defer_after_finish_error(state, topic_id).await;
            return Err(error.into());
        }
    };
    reschedule_or_defer(state, &topic).await?;
    if public_topic && let Some(day) = refreshed_day {
        state.digest.refresh(state, topic_id, day).await?;
    }
    Ok(())
}

/// A claimed topic has `next_run_at = NULL` until `finish` schedules it again.
/// If either completion write fails, leave a short retry slot so a transient
/// database error cannot orphan the topic indefinitely.
async fn defer_after_finish_error(state: &AppState, topic_id: &str) {
    if let Err(error) = sqlx::query(
        "UPDATE topics
         SET next_run_at = COALESCE(next_run_at, NOW() + INTERVAL '1 minute'),
             last_status = CASE WHEN last_status = 'running' THEN 'interrupted' ELSE last_status END
         WHERE id = $1",
    )
    .bind(topic_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%error, topic = %topic_id, "failed to defer topic after finish error");
    }
}

/// Every minute: roll the day over when the local date changes; hourly: prune old data and
/// expire authentication rows. The initial prune makes a restart sufficient to catch up.
async fn maintenance_loop(state: AppState) {
    let mut interval = tokio::time::interval(MAINTENANCE_TICK);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut day = state.digest.today();
    let mut ticks = 0_u32;
    if let Err(error) = prune(&state, day).await {
        tracing::warn!(%error, "initial data pruning failed");
    }
    loop {
        interval.tick().await;
        ticks += 1;
        let today = state.digest.today();
        if today != day {
            day = today;
            state.digest.rollover();
        }
        if ticks >= CLEANUP_EVERY_TICKS {
            ticks = 0;
            if let Err(error) = prune(&state, today).await {
                tracing::warn!(%error, "scheduled data pruning failed");
            }
            if let Err(error) = auth::cleanup(&state).await {
                tracing::warn!(%error, "failed to clean expired authentication records");
            }
        }
    }
}

/// Removes old runtime history and news items in bounded batches. A failed table does not
/// prevent the other tables from being attempted during the same maintenance pass.
async fn prune(state: &AppState, today: NaiveDate) -> Result<()> {
    let mut first_error = None;
    if let Err(error) = cleanup_runs(state).await {
        tracing::warn!(%error, "run history pruning failed");
        first_error = Some(error);
    }
    if let Err(error) = cleanup_insights(state).await {
        tracing::warn!(%error, "AI insight history pruning failed");
        if first_error.is_none() {
            first_error = Some(error);
        }
    }
    // Batches commit one by one, so a failed pass may still have removed the newest row.
    state.insight.invalidate();
    if state.config.retention_days > 0 {
        let keep_from = today - ChronoDuration::days(i64::from(state.config.retention_days));
        if let Err(error) = cleanup_items(state, keep_from).await {
            tracing::warn!(%error, "news item pruning failed");
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
        if let Err(error) = state.digest.prune_before(state, keep_from).await {
            tracing::warn!(%error, "news cache pruning failed");
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

async fn cleanup_runs(state: &AppState) -> Result<()> {
    cleanup_in_batches(
        state,
        "runs",
        r#"
        WITH expired AS (
            SELECT id
            FROM runs
            WHERE status <> 'running'
              AND started_at < NOW() - ($2 * INTERVAL '1 day')
            ORDER BY started_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM runs
        USING expired
        WHERE runs.id = expired.id
        "#,
        RUN_RETENTION_DAYS,
    )
    .await
}

async fn cleanup_insights(state: &AppState) -> Result<()> {
    cleanup_in_batches(
        state,
        "ai_insights",
        r#"
        WITH expired AS (
            SELECT id
            FROM ai_insights
            WHERE status <> 'running'
              AND started_at < NOW() - ($2 * INTERVAL '1 day')
            ORDER BY started_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM ai_insights
        USING expired
        WHERE ai_insights.id = expired.id
        "#,
        INSIGHT_RETENTION_DAYS,
    )
    .await
}

async fn cleanup_items(state: &AppState, keep_from: NaiveDate) -> Result<()> {
    let mut total = 0_u64;
    for batch_index in 0..CLEANUP_MAX_BATCHES {
        let deleted = sqlx::query(
            r#"
            WITH expired AS (
                SELECT id
                FROM items
                WHERE day < $2
                ORDER BY day, id
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            DELETE FROM items
            USING expired
            WHERE items.id = expired.id
            "#,
        )
        .bind(CLEANUP_BATCH_SIZE)
        .bind(keep_from)
        .execute(&state.pool)
        .await?
        .rows_affected();
        total += deleted;
        if deleted < CLEANUP_BATCH_SIZE as u64 {
            return Ok(());
        }
        if batch_index + 1 == CLEANUP_MAX_BATCHES {
            tracing::warn!(
                table = "items",
                total,
                "cleanup reached its per-run limit; remaining rows are deferred"
            );
        }
    }
    Ok(())
}

/// Deletes in bounded batches so a large backlog never holds locks for a whole pass.
async fn cleanup_in_batches(
    state: &AppState,
    table: &'static str,
    statement: &str,
    cutoff_days: i64,
) -> Result<()> {
    let mut total = 0_u64;
    for batch_index in 0..CLEANUP_MAX_BATCHES {
        let deleted = sqlx::query(statement)
            .bind(CLEANUP_BATCH_SIZE)
            .bind(cutoff_days)
            .execute(&state.pool)
            .await?
            .rows_affected();
        total += deleted;
        if deleted < CLEANUP_BATCH_SIZE as u64 {
            return Ok(());
        }
        if batch_index + 1 == CLEANUP_MAX_BATCHES {
            tracing::warn!(
                table,
                total,
                "cleanup reached its per-run limit; remaining rows are deferred"
            );
        }
    }
    Ok(())
}
