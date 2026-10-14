use crate::{
    email::send_login_email,
    error::{ApiJson, AppError, Result},
    runner::{CLEANUP_BATCH_SIZE, delete_in_batches},
    state::AppState,
};
use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, HeaderValue, header, request::Parts},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration as ChronoDuration, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const RESEND_COOLDOWN_SECONDS: i64 = 60;
const RATE_WINDOW_SECONDS: i64 = 10 * 60;
const RATE_WINDOW_MAX: i64 = 3;
const GLOBAL_RATE_WINDOW_SECONDS: i64 = 60;
const GLOBAL_RATE_WINDOW_MAX: i64 = 120;
const LOGIN_GLOBAL_LOCK_ID: i64 = 0x584e_4557_534c_4f43; // "XNEWSLOC"
/// Set beside the session cookie, but readable by the public page's script. It only decides
/// which feed the page asks for first; that request is authorized by the session as usual.
const ROLE_COOKIE_NAME: &str = "news_role";

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct User {
    pub(crate) id: i64,
    pub(crate) email: String,
    pub(crate) status: bool,
    pub(crate) role: String,
    pub(crate) pro_until: Option<DateTime<Utc>>,
}

/// The deliberately small plan surface used by both the API and scheduler.
/// A plan is effective while `pro_until` is in the future; there is no separate
/// subscription state to keep in sync.
#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct PlanLimits {
    pub(crate) max_topics: i64,
    pub(crate) min_interval_minutes: i32,
    pub(crate) daily_run_cap: i32,
}

pub(crate) const FREE_LIMITS: PlanLimits = PlanLimits {
    max_topics: 1,
    min_interval_minutes: 512,
    daily_run_cap: 12,
};
pub(crate) const PRO_LIMITS: PlanLimits = PlanLimits {
    max_topics: 4,
    min_interval_minutes: 64,
    daily_run_cap: 48,
};

impl User {
    pub(crate) fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    fn is_pro(&self) -> bool {
        self.pro_until.is_some_and(|until| until > Utc::now())
    }

    /// Creating personal fetch tasks requires Pro; execution follows plan limits.
    pub(crate) fn can_create_personal_topic(&self) -> bool {
        self.is_pro()
    }

    pub(crate) fn plan(&self) -> &'static str {
        if self.is_pro() { "pro" } else { "free" }
    }

    pub(crate) fn limits(&self) -> PlanLimits {
        if self.is_pro() {
            PRO_LIMITS
        } else {
            FREE_LIMITS
        }
    }
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
pub(crate) struct EmailLoginToken {
    token: String,
}

#[derive(Serialize)]
struct PublicUser<'a> {
    id: i64,
    email: &'a str,
    role: &'a str,
    plan: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pro_until: Option<DateTime<Utc>>,
    limits: PlanLimits,
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
    let login_url = create_login_link(&state, &email).await?;

    Ok(Json(StartEmailLoginResponse {
        ok: true,
        email,
        login_url,
    }))
}

/// Names the account a login link belongs to without spending it, so the page asks before
/// signing in and a link requested by someone else cannot switch this browser silently.
/// Whoever holds the link could sign in with it anyway.
pub(crate) async fn check_email_login(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<EmailLoginToken>,
) -> Result<Json<serde_json::Value>> {
    let email = sqlx::query_scalar::<_, String>(
        "SELECT email FROM email_login_tokens WHERE token_hash = $1 AND used_at IS NULL AND expires_at > NOW()",
    )
    .bind(hash_token(request.token.trim()))
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Unauthorized("登录链接无效或已失效".into()))?;
    Ok(Json(serde_json::json!({ "email": email })))
}

pub(crate) async fn consume_email_login(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<EmailLoginToken>,
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
        INSERT INTO users (email, role, last_login_at)
        VALUES ($1, $2, NOW())
        ON CONFLICT (email) DO UPDATE SET
            role = CASE
                WHEN users.role = 'admin' OR EXCLUDED.role = 'admin' THEN 'admin'
                ELSE users.role
            END
        RETURNING id, email, status, role, pro_until
        "#,
    )
    .bind(&email)
    .bind(if state.config.admin_emails.contains(&email) {
        "admin"
    } else {
        "user"
    })
    .fetch_one(&mut *transaction)
    .await?;
    if !user.status {
        // Commit first: the link was single-use and has now been spent. Returning early
        // without committing would roll the `used_at` write back and leave it replayable.
        transaction.commit().await?;
        return Err(AppError::Forbidden("当前账户不可用".into()));
    }

    // Only a login that is allowed to create a session counts as activity.  Keep this
    // separate from the upsert so a disabled account's spent link does not refresh it.
    sqlx::query("UPDATE users SET last_login_at = NOW() WHERE id = $1")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;

    sqlx::query("INSERT INTO sessions (user_id, token_hash, expires_at) VALUES ($1, $2, $3)")
        .bind(user.id)
        .bind(hash_token(&session_token))
        .bind(session_expires_at)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    let mut response = Json(serde_json::json!({ "user": public_user(&user) })).into_response();
    for cookie in [
        session_cookie(&state, &session_token),
        role_cookie(&state, &user.role),
    ] {
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie)
                .map_err(|_| AppError::Internal("会话 Cookie 无效".into()))?,
        );
    }
    Ok(response)
}

pub(crate) async fn me(State(state): State<AppState>, user: User) -> Result<Response> {
    let topic_count =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM topics WHERE owner_user_id = $1")
            .bind(user.id)
            .fetch_one(&state.pool)
            .await?;
    let since = crate::topics::day_start(&state);
    let reset_at = crate::topics::to_utc(
        state.config.timezone,
        state
            .digest
            .today()
            .succ_opt()
            .expect("date in range")
            .and_time(NaiveTime::MIN),
    );
    let runs_today = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id = $1 AND runs.started_at >= $2",
    )
    .bind(user.id)
    .bind(since)
    .fetch_one(&state.pool)
    .await?;
    let mut response = Json(serde_json::json!({
        "user": public_user(&user),
        "usage": {
            "topic_count": topic_count,
            "runs_today": runs_today,
            "reset_at": reset_at,
            "timezone": state.config.timezone.name(),
        },
    }))
    .into_response();
    // Refresh the page's role hint from the authenticated account.
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&role_cookie(&state, &user.role))
            .map_err(|_| AppError::Internal("会话 Cookie 无效".into()))?,
    );
    Ok(response)
}

fn public_user(user: &User) -> PublicUser<'_> {
    PublicUser {
        id: user.id,
        email: &user.email,
        role: &user.role,
        plan: user.plan(),
        pro_until: user.pro_until,
        limits: user.limits(),
    }
}

/// Who is reading, if anyone signed in: no session, an expired one or a disabled account reads
/// as anonymous, but a database error is still an error.
pub(crate) async fn viewer(state: &AppState, headers: &HeaderMap) -> Result<Option<User>> {
    match authenticate(state, headers).await {
        Ok(user) => Ok(Some(user)),
        Err(AppError::Unauthorized(_) | AppError::Forbidden(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn require_admin(user: &User) -> Result<()> {
    user.is_admin()
        .then_some(())
        .ok_or_else(|| AppError::Forbidden("需要管理员权限".into()))
}

pub(crate) fn require_personal(user: &User) -> Result<()> {
    (!user.is_admin())
        .then_some(())
        .ok_or_else(|| AppError::Forbidden("管理员账户仅管理公共主题".into()))
}

pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    if let Some(token) = read_session_token(&headers, &state.config.session_cookie_name) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(hash_token(&token))
            .execute(&state.pool)
            .await?;
    }
    let mut response = axum::http::StatusCode::NO_CONTENT.into_response();
    for cookie in [clear_cookie(&state), clear_role_cookie(&state)] {
        response.headers_mut().append(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie)
                .map_err(|_| AppError::Internal("会话 Cookie 无效".into()))?,
        );
    }
    Ok(response)
}

async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<User> {
    let token = read_session_token(headers, &state.config.session_cookie_name)
        .ok_or_else(|| AppError::Unauthorized("请先登录".into()))?;
    let user = sqlx::query_as::<_, User>(
        r#"
        SELECT users.id, users.email, users.status, users.role, users.pro_until
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
    delete_in_batches("sessions", || {
        sqlx::query(
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
        .bind(CLEANUP_BATCH_SIZE)
        .execute(&state.pool)
    })
    .await?;
    Ok(())
}

async fn cleanup_login_tokens(state: &AppState) -> Result<()> {
    // Spent tokens are retained briefly so they still count toward the send-rate windows.
    delete_in_batches("email_login_tokens", || {
        sqlx::query(
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
        .bind(CLEANUP_BATCH_SIZE)
        .execute(&state.pool)
    })
    .await?;
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
    url.set_path("/me/");
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
    let ttl = state.config.session_ttl.as_secs();
    cookie(state, &state.config.session_cookie_name, token, ttl, true)
}

fn clear_cookie(state: &AppState) -> String {
    cookie(state, &state.config.session_cookie_name, "", 0, true)
}

fn role_cookie(state: &AppState, role: &str) -> String {
    let ttl = state.config.session_ttl.as_secs();
    cookie(state, ROLE_COOKIE_NAME, role, ttl, false)
}

fn clear_role_cookie(state: &AppState) -> String {
    cookie(state, ROLE_COOKIE_NAME, "", 0, false)
}

fn cookie(state: &AppState, name: &str, value: &str, max_age: u64, http_only: bool) -> String {
    let mut cookie = format!("{name}={value}; Path=/; SameSite=Lax; Max-Age={max_age}");
    if http_only {
        cookie.push_str("; HttpOnly");
    }
    if state.config.public_base_url.scheme() == "https" {
        cookie.push_str("; Secure");
    }
    cookie
}

#[cfg(test)]
mod tests {
    use super::{
        FREE_LIMITS, GLOBAL_RATE_WINDOW_MAX, PRO_LIMITS, RATE_WINDOW_MAX, RATE_WINDOW_SECONDS,
        RESEND_COOLDOWN_SECONDS, build_login_url, hash_token, new_token, normalize_email,
        valid_email,
    };
    use url::Url;

    #[test]
    fn workspace_roles_keep_personal_and_public_access_exclusive() {
        let mut user = super::User {
            id: 1,
            email: "user@example.com".into(),
            status: true,
            role: "user".into(),
            pro_until: None,
        };
        assert!(super::require_personal(&user).is_ok());
        assert!(super::require_admin(&user).is_err());
        user.role = "admin".into();
        assert!(super::require_admin(&user).is_ok());
        assert!(super::require_personal(&user).is_err());
    }

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
            "https://news.example.com/me/#login_token=abc_123"
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

    #[test]
    fn plan_limits_match_current_product_policy() {
        assert_eq!(FREE_LIMITS.max_topics, 1);
        assert_eq!(FREE_LIMITS.min_interval_minutes, 512);
        assert_eq!(PRO_LIMITS.max_topics, 4);
        assert_eq!(PRO_LIMITS.min_interval_minutes, 64);
    }

    #[test]
    fn only_pro_accounts_can_create_personal_topics() {
        let free = super::User {
            id: 1,
            email: "free@example.com".into(),
            status: true,
            role: "user".into(),
            pro_until: None,
        };
        assert!(!free.can_create_personal_topic());
    }
}
