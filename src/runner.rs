use crate::{
    auth::{self, PlanLimits},
    error::{AppError, Result},
    fetch::{self, Known, NewsWindow},
    gallery, insight, media, reads,
    state::AppState,
    topics::{self, Topic},
    travel,
};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;
use tokio::{task::JoinSet, time::MissedTickBehavior};
use uuid::Uuid;

const TICK: Duration = Duration::from_secs(30);
const TICK_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(300);
const MAINTENANCE_TICK: Duration = Duration::from_secs(60);
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_secs(60 * 60);
const KNOWN_LIMIT: i64 = 80;
const RUN_RETENTION_DAYS: i64 = 3;
const INSIGHT_RETENTION_DAYS: i64 = 32;
const CLEANUP_BATCH_SIZE: i64 = 500;
const CLEANUP_MAX_BATCHES: usize = 20;
const BACKGROUND_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Starts the scheduler (unless disabled) and the maintenance loop. Detached workers are
/// registered in `AppState` so shutdown can cancel and join them before the process exits.
pub fn spawn(state: AppState) -> BackgroundTasks {
    let mut tasks = JoinSet::new();
    tasks.spawn(crate::payment::maintenance_loop(
        state.pool.clone(),
        state.config.timezone,
        state.shutdown.clone(),
    ));
    tasks.spawn(media_refresh_loop(state.clone()));
    tasks.spawn(reads::refresh_loop(state.clone()));
    if state.config.scheduler_enabled {
        tasks.spawn(schedule_loop(state.clone()));
        tasks.spawn(maintenance_loop(state.clone()));
    } else {
        // Read-only mode still clears stale running markers from a previous
        // process, so the workspace does not remain stuck on "running".
        let recovery_state = state.clone();
        tasks.spawn(async move {
            if let Err(error) = recover(&recovery_state).await {
                tracing::error!(%error, "failed to recover scheduler state");
            }
            maintenance_loop(recovery_state).await;
        });
    }
    BackgroundTasks {
        tasks,
        workers: state,
    }
}

pub struct BackgroundTasks {
    tasks: JoinSet<()>,
    workers: AppState,
}

impl BackgroundTasks {
    pub async fn shutdown(mut self) {
        self.workers.shutdown.cancel();
        let roots_finished = tokio::time::timeout(BACKGROUND_SHUTDOWN_TIMEOUT, async {
            while self.tasks.join_next().await.is_some() {}
        })
        .await
        .is_ok();
        if !roots_finished {
            self.tasks.abort_all();
            while self.tasks.join_next().await.is_some() {}
        }
        self.workers.shutdown_workers().await;
    }
}

async fn schedule_loop(state: AppState) {
    if let Err(error) = recover(&state).await {
        tracing::error!(%error, "failed to recover scheduler state");
    }
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_tick_error: Option<(String, Instant)> = None;
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
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
    insight::tick(state).await?;
    let slots = state.run_slots.available_permits();
    if slots == 0 {
        return Ok(());
    }
    let due = sqlx::query_as::<_, Topic>(
        r#"
        WITH due AS (
            SELECT id FROM topics
            WHERE enabled AND next_run_at <= NOW()
              AND ($2 OR owner_user_id IS NOT NULL)
            ORDER BY next_run_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        UPDATE topics SET next_run_at = NULL FROM due WHERE topics.id = due.id
        RETURNING topics.*
        "#,
    )
    .bind(slots as i64)
    .bind(!insight::is_running(state))
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
    Ok(())
}

/// Each completed topic is checkpointed by cutoff, so a retry only collects missing topics.
/// The coordinator holds no semaphore permit; even a single-slot deployment can progress.
/// Without a topic it collects what the briefing reads, the visible public news topics, so
/// an internal source such as `equity` or the photograph topics cannot hold the briefing back.
pub(crate) async fn collect_briefing(
    state: &AppState,
    window: NewsWindow,
    topic: Option<&str>,
) -> Result<()> {
    let topics = sqlx::query_as::<_, Topic>(
        "SELECT * FROM topics WHERE enabled AND owner_user_id IS NULL AND id NOT IN ('reads', 'llm', 'w', 'travel', 'movie', 'music', 'game', 'book', 'eink', 'ai-hardware', 'digital')
         AND (($2::text IS NULL AND visible) OR id = $2)
         AND NOT EXISTS (SELECT 1 FROM runs WHERE runs.topic_id = topics.id
                         AND triggered_by = 'briefing' AND status = 'ok' AND window_end = $1)
         ORDER BY sort_weight, id",
    )
    .bind(window.end)
    .bind(topic)
    .fetch_all(&state.pool)
    .await?;
    let mut jobs = JoinSet::new();
    for topic in topics {
        let state = state.clone();
        jobs.spawn(async move {
            let id = topic.id.clone();
            (id, collect_topic(&state, &topic, window).await)
        });
    }
    let mut failures = Vec::new();
    while let Some(result) = jobs.join_next().await {
        match result {
            Ok((id, Err(error))) => failures.push(format!("{id}: {error}")),
            Err(error) => failures.push(error.to_string()),
            Ok((_, Ok(()))) => {}
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Internal(
            format!("日报补采待重试：{}", failures.join("；")).into(),
        ))
    }
}

async fn collect_topic(state: &AppState, topic: &Topic, window: NewsWindow) -> Result<()> {
    let permit = tokio::select! {
        _ = state.shutdown.cancelled() => return Err(AppError::Internal("日报采集已中断".into())),
        permit = state.run_slots.clone().acquire_owned() =>
            permit.map_err(|_| AppError::Internal("调度器已关闭".into()))?,
    };
    if !state.in_flight().insert(topic.id.clone()) {
        return Err(AppError::Conflict("主题正在采集中".into()));
    }
    let run_id = Uuid::new_v4();
    let started_at = Utc::now();
    let registered = register_run(state, topic, run_id, started_at, "briefing", Some(window)).await;
    let topic = match registered {
        Ok(topic) => topic,
        Err(error) => {
            state.in_flight().remove(&topic.id);
            return Err(error);
        }
    };
    run(
        state.clone(),
        topic,
        run_id,
        started_at,
        permit,
        Some(window),
    )
    .await
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
    let registered = register_run(state, topic, run_id, started_at, triggered_by, None).await;
    let topic = match registered {
        Ok(topic) => topic,
        Err(error) => {
            state.in_flight().remove(&topic_id);
            return Err(error);
        }
    };
    let worker_state = state.clone();
    state.spawn_worker(async move {
        let _ = run(worker_state, topic, run_id, started_at, permit, None).await;
    });
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
    window: Option<NewsWindow>,
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
    if topic.last_status.as_deref() == Some("running") {
        return Err(AppError::Conflict("该主题正在抓取中".into()));
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
    if let Some(last) = topic.last_run_at
        && triggered_by != "briefing"
    {
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
        "INSERT INTO runs (id, topic_id, triggered_by, status, started_at, model, window_end) VALUES ($1, $2, $3, 'running', $4, $5, $6)",
    )
    .bind(run_id)
    .bind(&topic.id)
    .bind(triggered_by)
    .bind(started_at)
    .bind(&topic.model)
    .bind(window.map(|window| window.end))
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
    window: Option<NewsWindow>,
) -> Result<()> {
    let clock = Instant::now();
    let outcome = tokio::select! {
        _ = state.shutdown.cancelled() => {
            if let Err(error) = interrupt_run(&state, &topic.id, run_id, started_at).await {
                tracing::warn!(%error, topic = %topic.id, "failed to record cancelled run");
            }
            state.in_flight().remove(&topic.id);
            return Err(AppError::Internal("采集已中断".into()));
        }
        outcome = tokio::time::timeout(state.config.fetch_timeout, execute(&state, &topic, run_id, window.unwrap_or(NewsWindow {
            start: started_at - ChronoDuration::hours(i64::from(topic.lookback_hours)),
            end: started_at,
        }))) => {
            match outcome {
                Ok(outcome) => outcome,
                Err(_) => Err(AppError::Internal("抓取超时".into())),
            }
        }
    };
    let failure = outcome.as_ref().err().map(ToString::to_string);
    let recorded = finish(
        &state,
        &topic.id,
        topic.owner_user_id.is_none(),
        run_id,
        started_at,
        outcome,
    )
    .await;
    match &recorded {
        Ok(()) => match &failure {
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
    recorded?;
    match failure {
        Some(error) => Err(AppError::Internal(error.into())),
        None => Ok(()),
    }
}

async fn interrupt_run(
    state: &AppState,
    topic_id: &str,
    run_id: Uuid,
    started_at: DateTime<Utc>,
) -> Result<()> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("UPDATE runs SET status = 'interrupted', finished_at = NOW() WHERE id = $1")
        .bind(run_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE topics SET last_run_at = $2, last_status = 'interrupted', consecutive_failures = consecutive_failures + 1 WHERE id = $1",
    )
    .bind(topic_id)
    .bind(started_at)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
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

/// Collects and validates stories; Reads also prepares its body before publication.
async fn execute(
    state: &AppState,
    topic: &Topic,
    run_id: Uuid,
    window: NewsWindow,
) -> Result<Completed> {
    let started = Instant::now();
    let today = state.digest.today();
    if crate::llm::is_catalog(topic) {
        // The catalogue keeps its own records; the run counts providers and changes.
        let outcome = crate::llm::execute(state, topic).await?;
        return Ok(Completed {
            response_id: None,
            usage: outcome.usage,
            payload: outcome.payload,
            day: today,
            returned: outcome.read,
            new: outcome.changes,
            dup: outcome.unchanged,
            dropped: outcome.rejected,
        });
    }
    let yesterday = today.pred_opt().expect("date in range");
    let is_reads = topic.id == "reads" && topic.owner_user_id.is_none();
    let is_gallery = gallery::is_gallery(topic);
    let is_travel = travel::is_travel(topic);
    let catalog = crate::catalog::section(topic);
    let is_catalog = catalog.is_some();
    let window = if is_reads {
        NewsWindow {
            start: window.end - ChronoDuration::days(30),
            ..window
        }
    } else {
        window
    };
    if is_reads && reads::daily_count(state, today).await? >= reads::DAILY_LIMIT {
        return Ok(Completed {
            response_id: None,
            usage: fetch::Usage::default(),
            payload: serde_json::json!({"items": [], "skip": "reads_daily_limit"}),
            day: today,
            returned: 0,
            new: 0,
            dup: 0,
            dropped: 0,
        });
    }
    let known = sqlx::query_as::<_, Known>(
        "SELECT title, url, fingerprint FROM items WHERE topic_id = $1 AND ($4 OR day >= $2) ORDER BY first_seen_at DESC LIMIT $3",
    )
    .bind(&topic.id)
    .bind(if is_catalog { today - ChronoDuration::days(i64::from(topic.lookback_hours + 23) / 24) } else { yesterday })
    .bind(KNOWN_LIMIT)
    .bind(is_reads)
    .fetch_all(&state.pool)
    .await?;
    let mut fetched = if is_reads {
        reads::discover(state, topic, &known, window).await?
    } else {
        fetch::fetch(state, topic, &known, window).await?
    };

    let mut fingerprints = known
        .iter()
        .map(|item| item.fingerprint.clone())
        .collect::<HashSet<_>>();
    let mut titles = known
        .iter()
        .map(|item| fetch::normalize_title(&item.title))
        .collect::<Vec<_>>();
    let returned = fetched.candidates.len() as i32;
    let (mut dup, mut dropped) = (0, 0);
    let mut items = Vec::new();
    let mut duplicate_fingerprints = Vec::new();
    for candidate in std::mem::take(&mut fetched.candidates) {
        let Some(mut item) = fetch::validate(candidate, window.start)
            .filter(|item| item.published_at.is_none_or(|at| window.contains(at)))
        else {
            dropped += 1;
            continue;
        };
        // Specialized feeds know an X post by its id, whichever address the model used.
        if is_reads || is_gallery || is_travel || is_catalog {
            item.fingerprint =
                reads::fingerprint(&url::Url::parse(&item.url).expect("validated URL"));
        }
        if is_reads {
            item.image = None;
        }
        if (topic.id == "p" && item.image.is_none()) || (is_catalog && item.published_at.is_none())
        {
            dropped += 1;
            continue;
        }
        // Travelers at one place write alike; their posts are told apart by id alone.
        let title = fetch::normalize_title(&item.title);
        if !fingerprints.insert(item.fingerprint.clone())
            || (!is_travel && fetch::similar_title(&titles, &title))
        {
            dup += 1;
            duplicate_fingerprints.push(item.fingerprint);
            continue;
        }
        titles.push(title);
        items.push(item);
    }

    if is_gallery {
        let proposed = items.len();
        items = gallery::select(state, items, &known, window, &mut fetched.payload).await;
        dropped += (proposed - items.len()) as i32;
    }
    let mut moments = HashMap::new();
    if is_travel {
        let proposed = items.len();
        // Leave time to store the photographs and publish the moments.
        let deadline = tokio::time::Instant::now()
            + state
                .config
                .fetch_timeout
                .saturating_sub(started.elapsed() + Duration::from_secs(120));
        (items, moments) =
            travel::select(state, topic, items, window, today, &mut fetched, deadline).await?;
        dropped += (proposed - items.len()) as i32;
    }
    let destinations = travel::destinations(&moments);

    let mut articles = HashMap::new();
    if is_reads {
        let available =
            (reads::DAILY_LIMIT - reads::daily_count(state, today).await?).max(0) as usize;
        let limit = reads::ROUND_LIMIT
            .min(topic.max_items as usize)
            .min(available);
        let mut prepared = Vec::new();
        let mut skipped = Vec::new();
        for mut item in items {
            if prepared.len() == limit {
                break;
            }
            // Leave time to publish the successful articles even if later sources are slow.
            let remaining = state
                .config
                .fetch_timeout
                .saturating_sub(started.elapsed() + Duration::from_secs(15));
            if remaining < Duration::from_secs(10) {
                break;
            }
            let deadline = tokio::time::Instant::now() + remaining.min(Duration::from_secs(600));
            let result = tokio::time::timeout_at(
                deadline,
                reads::prepare(state, topic, &mut item, &mut fetched.usage, deadline),
            )
            .await
            .unwrap_or_else(|_| Err(AppError::Internal("Reads 单篇处理超时".into())));
            match result {
                Ok(Some(article)) => {
                    articles.insert(item.fingerprint.clone(), article);
                    prepared.push(item);
                }
                Ok(None) => {
                    dropped += 1;
                }
                Err(error) => {
                    tracing::warn!(url = %item.url, %error, "Reads candidate skipped");
                    skipped.push(serde_json::json!({"url": item.url, "reason": error.to_string()}));
                    dropped += 1;
                }
            }
        }
        items = prepared;
        fetched.payload["skipped"] = serde_json::json!(skipped);
        fetched.payload["cover"] = Value::Null;
    }

    let mut images = std::iter::once(media::stored_image(&fetched.payload["cover"]))
        .chain(items.iter_mut().map(|item| item.image.take()))
        .collect::<Vec<_>>();
    media::mirror_images(state, &mut images).await;
    let mut images = images.into_iter();
    fetched.payload["cover"] = serde_json::to_value(images.next().flatten())?;
    for (item, image) in items.iter_mut().zip(images) {
        item.image = image;
    }
    if is_catalog {
        let remaining = state
            .config
            .fetch_timeout
            .saturating_sub(started.elapsed() + Duration::from_secs(30));
        let deadline = tokio::time::Instant::now() + remaining.min(Duration::from_secs(120));
        crate::catalog::prepare_images(state, topic, &mut items, &fetched.payload, deadline).await;
    }
    items.retain(|item| {
        let keep = (topic.id != "p" && !is_gallery && !is_travel) || item.image.is_some();
        if !keep {
            dropped += 1;
        }
        keep
    });

    // A long read may finish after midnight; charge its publication to the new day.
    let today = if is_reads {
        state.digest.today()
    } else {
        today
    };
    let (new, conflicts, capped) = persist_items(
        state,
        topic,
        run_id,
        today,
        items,
        articles,
        moments,
        duplicate_fingerprints,
    )
    .await?;
    if is_travel {
        // The moments go out before their destinations' guides, which take minutes to write.
        if new > 0
            && let Err(error) = state.digest.refresh(state, &topic.id, today).await
        {
            tracing::warn!(%error, "travel feed refresh before guides failed");
        }
        let deadline = tokio::time::Instant::now()
            + state
                .config
                .fetch_timeout
                .saturating_sub(started.elapsed() + Duration::from_secs(30));
        fetched.payload["guides"] =
            travel::guides(state, topic, &destinations, &mut fetched.usage, deadline).await;
    }
    if let Some(section) = catalog {
        // Publish headlines before the slower, source-based guides.
        if new > 0 {
            state.digest.refresh(state, &topic.id, today).await?;
        }
        let deadline = tokio::time::Instant::now()
            + state
                .config
                .fetch_timeout
                .saturating_sub(started.elapsed() + Duration::from_secs(30));
        let enrichment = async {
            crate::catalog::attach(state, topic, section, run_id, &fetched.payload).await?;
            crate::catalog::guides(state, topic, section, &mut fetched.usage, deadline).await
        }
        .await;
        match enrichment {
            Ok(guides) => fetched.payload["guides"] = serde_json::json!(guides),
            Err(error) => {
                tracing::warn!(%error, "catalogue guides deferred");
                fetched.payload["guide_error"] = serde_json::json!(error.to_string());
            }
        }
    }
    Ok(Completed {
        response_id: fetched.response_id,
        usage: fetched.usage,
        payload: fetched.payload,
        day: today,
        returned,
        new,
        dup: dup + conflicts,
        dropped: dropped + capped,
    })
}

/// All network work finishes before this transaction. Reads metadata and its body, and a
/// travel moment and its photographs, publish together; the topic lock enforces the Reads
/// daily cap across concurrent workers.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_items(
    state: &AppState,
    topic: &Topic,
    run_id: Uuid,
    today: NaiveDate,
    items: Vec<fetch::NewItem>,
    mut articles: HashMap<Vec<u8>, reads::Article>,
    mut moments: HashMap<Vec<u8>, travel::Moment>,
    mut duplicate_fingerprints: Vec<Vec<u8>>,
) -> Result<(i32, i32, i32)> {
    let is_reads = topic.id == "reads" && topic.owner_user_id.is_none();
    let yesterday = today.pred_opt().expect("date in range");
    let (mut new, mut dup, mut dropped) = (0, 0, 0);
    let mut tx = state.pool.begin().await?;
    let mut available = i64::MAX;
    if is_reads {
        sqlx::query("SELECT id FROM topics WHERE id = 'reads' FOR UPDATE")
            .execute(&mut *tx)
            .await?;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE topic_id = 'reads' AND day = $1")
                .bind(today)
                .fetch_one(&mut *tx)
                .await?;
        available = (reads::DAILY_LIMIT - count).max(0);
    }
    for item in items {
        if available == 0 {
            dropped += 1;
            continue;
        }
        let id = Uuid::new_v4();
        let inserted = sqlx::query(
            "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, title_en, summary_en, url, source, published_at, tags, tags_en, importance, first_run_id, image) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16) ON CONFLICT DO NOTHING",
        )
        .bind(id)
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
        .bind(&item.tags_en)
        .bind(item.importance)
        .bind(run_id)
        .bind(item.image.map(serde_json::to_value).transpose()?)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if inserted == 1 {
            if is_reads {
                let article = articles
                    .remove(&item.fingerprint)
                    .ok_or_else(|| AppError::Internal("Reads 缺少正文，取消发布".into()))?;
                reads::insert(&mut tx, id, &item.fingerprint, article).await?;
                available -= 1;
            }
            if let Some(moment) = moments.remove(&item.fingerprint) {
                travel::insert(&mut tx, id, moment).await?;
            }
            new += 1;
        } else {
            dup += 1;
            duplicate_fingerprints.push(item.fingerprint);
        }
    }
    // Apply duplicate sightings after inserts so a repeated story in this same response
    // updates the row that was just inserted, matching the previous per-item ordering.
    for fingerprint in duplicate_fingerprints {
        sqlx::query(
            "UPDATE items SET last_seen_at = NOW(), seen_count = seen_count + 1 WHERE topic_id = $1 AND ($4 OR day >= $2) AND fingerprint = $3",
        )
        .bind(&topic.id)
        .bind(yesterday)
        .bind(fingerprint)
        .bind(is_reads)
        .execute(&mut *tx)
        .await?;
    }
    if new > 0 {
        sqlx::query("UPDATE topics SET feed_revision = feed_revision + 1 WHERE id = $1")
            .bind(&topic.id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok((new, dup, dropped))
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
            // 𝑳𝑳𝑴 changes live outside the news snapshots.
            let news = topic_id != crate::llm::TOPIC_ID;
            ("ok", (news && done.new > 0).then_some(done.day))
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
    if public_topic
        && topic.enabled
        && let Some(day) = refreshed_day
    {
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

/// One serial worker owns all media refreshes. Slow providers cannot stall the
/// minute maintenance loop or overlap another refresh from this process.
async fn media_refresh_loop(state: AppState) {
    let mut interval = tokio::time::interval(HOUSEKEEPING_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        if let Err(error) = media::startup_sync(&state).await {
            tracing::warn!(%error, "news image refresh deferred");
        }
        if let Err(error) = reads::refresh_images(&state).await {
            tracing::warn!(%error, "Reads image refresh deferred");
        }
    }
}

/// Lightweight maintenance and generation claims run every minute. Housekeeping
/// uses elapsed time so a skipped tick cannot postpone cleanup indefinitely.
async fn maintenance_loop(state: AppState) {
    let mut interval = tokio::time::interval(MAINTENANCE_TICK);
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut day = state.digest.today();
    let mut next_cleanup = Instant::now();
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        let today = state.digest.today();
        if today != day {
            day = today;
            state.digest.rollover();
        }
        if Instant::now() >= next_cleanup {
            next_cleanup = Instant::now() + HOUSEKEEPING_INTERVAL;
            if let Err(error) = prune(&state, today).await {
                tracing::warn!(%error, "data pruning failed");
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
    let insights = cleanup_insights(state).await;
    // Batches commit one by one, so a failed pass may still have removed the newest row.
    if !matches!(insights, Ok(0)) {
        state.insight.invalidate();
        state.equity.invalidate();
    }
    if let Err(error) = insights {
        tracing::warn!(%error, "AI insight history pruning failed");
        if first_error.is_none() {
            first_error = Some(error);
        }
    }
    if let Err(error) = crate::llm::prune(state).await {
        tracing::warn!(%error, "LLM catalogue pruning failed");
        if first_error.is_none() {
            first_error = Some(error);
        }
    }
    for topic in [gallery::TOPIC_ID, travel::TOPIC_ID] {
        if let Err(error) = media::prune_lapsed(state, topic).await {
            tracing::warn!(%error, topic, "photograph pruning failed");
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
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

async fn cleanup_runs(state: &AppState) -> Result<u64> {
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

async fn cleanup_insights(state: &AppState) -> Result<u64> {
    cleanup_in_batches(
        state,
        "ai_insights",
        r#"
        WITH expired AS (
            SELECT id
            FROM ai_insights
            WHERE status <> 'running'
              AND started_at < NOW() - (CASE WHEN kind = 'equity' THEN 365 ELSE $2 END * INTERVAL '1 day')
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

pub(crate) async fn cleanup_items(state: &AppState, keep_from: NaiveDate) -> Result<()> {
    let mut total = 0_u64;
    for batch_index in 0..CLEANUP_MAX_BATCHES {
        let deleted = sqlx::query(
            r#"
            WITH expired AS (
                SELECT id
                FROM items
                WHERE day < $2 AND topic_id <> 'reads'
                  AND NOT EXISTS (SELECT 1 FROM catalog_items c WHERE c.item_id = items.id AND c.role <> 'news')
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
/// Returns how many rows went.
async fn cleanup_in_batches(
    state: &AppState,
    table: &'static str,
    statement: &'static str,
    cutoff_days: i64,
) -> Result<u64> {
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
            return Ok(total);
        }
        if batch_index + 1 == CLEANUP_MAX_BATCHES {
            tracing::warn!(
                table,
                total,
                "cleanup reached its per-run limit; remaining rows are deferred"
            );
        }
    }
    Ok(total)
}
