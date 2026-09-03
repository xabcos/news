use crate::{
    email::send_login_email,
    error::{ApiJson, AppError, Result},
    state::AppState,
};
use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, HeaderValue, header, request::Parts},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const RESEND_COOLDOWN_SECONDS: i64 = 60;
const RATE_WINDOW_SECONDS: i64 = 10 * 60;
const RATE_WINDOW_MAX: i64 = 3;
const GLOBAL_RATE_WINDOW_SECONDS: i64 = 60;
const GLOBAL_RATE_WINDOW_MAX: i64 = 120;
const LOGIN_GLOBAL_LOCK_ID: i64 = 0x584e_4557_534c_4f43; // "XNEWSLOC"
const CLEANUP_BATCH_SIZE: i64 = 500;
const CLEANUP_MAX_BATCHES: usize = 20;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct User {
    id: i64,
    email: String,
    status: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartEmailLoginRequest {
    email: String,
}

// Debug is deliberately not derived on the two types below: it is unused, and both carry
// live credentials (a single-use login token, and the dev-mode URL containing it). A
// stray `?request` in a tracing call would put them straight into the logs.
#[derive(Serialize)]
pub(crate) struct StartEmailLoginResponse {
    ok: bool,
    email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    login_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConsumeEmailLoginRequest {
    token: String,
}

#[derive(Serialize)]
struct PublicUser<'a> {
    email: &'a str,
}

impl FromRequestParts<AppState> for User {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        authenticate(state, &parts.headers).await
    }
}

pub(crate) async fn start_email_login(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<StartEmailLoginRequest>,
) -> Result<Json<StartEmailLoginResponse>> {
    let email = normalize_email(&request.email)?;
    if !state.config.allowed_emails.contains(&email) {
        return Err(AppError::Forbidden("此邮箱没有管理权限".into()));
    }
    let login_url = create_login_link(&state, &email).await?;

    Ok(Json(StartEmailLoginResponse {
        ok: true,
        email,
        login_url,
    }))
}

pub(crate) async fn consume_email_login(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<ConsumeEmailLoginRequest>,
) -> Result<Response> {
    let token = request.token.trim();
    if token.is_empty() || token.len() > 256 {
        return Err(AppError::Unauthorized("登录链接无效或已失效".into()));
    }

    let session_token = new_token();
    let session_expires_at = Utc::now()
        + ChronoDuration::from_std(state.config.session_ttl)
            .map_err(|_| AppError::Internal("会话有效期无效".into()))?;
    let mut transaction = state.pool.begin().await?;

    let email = sqlx::query_scalar::<_, String>(
        r#"
        UPDATE email_login_tokens
        SET used_at = NOW()
        WHERE token_hash = $1
          AND used_at IS NULL
          AND expires_at > NOW()
        RETURNING email
        "#,
    )
    .bind(hash_token(token))
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or_else(|| AppError::Unauthorized("登录链接无效或已失效".into()))?;

    let user = sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (email)
        VALUES ($1)
        ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email
        RETURNING id, email, status
        "#,
    )
    .bind(email)
    .fetch_one(&mut *transaction)
    .await?;
    if !user.status {
        // Commit first: the link was single-use and has now been spent. Returning early
        // without committing would roll the `used_at` write back and leave it replayable.
        transaction.commit().await?;
        return Err(AppError::Forbidden("当前账户不可用".into()));
    }

    sqlx::query("INSERT INTO sessions (user_id, token_hash, expires_at) VALUES ($1, $2, $3)")
        .bind(user.id)
        .bind(hash_token(&session_token))
        .bind(session_expires_at)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    let mut response = Json(serde_json::json!({ "user": public_user(&user) })).into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&state, &session_token))
            .map_err(|_| AppError::Internal("会话 Cookie 无效".into()))?,
    );
    Ok(response)
}

pub(crate) async fn me(user: User) -> Result<Json<serde_json::Value>> {
    Ok(Json(serde_json::json!({ "user": public_user(&user) })))
}

fn public_user(user: &User) -> PublicUser<'_> {
    PublicUser { email: &user.email }
}

pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    if let Some(token) = read_session_token(&headers, &state.config.session_cookie_name) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(hash_token(&token))
            .execute(&state.pool)
            .await?;
    }
    let mut response = axum::http::StatusCode::NO_CONTENT.into_response();
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&clear_cookie(&state))
            .map_err(|_| AppError::Internal("会话 Cookie 无效".into()))?,
    );
    Ok(response)
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<User> {
    let token = read_session_token(headers, &state.config.session_cookie_name)
        .ok_or_else(|| AppError::Unauthorized("请先登录".into()))?;
    let user = sqlx::query_as::<_, User>(
        r#"
        SELECT users.id, users.email, users.status
        FROM sessions
        JOIN users ON users.id = sessions.user_id
        WHERE sessions.token_hash = $1
          AND sessions.expires_at > NOW()
        "#,
    )
    .bind(hash_token(&token))
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Unauthorized("登录状态已失效".into()))?;
    if !user.status {
        return Err(AppError::Forbidden("当前账户不可用".into()));
    }
    Ok(user)
}

pub async fn cleanup(state: &AppState) -> Result<()> {
    // Each table has its own batching loop and autocommit statements. A timeout in
    // one table neither rolls back nor prevents a full cleanup pass for the other.
    let session_result = cleanup_expired_sessions(state).await;
    let login_token_result = cleanup_login_tokens(state).await;
    match (session_result, login_token_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(session_error), Err(login_token_error)) => {
            tracing::warn!(error = %login_token_error, "login token cleanup also failed");
            Err(session_error)
        }
    }
}

async fn cleanup_expired_sessions(state: &AppState) -> Result<()> {
    cleanup_in_batches(
        state,
        "sessions",
        r#"
        WITH expired AS (
            SELECT token_hash
            FROM sessions
            WHERE expires_at <= NOW()
            ORDER BY expires_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM sessions
        USING expired
        WHERE sessions.token_hash = expired.token_hash
        "#,
    )
    .await
}

async fn cleanup_login_tokens(state: &AppState) -> Result<()> {
    // Spent tokens are retained briefly so they still count toward the send-rate windows.
    cleanup_in_batches(
        state,
        "email_login_tokens",
        r#"
        WITH expired AS (
            SELECT token_hash
            FROM email_login_tokens
            WHERE (used_at IS NOT NULL OR expires_at <= NOW())
              AND created_at <= NOW() - INTERVAL '10 minutes'
            ORDER BY created_at
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM email_login_tokens
        USING expired
        WHERE email_login_tokens.token_hash = expired.token_hash
        "#,
    )
    .await
}

/// Deletes in bounded batches so a large backlog never holds locks for a whole pass.
async fn cleanup_in_batches(state: &AppState, table: &'static str, statement: &str) -> Result<()> {
    let mut total = 0;
    for batch_index in 0..CLEANUP_MAX_BATCHES {
        let deleted = sqlx::query(statement)
            .bind(CLEANUP_BATCH_SIZE)
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

async fn create_login_link(state: &AppState, email: &str) -> Result<Option<String>> {
    let token = new_token();
    let token_hash = hash_token(&token);
    let expires_at = Utc::now()
        + ChronoDuration::from_std(state.config.email_login_ttl)
            .map_err(|_| AppError::Internal("登录链接有效期无效".into()))?;
    insert_rate_limited_login_token(state, email, &token_hash, expires_at).await?;

    let url = build_login_url(&state.config.public_base_url, &token);
    match send_login_email(state, email, &url).await {
        Ok(true) => {}
        Ok(false) if state.config.email_login_dev_links => {
            tracing::info!(login_url = %url, "development login link generated");
        }
        Ok(false) => {
            expire_undelivered_login_token(state, &token_hash).await;
            return Err(AppError::ServiceUnavailable(
                "邮件服务暂时不可用，请稍后重试".into(),
            ));
        }
        Err(error) => {
            expire_undelivered_login_token(state, &token_hash).await;
            return Err(error);
        }
    }

    Ok(state.config.email_login_dev_links.then_some(url))
}

async fn insert_rate_limited_login_token(
    state: &AppState,
    email: &str,
    token_hash: &[u8],
    expires_at: DateTime<Utc>,
) -> Result<()> {
    let mut transaction = state.pool.begin().await?;
    // One short global critical section makes both global and per-address limits atomic.
    // Do not wait while holding a pool connection: callers can safely retry a busy lock.
    let lock_acquired = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_xact_lock($1)")
        .bind(LOGIN_GLOBAL_LOCK_ID)
        .fetch_one(&mut *transaction)
        .await?;
    if !lock_acquired {
        return Err(AppError::TooManyRequests("登录请求较多，请稍后再试".into()));
    }

    let (cooldown_count, window_count) = sqlx::query_as::<_, (i64, i64)>(
        r#"
        SELECT
            COUNT(*) FILTER (
                WHERE expires_at > NOW()
                  AND created_at > NOW() - ($2::bigint * INTERVAL '1 second')
            ),
            COUNT(*)
        FROM email_login_tokens
        WHERE email = $1
          AND created_at > NOW() - ($3::bigint * INTERVAL '1 second')
        "#,
    )
    .bind(email)
    .bind(RESEND_COOLDOWN_SECONDS)
    .bind(RATE_WINDOW_SECONDS)
    .fetch_one(&mut *transaction)
    .await?;

    // An immediately expired failed attempt does not trigger the short resend
    // cooldown, but still counts in the per-address window and global limit. An
    // unavailable or ambiguously timed-out queue must not enable unbounded retries.
    let global_count = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT COUNT(*)
        FROM email_login_tokens
        WHERE created_at > NOW() - ($1::bigint * INTERVAL '1 second')
        "#,
    )
    .bind(GLOBAL_RATE_WINDOW_SECONDS)
    .fetch_one(&mut *transaction)
    .await?;

    if cooldown_count > 0 {
        return Err(AppError::TooManyRequests("请稍候再发送登录邮件".into()));
    }
    if window_count >= RATE_WINDOW_MAX {
        return Err(AppError::TooManyRequests(
            "登录邮件发送过于频繁，请稍后再试".into(),
        ));
    }
    if global_count >= GLOBAL_RATE_WINDOW_MAX {
        return Err(AppError::TooManyRequests("登录请求较多，请稍后再试".into()));
    }

    // Supersede any link still outstanding for this address, so only the newest works.
    sqlx::query(
        "UPDATE email_login_tokens SET used_at = NOW(), expires_at = NOW() WHERE email = $1 AND used_at IS NULL",
    )
    .bind(email)
    .execute(&mut *transaction)
    .await?;

    sqlx::query(
        "INSERT INTO email_login_tokens (email, token_hash, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(email)
    .bind(token_hash)
    .bind(expires_at)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn expire_undelivered_login_token(state: &AppState, token_hash: &[u8]) {
    if let Err(error) = sqlx::query(
        "UPDATE email_login_tokens SET used_at = NOW(), expires_at = NOW() WHERE token_hash = $1 AND used_at IS NULL",
    )
    .bind(token_hash)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%error, "failed to expire an undelivered login token");
    }
}

fn normalize_email(value: &str) -> Result<String> {
    let email = value.trim().to_ascii_lowercase();
    if !valid_email(&email) {
        return Err(AppError::BadRequest("请输入有效的邮箱地址".into()));
    }
    Ok(email)
}

fn valid_email(email: &str) -> bool {
    if email.is_empty() || email.len() > 254 || !email.is_ascii() {
        return false;
    }
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    if local.is_empty()
        || local.len() > 64
        || domain.is_empty()
        || domain.contains('@')
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
        })
    {
        return false;
    }

    let labels = domain.split('.').collect::<Vec<_>>();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        && labels.last().is_some_and(|label| {
            label.len() >= 2 && label.bytes().any(|byte| byte.is_ascii_alphabetic())
        })
}

fn new_token() -> String {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut *bytes).expect("operating system CSPRNG is available");
    URL_SAFE_NO_PAD.encode(bytes.as_slice())
}

fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn build_login_url(base: &url::Url, token: &str) -> String {
    let mut url = base.clone();
    url.set_path("/admin/");
    url.set_query(None);
    url.set_fragment(Some(&format!("login_token={token}")));
    url.to_string()
}

fn read_session_token(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            (name == cookie_name && !value.is_empty() && value.len() <= 128)
                .then(|| value.to_string())
        })
}

fn session_cookie(state: &AppState, token: &str) -> String {
    let mut cookie = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        state.config.session_cookie_name,
        token,
        state.config.session_ttl.as_secs()
    );
    if state.config.public_base_url.scheme() == "https" {
        cookie.push_str("; Secure");
    }
    cookie
}

fn clear_cookie(state: &AppState) -> String {
    let mut cookie = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        state.config.session_cookie_name
    );
    if state.config.public_base_url.scheme() == "https" {
        cookie.push_str("; Secure");
    }
    cookie
}

#[cfg(test)]
mod tests {
    use super::{
        GLOBAL_RATE_WINDOW_MAX, RATE_WINDOW_MAX, RATE_WINDOW_SECONDS, RESEND_COOLDOWN_SECONDS,
        build_login_url, hash_token, new_token, normalize_email, valid_email,
    };
    use url::Url;

    #[test]
    fn validates_and_normalizes_email_addresses() {
        assert_eq!(
            normalize_email(" User+X@Example.COM ").expect("valid email"),
            "user+x@example.com"
        );
        assert!(valid_email("a@example.com"));
        assert!(!valid_email("a..b@example.com"));
        assert!(!valid_email("user@localhost"));
        assert!(!valid_email("用户@example.com"));
    }

    #[test]
    fn creates_an_unlogged_fragment_login_link() {
        let base = Url::parse("https://news.example.com/").expect("valid URL");
        assert_eq!(
            build_login_url(&base, "abc_123"),
            "https://news.example.com/admin/#login_token=abc_123"
        );
    }

    #[test]
    fn creates_random_tokens_and_fixed_length_hashes() {
        let first = new_token();
        let second = new_token();
        assert_ne!(first, second);
        assert_eq!(first.len(), 43);
        assert_eq!(hash_token(&first).len(), 32);
    }

    #[test]
    fn keeps_send_rate_limits_usable() {
        // A cooldown at or above the window would make the window unreachable, and a
        // per-address allowance above the global one would make the global cap dead code.
        const {
            assert!(RESEND_COOLDOWN_SECONDS > 0);
            assert!(RESEND_COOLDOWN_SECONDS < RATE_WINDOW_SECONDS);
            assert!(RATE_WINDOW_MAX > 0);
            assert!(RATE_WINDOW_MAX < GLOBAL_RATE_WINDOW_MAX);
        }
    }
}
