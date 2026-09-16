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
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use tokio::sync::Semaphore;
use url::Url;

const DATABASE_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const DATABASE_STATEMENT_TIMEOUT_SECONDS: u64 = 10;
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REDIS_CONNECTION_TIMEOUT: Duration = Duration::from_secs(3);
const REDIS_RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
const REDIS_MAX_RECONNECT_DELAY: Duration = Duration::from_secs(1);
const REDIS_CONNECTION_RETRIES: usize = 3;

#[derive(Clone)]
pub struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) redis: Option<ConnectionManager>,
    pub(crate) http: reqwest::Client,
    pub(crate) image_http: reqwest::Client,
    pub(crate) config: Arc<Config>,
    pub(crate) digest: DigestCache,
    pub(crate) insight: InsightCache,
    /// Topics with a fetch in flight; one run per topic at a time.
    in_flight: Arc<Mutex<HashSet<String>>>,
    /// One process-wide budget shared by scheduled and manual runs.
    pub(crate) run_slots: Arc<Semaphore>,
    /// Autonomous travel companion work has its own bounded budget so it cannot starve NEWS.
    pub(crate) travel_slots: Arc<Semaphore>,
    /// Image generation is even more expensive than text; keep it isolated
    /// from both NEWS collection and the autonomous planner.
    pub(crate) travel_image_slots: Arc<Semaphore>,
    /// A settlement worker may wait on this slot without blocking the minute
    /// maintenance loop itself.
    pub(crate) travel_settle_slots: Arc<Semaphore>,
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
                    sqlx::query(&format!(
                        "SET statement_timeout = '{DATABASE_STATEMENT_TIMEOUT_SECONDS}s'"
                    ))
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
            config: Arc::new(config),
            in_flight: Arc::default(),
            run_slots: Arc::new(Semaphore::new(max_concurrent_runs)),
            travel_slots: Arc::new(Semaphore::new(2)),
            travel_image_slots: Arc::new(Semaphore::new(1)),
            travel_settle_slots: Arc::new(Semaphore::new(1)),
        };
        Ok(state)
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
    use super::redis_url;

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
}
