use crate::{
    config::Config,
    digest::DigestCache,
    error::{AppError, Result},
    insight::InsightCache,
};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    collections::HashSet,
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use url::Url;

const DATABASE_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIS_CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);
const REDIS_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
const REDIS_MAX_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const REDIS_CONNECTION_RETRIES: usize = 3;
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Process-local request counters. They intentionally avoid endpoint labels so a malformed
/// path cannot grow an unbounded metrics map; the admin endpoint exposes the aggregate view.
#[derive(Default)]
pub(crate) struct Metrics {
    active_requests: AtomicU64,
    total_requests: AtomicU64,
    error_requests: AtomicU64,
    total_duration_us: AtomicU64,
    max_duration_us: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MetricsSnapshot {
    pub(crate) active_requests: u64,
    pub(crate) total_requests: u64,
    pub(crate) error_requests: u64,
    pub(crate) total_duration_us: u64,
    pub(crate) max_duration_us: u64,
}

impl Metrics {
    pub(crate) fn request_started(&self) {
        self.active_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn request_finished(&self, status: u16, elapsed: Duration) {
        self.active_requests.fetch_sub(1, Ordering::Relaxed);
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if status >= 400 {
            self.error_requests.fetch_add(1, Ordering::Relaxed);
        }
        let duration_us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        self.total_duration_us
            .fetch_add(duration_us, Ordering::Relaxed);
        self.max_duration_us
            .fetch_max(duration_us, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            active_requests: self.active_requests.load(Ordering::Relaxed),
            total_requests: self.total_requests.load(Ordering::Relaxed),
            error_requests: self.error_requests.load(Ordering::Relaxed),
            total_duration_us: self.total_duration_us.load(Ordering::Relaxed),
            max_duration_us: self.max_duration_us.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct WorkerGroupState {
    closed: bool,
    handles: Vec<JoinHandle<()>>,
}

/// Tracks detached application workers and gives them a bounded shutdown window. Finished
/// handles are discarded when new work is registered, so a busy service does not accumulate
/// one join handle per historical run.
#[derive(Default)]
pub(crate) struct WorkerGroup {
    state: Mutex<WorkerGroupState>,
}

impl WorkerGroup {
    fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.handles.retain(|handle| !handle.is_finished());
        if state.closed {
            handle.abort();
        } else {
            state.handles.push(handle);
        }
    }

    async fn shutdown(&self) {
        let handles = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.closed = true;
            std::mem::take(&mut state.handles)
        };
        let started = tokio::time::Instant::now();
        while started.elapsed() < WORKER_SHUTDOWN_TIMEOUT
            && handles.iter().any(|handle| !handle.is_finished())
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        for handle in handles {
            if !handle.is_finished() {
                handle.abort();
            }
            let _ = handle.await;
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) redis: Option<ConnectionManager>,
    pub(crate) http: reqwest::Client,
    pub(crate) image_http: reqwest::Client,
    pub(crate) config: Arc<Config>,
    pub(crate) digest: DigestCache,
    pub(crate) insight: InsightCache,
    pub(crate) equity: InsightCache,
    /// Topics with a fetch in flight; one run per topic at a time.
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// One process-wide budget shared by scheduled and manual runs.
    pub(crate) run_slots: Arc<Semaphore>,
    pub(crate) image_slots: Arc<Semaphore>,
    /// Cancels scheduler and detached workers before process shutdown.
    pub(crate) shutdown: CancellationToken,
    workers: Arc<WorkerGroup>,
    pub(crate) metrics: Arc<Metrics>,
}

impl AppState {
    pub async fn new(config: Config) -> Result<Self> {
        run_migrations(&config).await?;

        let pool = PgPoolOptions::new()
            .max_connections(config.database_max_connections)
            .acquire_timeout(DATABASE_ACQUIRE_TIMEOUT)
            .after_connect(|connection, _metadata| {
                Box::pin(async move {
                    // Runtime queries are bounded at the database. Migrations deliberately
                    // use a separate pool below and are not subject to this short timeout.
                    sqlx::query("SET statement_timeout = '10s'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&config.database_url)
            .await?;

        let redis = connect_redis(&config).await?;
        let http = reqwest::Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(config.fetch_timeout)
            .build()?;
        let image_http = reqwest::Client::builder()
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .timeout(config.image_timeout)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5
                    || !crate::media::is_public_https_url(attempt.url())
                {
                    attempt.stop()
                } else {
                    attempt.follow()
                }
            }))
            .build()?;
        let max_concurrent_runs = config.max_concurrent_runs;
        let state = Self {
            pool,
            redis,
            http,
            image_http,
            digest: DigestCache::new(config.timezone),
            insight: InsightCache::default(),
            equity: InsightCache::default(),
            config: Arc::new(config),
            in_flight: Arc::default(),
            run_slots: Arc::new(Semaphore::new(max_concurrent_runs)),
            image_slots: Arc::new(Semaphore::new(3)),
            shutdown: CancellationToken::new(),
            workers: Arc::new(WorkerGroup::default()),
            metrics: Arc::new(Metrics::default()),
        };
        state.digest.reload_topics(&state).await?;
        Ok(state)
    }

    pub(crate) fn spawn_worker<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.workers.spawn(future);
    }

    pub(crate) async fn shutdown_workers(&self) {
        self.workers.shutdown().await;
    }

    pub(crate) fn in_flight(&self) -> MutexGuard<'_, HashSet<String>> {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

async fn run_migrations(config: &Config) -> Result<()> {
    let migrator = sqlx::migrate::Migrator::new(config.migrations_dir.clone()).await?;
    // Schema changes can legitimately exceed the runtime statement timeout. A dedicated
    // one-connection pool also prevents migrations from consuming application capacity.
    let migration_pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(DATABASE_ACQUIRE_TIMEOUT)
        .connect(&config.database_url)
        .await?;
    let result = migrator.run(&migration_pool).await;
    migration_pool.close().await;
    result?;
    Ok(())
}

async fn connect_redis(config: &Config) -> Result<Option<ConnectionManager>> {
    let Some(addr) = config.redis_addr.as_deref() else {
        return Ok(None);
    };
    let url = redis_url(addr, config.redis_password.as_deref(), config.redis_db)?;
    let client = redis::Client::open(url)?;
    let manager = ConnectionManager::new_with_config(client, redis_manager_config()).await?;
    Ok(Some(manager))
}

fn redis_manager_config() -> ConnectionManagerConfig {
    ConnectionManagerConfig::new()
        .set_connection_timeout(Some(REDIS_CONNECTION_TIMEOUT))
        .set_response_timeout(Some(REDIS_RESPONSE_TIMEOUT))
        .set_max_delay(REDIS_MAX_RECONNECT_DELAY)
        .set_number_of_retries(REDIS_CONNECTION_RETRIES)
}

fn redis_url(addr: &str, password: Option<&str>, db: i64) -> Result<String> {
    let raw = if addr.starts_with("redis://") || addr.starts_with("rediss://") {
        addr.to_string()
    } else {
        format!("redis://{addr}")
    };
    let mut url = Url::parse(&raw)
        .map_err(|_| AppError::Config("REDIS_ADDR 必须是有效的 Redis 地址".into()))?;
    if !matches!(url.scheme(), "redis" | "rediss") || url.host_str().is_none() {
        return Err(AppError::Config(
            "REDIS_ADDR 必须使用 redis:// 或 rediss://".into(),
        ));
    }
    if let Some(password) = password {
        url.set_password(Some(password))
            .map_err(|_| AppError::Config("REDIS_PASS 无法写入 Redis 地址".into()))?;
    }
    if url.path().is_empty() || url.path() == "/" {
        url.set_path(&format!("/{db}"));
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::{Metrics, redis_url};
    use std::time::Duration;

    #[test]
    fn builds_redis_urls_and_escapes_passwords() {
        assert_eq!(
            redis_url("cache:6379", None, 1).expect("valid URL"),
            "redis://cache:6379/1"
        );
        assert_eq!(
            redis_url("cache:6379", Some("s@fe/pass"), 2).expect("valid URL"),
            "redis://:s%40fe%2Fpass@cache:6379/2"
        );
    }

    #[test]
    fn records_request_counts_and_latency_bounds() {
        let metrics = Metrics::default();
        metrics.request_started();
        metrics.request_finished(200, Duration::from_millis(2));
        metrics.request_started();
        metrics.request_finished(503, Duration::from_millis(5));
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.active_requests, 0);
        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.error_requests, 1);
        assert!(snapshot.total_duration_us >= 7_000);
        assert!(snapshot.max_duration_us >= 5_000);
    }
}
