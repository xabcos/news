use crate::error::{AppError, Result};
use chrono::NaiveTime;
use chrono_tz::Tz;
use sqlx::postgres::PgConnectOptions;
use std::{collections::HashSet, env, net::SocketAddr, path::PathBuf, time::Duration};
use url::{Host, Url};

const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8788";

pub struct Config {
    pub(crate) bind_addr: SocketAddr,
    pub(crate) database_url: String,
    pub(crate) database_max_connections: u32,
    pub(crate) migrations_dir: PathBuf,
    pub(crate) pages_dir: PathBuf,
    pub(crate) public_base_url: Url,
    pub(crate) session_cookie_name: String,
    pub(crate) session_ttl: Duration,
    pub(crate) email_login_ttl: Duration,
    pub(crate) email_login_dev_links: bool,
    /// Emails that receive the administrator role on first login.  Other valid
    /// addresses are regular accounts and can use their own `/me/` space.
    pub(crate) admin_emails: HashSet<String>,
    pub(crate) redis_addr: Option<String>,
    pub(crate) redis_password: Option<String>,
    pub(crate) redis_db: i64,
    pub(crate) email_stream_priority: String,
    pub(crate) email_oid: u64,
    pub(crate) email_reply_to: String,
    pub(crate) ypay_base_url: Option<Url>,
    pub(crate) ypay_tenant_code: String,
    pub(crate) ypay_store_api_key: Option<String>,
    pub(crate) payment_webhook_secret: Option<String>,
    pub(crate) ai_base_url: Url,
    pub(crate) ai_api_key: String,
    pub(crate) ai_default_model: String,
    pub(crate) ai_insight_model: String,
    pub(crate) ai_image_model: String,
    pub(crate) timezone: Tz,
    pub(crate) briefing_time: NaiveTime,
    pub(crate) fetch_timeout: Duration,
    pub(crate) max_concurrent_runs: usize,
    pub(crate) scheduler_enabled: bool,
    pub(crate) retention_days: u32,
    pub(crate) tmpfile_base_url: Url,
    pub(crate) tmpfile_api_key: String,
    pub(crate) image_max_bytes: u64,
    pub(crate) image_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bind_addr = env_value("BIND_ADDR", DEFAULT_BIND_ADDR)
            .parse()
            .map_err(|_| AppError::Config("BIND_ADDR 必须是有效的监听地址".into()))?;
        let database_url = required_env("DATABASE_URL")?;
        database_url.parse::<PgConnectOptions>().map_err(|_| {
            AppError::Config("DATABASE_URL 必须是有效的 PostgreSQL 连接地址".into())
        })?;
        let database_max_connections = parse_env("DATABASE_MAX_CONNECTIONS", 20_u32)?;
        if !(1..=200).contains(&database_max_connections) {
            return Err(AppError::Config(
                "DATABASE_MAX_CONNECTIONS 必须在 1 到 200 之间".into(),
            ));
        }

        let public_base_url = parse_public_base_url(&required_env("PUBLIC_BASE_URL")?)?;
        let session_cookie_name = env_value("SESSION_COOKIE_NAME", "news_session");
        validate_cookie_name(&session_cookie_name)?;
        let session_days = parse_env("SESSION_TTL_DAYS", 30_u64)?;
        let login_minutes = parse_env("EMAIL_LOGIN_LINK_TTL_MINUTES", 30_u64)?;
        if !(1..=3650).contains(&session_days) {
            return Err(AppError::Config(
                "SESSION_TTL_DAYS 必须在 1 到 3650 之间".into(),
            ));
        }
        if !(5..=1440).contains(&login_minutes) {
            return Err(AppError::Config(
                "EMAIL_LOGIN_LINK_TTL_MINUTES 必须在 5 到 1440 之间".into(),
            ));
        }

        let email_login_dev_links = bool_env("EMAIL_LOGIN_DEV_LINKS", false)?;
        if email_login_dev_links && !is_loopback_url(&public_base_url) {
            return Err(AppError::Config(
                "EMAIL_LOGIN_DEV_LINKS 只能用于 localhost 或回环地址".into(),
            ));
        }
        let admin_emails = env_value("NEWS_ADMIN_EMAILS", "")
            .split(',')
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .map(|email| email.to_ascii_lowercase())
            .collect::<HashSet<_>>();
        if admin_emails.is_empty() {
            return Err(AppError::Config(
                "NEWS_ADMIN_EMAILS 至少需要一个邮箱".into(),
            ));
        }

        let redis_addr = optional_env("REDIS_ADDR");
        if redis_addr.is_none() && !email_login_dev_links {
            return Err(AppError::Config(
                "生产模式必须配置 REDIS_ADDR 以发送登录邮件".into(),
            ));
        }
        let redis_db = parse_env("REDIS_DB", 1_i64)?;
        if redis_db < 0 {
            return Err(AppError::Config("REDIS_DB 不能为负数".into()));
        }
        let email_stream_priority = env_value("EMAIL_STREAM_PRIORITY", "important");
        if !matches!(
            email_stream_priority.as_str(),
            "critical" | "important" | "default"
        ) {
            return Err(AppError::Config(
                "EMAIL_STREAM_PRIORITY 必须是 critical、important 或 default".into(),
            ));
        }

        let ypay_base_url = optional_env("YPAY_BASE_URL")
            .map(|value| parse_http_url("YPAY_BASE_URL", &value))
            .transpose()?;
        let ypay_store_api_key = optional_env("YPAY_STORE_API_KEY");
        let payment_webhook_secret = optional_env("PAYMENT_WEBHOOK_SECRET");
        let any_ypay = ypay_base_url.is_some()
            || ypay_store_api_key.is_some()
            || payment_webhook_secret.is_some();
        if any_ypay
            && (ypay_base_url.is_none()
                || ypay_store_api_key.is_none()
                || payment_webhook_secret.is_none())
        {
            return Err(AppError::Config(
                "YPAY_BASE_URL、YPAY_STORE_API_KEY、PAYMENT_WEBHOOK_SECRET 必须同时配置".into(),
            ));
        }
        let ypay_tenant_code = env_value("YPAY_TENANT_CODE", "news");
        validate_tenant_code(&ypay_tenant_code)?;

        let scheduler_enabled = bool_env("NEWS_SCHEDULER_ENABLED", true)?;
        let ai_api_key = optional_env("AI_API_KEY").unwrap_or_default();
        if scheduler_enabled && ai_api_key.is_empty() {
            return Err(AppError::Config(
                "启用调度时必须配置 AI_API_KEY（或设置 NEWS_SCHEDULER_ENABLED=false）".into(),
            ));
        }
        let ai_base_url = Url::parse(&env_value("AI_BASE_URL", "https://api.zairouter.com"))
            .map_err(|_| AppError::Config("AI_BASE_URL 必须是有效的 URL".into()))?;
        let timezone = env_value("NEWS_TIMEZONE", "Asia/Shanghai")
            .parse::<Tz>()
            .map_err(|_| AppError::Config("NEWS_TIMEZONE 必须是有效的 IANA 时区".into()))?;
        let briefing_time =
            NaiveTime::parse_from_str(&env_value("NEWS_BRIEFING_TIME", "06:00"), "%H:%M")
                .map_err(|_| AppError::Config("NEWS_BRIEFING_TIME 必须是 HH:MM 格式".into()))?;
        let fetch_timeout_secs = parse_env("NEWS_FETCH_TIMEOUT_SECS", 1800_u64)?;
        if !(30..=1800).contains(&fetch_timeout_secs) {
            return Err(AppError::Config(
                "NEWS_FETCH_TIMEOUT_SECS 必须在 30 到 1800 之间".into(),
            ));
        }
        let max_concurrent_runs = parse_env("NEWS_MAX_CONCURRENT_RUNS", 2_usize)?;
        if !(1..=8).contains(&max_concurrent_runs) {
            return Err(AppError::Config(
                "NEWS_MAX_CONCURRENT_RUNS 必须在 1 到 8 之间".into(),
            ));
        }

        // NEWS always serves media through tmpfile so browser-visible assets stay stable.
        let tmpfile_base_url =
            parse_http_url("TMPFILE_BASE_URL", &required_env("TMPFILE_BASE_URL")?)?;
        let tmpfile_api_key = required_env("TMPFILE_API_KEY")?;
        let image_max_bytes = parse_env("NEWS_IMAGE_MAX_BYTES", 8_u64 * 1024 * 1024)?;
        if !(64 * 1024..=64 * 1024 * 1024).contains(&image_max_bytes) {
            return Err(AppError::Config(
                "NEWS_IMAGE_MAX_BYTES 必须在 65536 到 67108864 之间".into(),
            ));
        }
        let image_timeout_secs = parse_env("NEWS_IMAGE_TIMEOUT_SECS", 30_u64)?;
        if !(5..=600).contains(&image_timeout_secs) {
            return Err(AppError::Config(
                "NEWS_IMAGE_TIMEOUT_SECS 必须在 5 到 600 秒之间".into(),
            ));
        }

        Ok(Self {
            bind_addr,
            database_url,
            database_max_connections,
            migrations_dir: PathBuf::from(env_value("MIGRATIONS_DIR", "migrations")),
            pages_dir: PathBuf::from(env_value("PAGES_DIR", "pages")),
            public_base_url,
            session_cookie_name,
            session_ttl: Duration::from_secs(session_days * 24 * 60 * 60),
            email_login_ttl: Duration::from_secs(login_minutes * 60),
            email_login_dev_links,
            admin_emails,
            redis_addr,
            redis_password: optional_env("REDIS_PASS"),
            redis_db,
            email_stream_priority,
            email_oid: parse_env("EMAIL_OID", 0_u64)?,
            email_reply_to: env_value("EMAIL_REPLY_TO", ""),
            ypay_base_url,
            ypay_tenant_code,
            ypay_store_api_key,
            payment_webhook_secret,
            ai_base_url,
            ai_api_key,
            ai_default_model: env_value("AI_DEFAULT_MODEL", "grok-4.7"),
            ai_insight_model: env_value("AI_INSIGHT_MODEL", "gpt-6-astra"),
            ai_image_model: env_value("AI_IMAGE_MODEL", "gpt-image-2.5-flare"),
            timezone,
            briefing_time,
            fetch_timeout: Duration::from_secs(fetch_timeout_secs),
            max_concurrent_runs,
            scheduler_enabled,
            retention_days: parse_env("NEWS_RETENTION_DAYS", 128_u32)?,
            tmpfile_base_url,
            tmpfile_api_key,
            image_max_bytes,
            image_timeout: Duration::from_secs(image_timeout_secs),
        })
    }
}

fn parse_http_url(name: &str, value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|_| AppError::Config(format!("{name} 必须是有效的 HTTP(S) URL").into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::Config(
            format!("{name} 必须是无认证信息的 HTTP(S) URL").into(),
        ));
    }
    Ok(url)
}

fn validate_tenant_code(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::Config(
            "YPAY_TENANT_CODE 只能包含字母、数字、连字符和下划线".into(),
        ));
    }
    Ok(())
}

fn parse_public_base_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value)
        .map_err(|_| AppError::Config("PUBLIC_BASE_URL 必须是有效的 HTTP(S) URL".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (!url.path().is_empty() && url.path() != "/")
    {
        return Err(AppError::Config(
            "PUBLIC_BASE_URL 必须是站点根目录的 HTTP(S) URL".into(),
        ));
    }
    if url.scheme() == "http" && !is_loopback_url(&url) {
        return Err(AppError::Config(
            "公开服务必须使用 HTTPS（回环地址除外）".into(),
        ));
    }
    // `https://example.com` and `https://example.com/` are the same site. Keep
    // one canonical root form for login links and origin comparisons.
    if url.path().is_empty() {
        url.set_path("/");
    }
    Ok(url)
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn validate_cookie_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AppError::Config(
            "SESSION_COOKIE_NAME 只能包含字母、数字、下划线和连字符".into(),
        ));
    }
    Ok(())
}

fn required_env(name: &str) -> Result<String> {
    optional_env(name).ok_or_else(|| AppError::Config(format!("缺少环境变量 {name}").into()))
}

fn optional_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_value(name: &str, default: &str) -> String {
    optional_env(name).unwrap_or_else(|| default.to_string())
}

fn parse_env<T>(name: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
{
    match optional_env(name) {
        Some(value) => value
            .parse()
            .map_err(|_| AppError::Config(format!("环境变量 {name} 的值无效").into())),
        None => Ok(default),
    }
}

fn bool_env(name: &str, default: bool) -> Result<bool> {
    match optional_env(name) {
        None => Ok(default),
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            _ => Err(AppError::Config(
                format!("环境变量 {name} 必须是 true 或 false").into(),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_public_base_url, validate_cookie_name};

    #[test]
    fn accepts_only_root_http_urls() {
        assert!(parse_public_base_url("https://news.example.com/").is_ok());
        assert!(parse_public_base_url("https://news.example.com").is_ok());
        assert!(parse_public_base_url("http://127.0.0.1:8788/").is_ok());
        assert!(parse_public_base_url("https://news.example.com/app").is_err());
        assert!(parse_public_base_url("http://news.example.com/").is_err());
    }

    #[test]
    fn validates_cookie_names() {
        assert!(validate_cookie_name("news_session").is_ok());
        assert!(validate_cookie_name("bad cookie").is_err());
    }
}
