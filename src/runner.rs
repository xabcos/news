use crate::{
    auth,
    error::{AppError, Result},
    fetch::{self, Known},
    state::AppState,
    topics::{self, Topic},
};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, NaiveTime, Utc};
use serde_json::Value;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use tokio::{task::JoinSet, time::MissedTickBehavior};
use uuid::Uuid;

const TICK: Duration = Duration::from_secs(30);
const MAINTENANCE_TICK: Duration = Duration::from_secs(60);
const AUTH_CLEANUP_EVERY_TICKS: u32 = 60;
const KNOWN_LIMIT: i64 = 80;
const PAYLOAD_RETENTION_DAYS: i32 = 30;

/// Starts the scheduler (unless disabled) and the maintenance loop.
pub fn spawn(state: AppState) -> JoinSet<()> {
    let mut tasks = JoinSet::new();
    if state.config.scheduler_enabled {
        tasks.spawn(schedule_loop(state.clone()));
    }
    tasks.spawn(maintenance_loop(state));
    tasks
}

async fn schedule_loop(state: AppState) {
    if let Err(error) = recover(&state).await {
        tracing::error!(%error, "failed to recover scheduler state");
    }
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if let Err(error) = tick(&state).await {
            tracing::warn!(%error, "scheduler tick failed");
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
    let unscheduled =
        sqlx::query_as::<_, Topic>("SELECT * FROM topics WHERE enabled AND next_run_at IS NULL")
            .fetch_all(&state.pool)
            .await?;
    for topic in unscheduled {
        reschedule(state, topic).await?;
    }
    Ok(())
}

/// Recomputes and stores `next_run_at` from the topic's last run; disabled topics stay
/// unscheduled.
pub(crate) async fn reschedule(state: &AppState, topic: Topic) -> Result<Topic> {
    let next = match topic.enabled {
        true => Some(topics::next_run(
            &topic,
            state.config.timezone,
            Utc::now(),
            topic.last_run_at,
            today_runs(state, &topic.id).await?,
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

async fn today_runs(state: &AppState, topic_id: &str) -> Result<i64> {
    let since = topics::to_utc(
        state.config.timezone,
        state.digest.today().and_time(NaiveTime::MIN),
    );
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE topic_id = $1 AND started_at >= $2")
            .bind(topic_id)
            .bind(since)
            .fetch_one(&state.pool)
            .await?,
    )
}

/// Claims due topics up to the free concurrency slots and starts them. Claiming clears
/// `next_run_at`, so a topic cannot be picked twice; the run reschedules it on completion.
async fn tick(state: &AppState) -> Result<()> {
    let slots = state
        .config
        .max_concurrent_runs
        .saturating_sub(state.in_flight().len());
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
        if let Err(error) = start(state, topic, "schedule").await {
            tracing::info!(%error, "scheduled run skipped");
        }
    }
    Ok(())
}

/// Registers a run and spawns the fetch. One run per topic at a time.
pub(crate) async fn start(
    state: &AppState,
    topic: Topic,
    triggered_by: &'static str,
) -> Result<Uuid> {
    if !state.in_flight().insert(topic.id.clone()) {
        return Err(AppError::Conflict("该主题正在抓取中".into()));
    }
    let run_id = Uuid::new_v4();
    let started_at = Utc::now();
    let registered = sqlx::query(
        "INSERT INTO runs (id, topic_id, triggered_by, status, started_at, model) VALUES ($1, $2, $3, 'running', $4, $5)",
    )
    .bind(run_id)
    .bind(&topic.id)
    .bind(triggered_by)
    .bind(started_at)
    .bind(&topic.model)
    .execute(&state.pool)
    .await;
    if let Err(error) = registered {
        state.in_flight().remove(&topic.id);
        return Err(error.into());
    }
    tokio::spawn(run(state.clone(), topic, run_id, started_at));
    Ok(run_id)
}

async fn run(state: AppState, topic: Topic, run_id: Uuid, started_at: DateTime<Utc>) {
    let clock = Instant::now();
    let outcome =
        match tokio::time::timeout(state.config.fetch_timeout, execute(&state, &topic, run_id))
            .await
        {
            Ok(outcome) => outcome,
            Err(_) => Err(AppError::Internal("抓取超时".into())),
        };
    let status = if outcome.is_ok() { "ok" } else { "failed" };
    match finish(&state, &topic.id, run_id, started_at, outcome).await {
        Ok(()) => tracing::info!(
            topic = %topic.id,
            status,
            elapsed_ms = clock.elapsed().as_millis() as u64,
            "run finished"
        ),
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
    run_id: Uuid,
    started_at: DateTime<Utc>,
    outcome: Result<Completed>,
) -> Result<()> {
    let (status, refreshed_day) = match outcome {
        Ok(done) => {
            let usage = &done.usage;
            sqlx::query(
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
            .await?;
            ("ok", (done.new > 0).then_some(done.day))
        }
        Err(error) => {
            sqlx::query(
                "UPDATE runs SET status = 'failed', finished_at = NOW(), error = $2 WHERE id = $1",
            )
            .bind(run_id)
            .bind(error.to_string())
            .execute(&state.pool)
            .await?;
            ("failed", None)
        }
    };
    let topic = sqlx::query_as::<_, Topic>(
        "UPDATE topics SET last_run_at = $2, last_status = $3, \
         consecutive_failures = CASE WHEN $3 = 'ok' THEN 0 ELSE consecutive_failures + 1 END \
         WHERE id = $1 RETURNING *",
    )
    .bind(topic_id)
    .bind(started_at)
    .bind(status)
    .fetch_one(&state.pool)
    .await?;
    reschedule(state, topic).await?;
    if let Some(day) = refreshed_day {
        state.digest.refresh(state, topic_id, day).await?;
    }
    Ok(())
}

/// Every minute: roll the day over when the local date changes; hourly: expire auth rows.
async fn maintenance_loop(state: AppState) {
    let mut interval = tokio::time::interval(MAINTENANCE_TICK);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut day = state.digest.today();
    let mut ticks = 0_u32;
    loop {
        interval.tick().await;
        ticks += 1;
        let today = state.digest.today();
        if today != day {
            day = today;
            state.digest.rollover();
            if let Err(error) = prune(&state, today).await {
                tracing::warn!(%error, "daily pruning failed");
            }
        }
        if ticks.is_multiple_of(AUTH_CLEANUP_EVERY_TICKS)
            && let Err(error) = auth::cleanup(&state).await
        {
            tracing::warn!(%error, "failed to clean expired authentication records");
        }
    }
}

/// Model payloads are only useful for recent troubleshooting; items are kept forever unless
/// a retention is configured.
async fn prune(state: &AppState, today: NaiveDate) -> Result<()> {
    sqlx::query(
        "UPDATE runs SET payload = NULL WHERE payload IS NOT NULL AND started_at < NOW() - $1 * INTERVAL '1 day'",
    )
    .bind(PAYLOAD_RETENTION_DAYS)
    .execute(&state.pool)
    .await?;
    if state.config.retention_days > 0 {
        let keep_from = today - ChronoDuration::days(i64::from(state.config.retention_days));
        sqlx::query("DELETE FROM items WHERE day < $1")
            .bind(keep_from)
            .execute(&state.pool)
            .await?;
    }
    Ok(())
}
