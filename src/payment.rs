//! The small, account-scoped payment surface used by the personal console.
//!
//! NEWS deliberately keeps payment state to three tables: a two-row plan
//! catalogue, local orders, and a durable webhook idempotency log.  YPay owns
//! the actual WeChat checkout; this service only records a successful purchase
//! and extends `users.pro_until`.

use crate::{
    auth::{self, User},
    error::{ApiJson, AppError, Result},
    state::AppState,
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use hmac::{Hmac, KeyInit, Mac};
use qrcode::{QrCode, render::svg};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::{FromRow, PgPool};
use std::time::Duration as StdDuration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub(crate) const PRO_PRICE_FEN: i64 = 9_900;
pub(crate) const PRO_DURATION_DAYS: i32 = 31;
const PAYMENT_CREATE_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const PAYMENT_CLAIM_TTL: ChronoDuration = ChronoDuration::minutes(2);
const ORDER_CLEANUP_BATCH_SIZE: i64 = 500;
const ORDER_CLEANUP_MAX_BATCHES: usize = 20;

#[derive(Debug, Clone, Serialize, FromRow)]
pub(crate) struct Plan {
    pub(crate) code: String,
    pub(crate) name: String,
    pub(crate) price_fen: i64,
    pub(crate) duration_days: i32,
    pub(crate) description: String,
    pub(crate) active: bool,
    pub(crate) sort_weight: i32,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub(crate) struct Order {
    pub(crate) id: i64,
    pub(crate) plan_code: String,
    pub(crate) out_trade_no: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provider_trade_no: Option<String>,
    pub(crate) amount_fen: i64,
    pub(crate) duration_days: i32,
    pub(crate) status: String,
    pub(crate) expires_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) paid_at: Option<DateTime<Utc>>,
    pub(crate) payment: Value,
    #[serde(skip_serializing)]
    pub(crate) payment_started_at: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub(crate) struct OrderResponse {
    pub(crate) order: Order,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) payment: Option<YpayPayment>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PlansResponse {
    pub(crate) plans: Vec<Plan>,
    /// Lets the small personal console avoid presenting a dead checkout button
    /// in local/staging deployments where YPay is intentionally unset.
    pub(crate) checkout_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct YpayPayment {
    #[serde(default)]
    pub(crate) order_id: Option<i64>,
    pub(crate) out_trade_no: String,
    #[serde(default)]
    pub(crate) amount: i64,
    #[serde(default)]
    pub(crate) credit_amount: Option<f64>,
    #[serde(default)]
    pub(crate) pay_status: String,
    #[serde(default)]
    pub(crate) fulfill_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) code_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prepay_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sign_data: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) h5_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expired_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateOrderRequest {
    pub(crate) plan_code: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PaymentWebhookRequest {
    pub(crate) out_trade_no: String,
    #[serde(default)]
    pub(crate) provider_trade_no: Option<String>,
    pub(crate) status: String,
}

#[derive(Debug, FromRow)]
struct PaymentOrder {
    id: i64,
    user_id: i64,
    plan_code: String,
    amount_fen: i64,
    duration_days: i32,
    status: String,
    paid_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct YpayCreateOrderRequest {
    external_user_id: i64,
    amount: f64,
    amount_input: String,
    pay_method: String,
    description: String,
    out_trade_no: String,
    product_code: String,
    product_name: String,
    unit_count: i64,
    metadata: Value,
}

pub(crate) async fn list_plans(State(state): State<AppState>) -> Result<Json<PlansResponse>> {
    let plans = sqlx::query_as::<_, Plan>(
        r#"
        SELECT code, name, price_fen, duration_days, description, active,
               sort_weight, created_at, updated_at
        FROM plans
        WHERE active = TRUE
        ORDER BY sort_weight, code
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(PlansResponse {
        plans,
        checkout_enabled: payment_enabled(&state),
    }))
}

pub(crate) async fn create_order(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<CreateOrderRequest>,
) -> Result<Json<OrderResponse>> {
    auth::require_personal(&user)?;
    ensure_payment_enabled(&state)?;
    let plan_code = request.plan_code.trim().to_ascii_lowercase();
    if plan_code != "pro" {
        return Err(AppError::BadRequest("当前仅支持 Pro 套餐".into()));
    }
    let plan = sqlx::query_as::<_, Plan>(
        r#"
        SELECT code, name, price_fen, duration_days, description, active,
               sort_weight, created_at, updated_at
        FROM plans
        WHERE code = $1 AND active = TRUE
        "#,
    )
    .bind(&plan_code)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::BadRequest("当前套餐不可用".into()))?;
    if plan.price_fen != PRO_PRICE_FEN || plan.duration_days != PRO_DURATION_DAYS {
        return Err(AppError::Internal("Pro 套餐配置不一致".into()));
    }

    let user_id = user.id;
    // Claim the local order in a short transaction. The YPay request happens
    // after commit so a slow provider cannot occupy a database connection, and
    // the committed pending row can still receive a webhook after a restart.
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE orders SET status = 'closed', updated_at = NOW() \
         WHERE user_id = $1 AND plan_code = $2 AND status = 'pending' \
         AND (expires_at <= NOW() OR created_at <= NOW() - INTERVAL '1 hour')",
    )
    .bind(user_id)
    .bind(&plan.code)
    .execute(&mut *transaction)
    .await?;
    let mut order = if let Some(order) = sqlx::query_as::<_, Order>(
        r#"
        SELECT id, plan_code, out_trade_no, provider_trade_no,
               amount_fen, duration_days, status, expires_at, paid_at, payment,
               payment_started_at,
               created_at, updated_at
        FROM orders
        WHERE user_id = $1 AND plan_code = $2 AND status = 'pending' AND expires_at > NOW()
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        FOR UPDATE
        "#,
    )
    .bind(user_id)
    .bind(&plan.code)
    .fetch_optional(&mut *transaction)
    .await?
    {
        order
    } else {
        let out_trade_no = new_out_trade_no();
        sqlx::query_as::<_, Order>(
            r#"
            INSERT INTO orders
                (user_id, plan_code, out_trade_no, amount_fen, duration_days)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, plan_code, out_trade_no, provider_trade_no,
                      amount_fen, duration_days, status, expires_at, paid_at, payment,
                      payment_started_at,
                      created_at, updated_at
            "#,
        )
        .bind(user_id)
        .bind(&plan.code)
        .bind(out_trade_no)
        .bind(plan.price_fen)
        .bind(plan.duration_days)
        .fetch_one(&mut *transaction)
        .await?
    };
    if let Some(payment) = stored_payment(&order) {
        transaction.commit().await?;
        return Ok(Json(OrderResponse {
            order,
            payment: Some(payment),
        }));
    }

    let now = Utc::now();
    if let Some(started) = order.payment_started_at {
        if now - started < PAYMENT_CLAIM_TTL {
            transaction.commit().await?;
            return Err(AppError::Conflict(
                "支付二维码正在生成，请稍后刷新订单".into(),
            ));
        }
        // The previous process may have died after YPay accepted the request
        // but before saving its response. YPay does not accept duplicate
        // merchant numbers, so retire that uncertain local order; a signed
        // late webhook is still accepted and can credit it normally.
        sqlx::query(
            "UPDATE orders SET status = 'closed', updated_at = NOW() WHERE id = $1 AND status = 'pending'",
        )
        .bind(order.id)
        .execute(&mut *transaction)
        .await?;
        let out_trade_no = new_out_trade_no();
        order = sqlx::query_as::<_, Order>(
            r#"
            INSERT INTO orders
                (user_id, plan_code, out_trade_no, amount_fen, duration_days)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, plan_code, out_trade_no, provider_trade_no,
                      amount_fen, duration_days, status, expires_at, paid_at, payment,
                      payment_started_at, created_at, updated_at
            "#,
        )
        .bind(user_id)
        .bind(&plan.code)
        .bind(out_trade_no)
        .bind(plan.price_fen)
        .bind(plan.duration_days)
        .fetch_one(&mut *transaction)
        .await?;
    }
    let order = sqlx::query_as::<_, Order>(
        r#"
        UPDATE orders
        SET payment_started_at = NOW(), updated_at = NOW()
        WHERE id = $1 AND status = 'pending'
        RETURNING id, plan_code, out_trade_no, provider_trade_no,
                  amount_fen, duration_days, status, expires_at, paid_at, payment,
                  payment_started_at, created_at, updated_at
        "#,
    )
    .bind(order.id)
    .fetch_one(&mut *transaction)
    .await?;
    let claim_started_at = order
        .payment_started_at
        .ok_or_else(|| AppError::Internal("支付订单租约创建失败".into()))?;
    transaction.commit().await?;

    // Keep the lease on every provider error. A timeout or malformed response
    // is ambiguous: YPay may already have accepted the order, so reusing its
    // merchant number immediately could strand a paid checkout.
    let payment = create_ypay_order(&state, &user, &plan, &order).await?;
    let payment_value = serde_json::to_value(&payment)?;
    let saved_order = sqlx::query_as::<_, Order>(
        r#"
        UPDATE orders
        SET payment = $2, expires_at = LEAST(COALESCE($3, expires_at), created_at + INTERVAL '1 hour'),
            payment_started_at = NULL, updated_at = NOW()
        WHERE id = $1 AND status = 'pending' AND payment_started_at = $4
        RETURNING id, plan_code, out_trade_no, provider_trade_no,
                  amount_fen, duration_days, status, expires_at, paid_at, payment,
                  payment_started_at, created_at, updated_at
        "#,
    )
    .bind(order.id)
    .bind(payment_value)
    .bind(payment.expired_at)
    .bind(claim_started_at)
    .fetch_optional(&state.pool)
    .await?;
    let order = if let Some(order) = saved_order {
        order
    } else {
        let order = load_order(&state, order.id).await?;
        if order.status == "paid" {
            return Ok(Json(OrderResponse {
                order,
                payment: None,
            }));
        }
        return Err(AppError::Conflict("订单状态已变化，请刷新后重试".into()));
    };
    Ok(Json(OrderResponse {
        order,
        payment: Some(payment),
    }))
}

pub(crate) async fn list_orders(
    State(state): State<AppState>,
    user: User,
) -> Result<Json<Vec<Order>>> {
    auth::require_personal(&user)?;
    close_expired_orders(&state, user.id).await?;
    let orders = sqlx::query_as::<_, Order>(
        r#"
        SELECT id, plan_code, out_trade_no, provider_trade_no,
               amount_fen, duration_days, status, expires_at, paid_at, payment,
               payment_started_at,
               created_at, updated_at
        FROM orders
        WHERE user_id = $1
        ORDER BY created_at DESC, id DESC
        LIMIT 20
        "#,
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(orders))
}

pub(crate) async fn get_order(
    State(state): State<AppState>,
    user: User,
    Path(out_trade_no): Path<String>,
) -> Result<Json<OrderResponse>> {
    auth::require_personal(&user)?;
    validate_out_trade_no(&out_trade_no)?;
    close_expired_orders(&state, user.id).await?;
    let order = load_user_order(&state, user.id, &out_trade_no).await?;
    let payment = (order.status == "pending")
        .then(|| stored_payment(&order))
        .flatten();
    Ok(Json(OrderResponse { order, payment }))
}

pub(crate) async fn order_qr(
    State(state): State<AppState>,
    user: User,
    Path(out_trade_no): Path<String>,
) -> Result<Response> {
    auth::require_personal(&user)?;
    validate_out_trade_no(&out_trade_no)?;
    close_expired_orders(&state, user.id).await?;
    let order = load_user_order(&state, user.id, &out_trade_no).await?;
    if order.status != "pending" || order.expires_at <= Utc::now() {
        return Err(AppError::Conflict("当前订单无需继续支付".into()));
    }
    let payment = stored_payment(&order)
        .ok_or_else(|| AppError::ServiceUnavailable("支付信息暂不可用".into()))?;
    let code_url = payment
        .code_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::ServiceUnavailable("当前支付方式不支持扫码".into()))?;
    if code_url.len() > 4096 || code_url.chars().any(char::is_control) {
        return Err(AppError::BadRequest("二维码内容无效".into()));
    }
    let code = QrCode::new(code_url.as_bytes())
        .map_err(|_| AppError::BadRequest("二维码内容过长".into()))?;
    let svg = code
        .render::<svg::Color>()
        .min_dimensions(320, 320)
        .dark_color(svg::Color("#13231f"))
        .light_color(svg::Color("#ffffff"))
        .build();
    let mut response = svg.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    Ok(response)
}

pub(crate) async fn payment_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>> {
    verify_webhook_signature(&state, &headers, &body)?;
    let request: PaymentWebhookRequest = serde_json::from_slice(&body)
        .map_err(|_| AppError::BadRequest("无效的支付回调内容".into()))?;
    validate_webhook_request(&request)?;
    let payload: Value = serde_json::from_slice(&body)?;
    let (pro_until, duplicate) = apply_payment_event(&state, &request, payload).await?;
    Ok(Json(json!({
        "ok": true,
        "duplicate": duplicate,
        "pro_until": pro_until,
    })))
}

async fn close_expired_orders(state: &AppState, user_id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE orders SET status = 'closed', updated_at = NOW() \
         WHERE user_id = $1 AND status = 'pending' \
         AND (expires_at <= NOW() OR created_at <= NOW() - INTERVAL '1 hour')",
    )
    .bind(user_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Runs independently of slow news/image maintenance and also when NEWS
/// collection is disabled. The first interval tick performs startup catch-up.
pub(crate) async fn maintenance_loop(
    pool: PgPool,
    timezone: chrono_tz::Tz,
    shutdown: CancellationToken,
) {
    let mut interval = tokio::time::interval(StdDuration::from_secs(60));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cleaned_day = None;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        match close_pending_orders(&pool).await {
            Ok(closed) if closed > 0 => tracing::info!(closed, "expired unpaid orders closed"),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "order closure deferred"),
        }
        let day = Utc::now().with_timezone(&timezone).date_naive();
        if cleaned_day != Some(day) {
            match cleanup_orders(&pool, day).await {
                Ok(Some(deleted)) => {
                    cleaned_day = Some(day);
                    if deleted > 0 {
                        tracing::info!(deleted, %day, "daily closed-order cleanup completed");
                    }
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "daily order cleanup deferred"),
            }
        }
    }
}

async fn close_pending_orders(pool: &PgPool) -> Result<u64> {
    let mut total = 0;
    for _ in 0..ORDER_CLEANUP_MAX_BATCHES {
        let closed = sqlx::query(
            r#"WITH expired AS (
                   SELECT id FROM orders
                   WHERE status='pending' AND paid_at IS NULL
                     AND created_at <= NOW() - INTERVAL '1 hour'
                   ORDER BY created_at,id LIMIT $1 FOR UPDATE SKIP LOCKED
               )
               UPDATE orders SET status='closed',updated_at=NOW()
               WHERE id IN (SELECT id FROM expired)"#,
        )
        .bind(ORDER_CLEANUP_BATCH_SIZE)
        .execute(pool)
        .await?
        .rows_affected();
        total += closed;
        if closed < ORDER_CLEANUP_BATCH_SIZE as u64 {
            break;
        }
    }
    Ok(total)
}

/// The date marker and deletion commit together. A second process or restart
/// observes the completed day, while a failed transaction can retry safely.
async fn cleanup_orders(pool: &PgPool, day: NaiveDate) -> Result<Option<u64>> {
    let mut tx = pool.begin().await?;
    let locked: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtextextended('news:order_cleanup',0))",
    )
    .fetch_one(&mut *tx)
    .await?;
    if !locked {
        return Ok(None);
    }
    let last_day = sqlx::query_scalar::<_, NaiveDate>(
        "SELECT last_completed_day FROM maintenance_state WHERE task='order_cleanup'",
    )
    .fetch_optional(&mut *tx)
    .await?;
    if last_day.is_some_and(|previous| previous >= day) {
        return Ok(Some(0));
    }
    let mut total = 0;
    for _ in 0..ORDER_CLEANUP_MAX_BATCHES {
        let ids = sqlx::query_scalar::<_, i64>(
            r#"SELECT o.id FROM orders o
               WHERE o.status='closed' AND o.paid_at IS NULL
                 AND o.created_at <= NOW() - INTERVAL '1 hour'
                 AND NOT EXISTS (SELECT 1 FROM payment_events e
                     WHERE e.out_trade_no=o.out_trade_no AND e.status='paid')
               ORDER BY o.created_at,o.id LIMIT $1
               FOR UPDATE OF o SKIP LOCKED"#,
        )
        .bind(ORDER_CLEANUP_BATCH_SIZE)
        .fetch_all(&mut *tx)
        .await?;
        if ids.is_empty() {
            break;
        }
        sqlx::query(
            r#"INSERT INTO payment_events(event_key,out_trade_no,user_id,status,payload)
               SELECT 'closed:'||out_trade_no,out_trade_no,user_id,'closed',
                   jsonb_build_object('plan_code',plan_code,'amount_fen',amount_fen,
                       'duration_days',duration_days,'created_at',created_at,'expires_at',expires_at)
               FROM orders WHERE id=ANY($1)
               ON CONFLICT(event_key) DO NOTHING"#,
        )
        .bind(&ids).execute(&mut *tx).await?;
        let deleted = sqlx::query(
            "DELETE FROM orders WHERE id=ANY($1) AND status='closed' AND paid_at IS NULL",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        total += deleted;
        if ids.len() < ORDER_CLEANUP_BATCH_SIZE as usize {
            break;
        }
    }
    sqlx::query("INSERT INTO maintenance_state(task,last_completed_day) VALUES('order_cleanup',$1) ON CONFLICT(task) DO UPDATE SET last_completed_day=EXCLUDED.last_completed_day")
        .bind(day).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(total))
}

async fn load_user_order(state: &AppState, user_id: i64, out_trade_no: &str) -> Result<Order> {
    sqlx::query_as::<_, Order>(
        r#"
        SELECT id, plan_code, out_trade_no, provider_trade_no,
               amount_fen, duration_days, status, expires_at, paid_at, payment,
               payment_started_at, created_at, updated_at
        FROM orders
        WHERE user_id = $1 AND out_trade_no = $2
        "#,
    )
    .bind(user_id)
    .bind(out_trade_no)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("订单不存在或已过期清理，请重新发起支付".into()))
}

async fn load_order(state: &AppState, order_id: i64) -> Result<Order> {
    sqlx::query_as::<_, Order>(
        r#"
        SELECT id, plan_code, out_trade_no, provider_trade_no,
               amount_fen, duration_days, status, expires_at, paid_at, payment,
               payment_started_at, created_at, updated_at
        FROM orders
        WHERE id = $1
        "#,
    )
    .bind(order_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("订单不存在".into()))
}

async fn create_ypay_order(
    state: &AppState,
    user: &User,
    plan: &Plan,
    order: &Order,
) -> Result<YpayPayment> {
    let base_url = state
        .config
        .ypay_base_url
        .as_ref()
        .ok_or_else(|| AppError::ServiceUnavailable("支付服务暂未启用".into()))?;
    let store_token = state
        .config
        .ypay_store_api_key
        .as_deref()
        .ok_or_else(|| AppError::ServiceUnavailable("支付服务暂未启用".into()))?;
    let url = format!(
        "{}/t/{}/store/orders/create",
        base_url.as_str().trim_end_matches('/'),
        state.config.ypay_tenant_code
    );
    let payload = YpayCreateOrderRequest {
        external_user_id: user.id,
        amount: order.amount_fen as f64 / 100.0,
        amount_input: fen_amount_input(order.amount_fen),
        pay_method: state.config.ypay_pay_method.clone(),
        description: format!("{} · {} 天", plan.name, plan.duration_days),
        out_trade_no: order.out_trade_no.clone(),
        product_code: plan.code.clone(),
        product_name: plan.name.clone(),
        unit_count: i64::from(plan.duration_days),
        metadata: json!({
            "news_user_id": user.id,
            "plan": plan.code,
            "duration_days": plan.duration_days,
        }),
    };
    let response = state
        .http
        .post(url)
        .bearer_auth(store_token)
        .json(&payload)
        .timeout(PAYMENT_CREATE_TIMEOUT)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, body = %truncate_for_log(&body), "YPay order creation rejected");
        return Err(AppError::ServiceUnavailable(
            "暂时无法创建订单，请稍后重试".into(),
        ));
    }
    let value: Value = response.json().await?;
    let value = value.get("data").cloned().unwrap_or(value);
    let payment: YpayPayment = serde_json::from_value(value)
        .map_err(|_| AppError::ServiceUnavailable("支付服务返回内容无效".into()))?;
    if payment.out_trade_no != order.out_trade_no {
        return Err(AppError::Internal("支付订单号不一致".into()));
    }
    if payment.amount != order.amount_fen {
        return Err(AppError::Internal("支付金额不一致".into()));
    }
    Ok(payment)
}

async fn apply_payment_event(
    state: &AppState,
    request: &PaymentWebhookRequest,
    payload: Value,
) -> Result<(Option<DateTime<Utc>>, bool)> {
    let (user_id, pro_until, duplicate) = settle_payment(&state.pool, request, payload).await?;
    crate::topics::wake_personal_topics(state, user_id, crate::auth::PRO_LIMITS.max_topics).await?;
    Ok((pro_until, duplicate))
}

async fn lock_payment_order(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    out_trade_no: &str,
) -> Result<Option<PaymentOrder>> {
    Ok(sqlx::query_as::<_, PaymentOrder>(
        r#"
        SELECT id, user_id, plan_code, amount_fen,
               duration_days, status, paid_at
        FROM orders
        WHERE out_trade_no = $1
        FOR UPDATE
        "#,
    )
    .bind(out_trade_no)
    .fetch_optional(&mut **transaction)
    .await?)
}

async fn settle_payment(
    pool: &PgPool,
    request: &PaymentWebhookRequest,
    payload: Value,
) -> Result<(i64, Option<DateTime<Utc>>, bool)> {
    let event_key = format!("paid:{}", request.out_trade_no);
    let mut transaction = pool.begin().await?;
    // Use the same per-user lock as order creation so a delayed webhook cannot
    // race a second tab that is preparing the same pending order.
    let user_id =
        sqlx::query_scalar::<_, i64>("SELECT user_id FROM orders WHERE out_trade_no = $1 UNION ALL SELECT user_id FROM payment_events WHERE event_key = 'closed:' || $1 AND status = 'closed' LIMIT 1")
            .bind(&request.out_trade_no)
            .fetch_optional(&mut *transaction)
            .await?
            .ok_or_else(|| AppError::NotFound("订单不存在".into()))?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(user_id)
        .execute(&mut *transaction)
        .await?;
    let order = match lock_payment_order(&mut transaction, &request.out_trade_no).await? {
        Some(order) => order,
        None => {
            // Cleanup removes checkout/QR details, but a signed delayed payment may
            // reconstruct its immutable purchase terms from the minimal closure event.
            sqlx::query(
                r#"INSERT INTO orders(user_id,plan_code,out_trade_no,amount_fen,duration_days,status,created_at,expires_at)
                   SELECT user_id,payload->>'plan_code',out_trade_no,
                       (payload->>'amount_fen')::bigint,(payload->>'duration_days')::integer,'closed',
                       (payload->>'created_at')::timestamptz,(payload->>'expires_at')::timestamptz
                   FROM payment_events e
                   WHERE e.event_key='closed:'||$1 AND e.user_id=$2 AND e.status='closed'
                     AND NOT EXISTS(SELECT 1 FROM orders o WHERE o.out_trade_no=$1)
                   ON CONFLICT(out_trade_no) DO NOTHING"#,
            )
            .bind(&request.out_trade_no).bind(user_id).execute(&mut *transaction).await?;
            lock_payment_order(&mut transaction, &request.out_trade_no)
                .await?
                .ok_or_else(|| AppError::NotFound("订单不存在".into()))?
        }
    };
    if order.plan_code != "pro"
        || order.amount_fen != PRO_PRICE_FEN
        || order.duration_days != PRO_DURATION_DAYS
    {
        return Err(AppError::Conflict("订单套餐信息不一致".into()));
    }
    if order.status == "paid" {
        let pro_until = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT pro_until FROM users WHERE id = $1",
        )
        .bind(order.user_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Ok((order.user_id, pro_until, true));
    }

    let inserted = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO payment_events (event_key, out_trade_no, status, payload, user_id)
        VALUES ($1, $2, 'paid', $3, $4)
        ON CONFLICT (event_key) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(&event_key)
    .bind(&request.out_trade_no)
    .bind(payload)
    .bind(order.user_id)
    .fetch_optional(&mut *transaction)
    .await?;
    if inserted.is_none() {
        let pro_until = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT pro_until FROM users WHERE id = $1",
        )
        .bind(order.user_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        return Ok((order.user_id, pro_until, true));
    }

    let now = Utc::now();
    let current = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT pro_until FROM users WHERE id = $1 FOR UPDATE",
    )
    .bind(order.user_id)
    .fetch_one(&mut *transaction)
    .await?;
    let base = current.filter(|expires| *expires > now).unwrap_or(now);
    let pro_until = base + ChronoDuration::days(i64::from(order.duration_days));
    let pro_until = sqlx::query_scalar::<_, DateTime<Utc>>(
        "UPDATE users SET pro_until = $2 WHERE id = $1 RETURNING pro_until",
    )
    .bind(order.user_id)
    .bind(pro_until)
    .fetch_one(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE orders
        SET status = 'paid', provider_trade_no = COALESCE($2, provider_trade_no),
            paid_at = COALESCE(paid_at, $3), payment_started_at = NULL,
            updated_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(order.id)
    .bind(request.provider_trade_no.as_deref())
    .bind(order.paid_at.unwrap_or(now))
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM payment_events WHERE event_key='closed:'||$1 AND user_id=$2")
        .bind(&request.out_trade_no)
        .bind(order.user_id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok((order.user_id, Some(pro_until), false))
}

fn ensure_payment_enabled(state: &AppState) -> Result<()> {
    if !payment_enabled(state) {
        return Err(AppError::ServiceUnavailable("支付服务暂未启用".into()));
    }
    Ok(())
}

fn payment_enabled(state: &AppState) -> bool {
    state.config.ypay_base_url.is_some()
        && state.config.ypay_store_api_key.is_some()
        && state.config.payment_webhook_secret.is_some()
}

fn verify_webhook_signature(state: &AppState, headers: &HeaderMap, body: &[u8]) -> Result<()> {
    let secret = state
        .config
        .payment_webhook_secret
        .as_deref()
        .ok_or_else(|| AppError::ServiceUnavailable("支付回调未配置".into()))?;
    let signature = ["x-ypay-signature", "x-xabcimg-signature"]
        .iter()
        .find_map(|name| headers.get(*name))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or("");
    let signature = signature.strip_prefix("sha256=").unwrap_or(signature);
    let provided =
        hex::decode(signature).map_err(|_| AppError::Unauthorized("回调签名无效".into()))?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| AppError::Internal("回调密钥无效".into()))?;
    mac.update(body);
    mac.verify_slice(&provided)
        .map_err(|_| AppError::Unauthorized("回调签名无效".into()))
}

fn validate_webhook_request(request: &PaymentWebhookRequest) -> Result<()> {
    validate_out_trade_no(&request.out_trade_no)?;
    if request.status.trim() != "paid" {
        return Err(AppError::BadRequest("不支持的支付回调状态".into()));
    }
    Ok(())
}

fn validate_out_trade_no(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::BadRequest("订单编号无效".into()));
    }
    Ok(())
}

fn stored_payment(order: &Order) -> Option<YpayPayment> {
    if !order.payment.is_object() {
        return None;
    }
    serde_json::from_value(order.payment.clone()).ok()
}

fn new_out_trade_no() -> String {
    let nonce = Uuid::new_v4().simple().to_string();
    format!("NW{}{}", Utc::now().timestamp(), &nonce[..12])
}

fn fen_amount_input(amount_fen: i64) -> String {
    format!("{}.{:02}", amount_fen / 100, amount_fen.abs() % 100)
}

fn truncate_for_log(value: &str) -> String {
    const MAX: usize = 512;
    value
        .get(..MAX)
        .map_or_else(|| value.to_string(), |prefix| format!("{prefix}…"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::Mac;

    #[test]
    fn fixed_pro_catalogue_values() {
        assert_eq!(PRO_PRICE_FEN, 9_900);
        assert_eq!(PRO_DURATION_DAYS, 31);
        assert_eq!(fen_amount_input(PRO_PRICE_FEN), "99.00");
    }

    #[test]
    fn merchant_order_numbers_are_short_and_safe() {
        let value = new_out_trade_no();
        assert!(value.len() <= 32);
        assert!(
            value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        );
    }

    #[test]
    fn webhook_signature_matches_raw_body() {
        let body = br#"{"out_trade_no":"NW1","status":"paid"}"#;
        let mut mac = HmacSha256::new_from_slice(b"secret").expect("key");
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());

        let mut check = HmacSha256::new_from_slice(b"secret").expect("key");
        check.update(body);
        assert!(
            check
                .verify_slice(&hex::decode(signature).expect("hex"))
                .is_ok()
        );

        let mut other = HmacSha256::new_from_slice(b"secret").expect("key");
        other.update(b"different");
        assert!(
            other
                .verify_slice(&hex::decode("00").expect("hex"))
                .is_err()
        );
    }

    async fn fixture_owner(pool: &PgPool) -> i64 {
        sqlx::query_scalar("INSERT INTO users(email) VALUES($1) RETURNING id")
            .bind(format!("payment-{}@example.test", Uuid::new_v4()))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn fixture_order(pool: &PgPool, user: i64, age_minutes: i64, status: &str) -> String {
        let number = new_out_trade_no();
        sqlx::query("INSERT INTO orders(user_id,plan_code,out_trade_no,amount_fen,duration_days,status,created_at,paid_at,payment) VALUES($1,'pro',$2,9900,31,$3,NOW()-($4*INTERVAL '1 minute'),CASE WHEN $3='paid' THEN NOW() END,'{\"code_url\":\"private-checkout-data\"}')")
            .bind(user).bind(&number).bind(status).bind(age_minutes).execute(pool).await.unwrap();
        number
    }

    async fn order_status(pool: &PgPool, number: &str) -> Option<String> {
        sqlx::query_scalar("SELECT status FROM orders WHERE out_trade_no=$1")
            .bind(number)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore = "requires an explicitly named disposable PostgreSQL database"]
    async fn postgres_daily_order_cleanup_preserves_late_payments_and_idempotency() {
        let url = std::env::var("NEWS_PAYMENT_TEST_DATABASE_URL").expect("disposable database URL");
        let pool = PgPool::connect(&url).await.unwrap();
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(database.starts_with("news_payment_audit_"));
        sqlx::migrate::Migrator::new(std::path::Path::new("migrations"))
            .await
            .unwrap()
            .run(&pool)
            .await
            .unwrap();
        let day = Utc::now().date_naive();
        let user = fixture_owner(&pool).await;
        let overdue = fixture_order(&pool, user, 61, "pending").await;
        let fresh = fixture_order(&pool, user, 59, "pending").await;
        let closed = fixture_order(&pool, user, 70, "closed").await;
        let recent_closed = fixture_order(&pool, user, 30, "closed").await;
        let paid = fixture_order(&pool, user, 90, "paid").await;
        let recorded_payment = fixture_order(&pool, user, 90, "closed").await;
        sqlx::query("INSERT INTO payment_events(event_key,out_trade_no,user_id,status,payload) VALUES($1,$2,$3,'paid','{}')")
            .bind(format!("paid:{recorded_payment}")).bind(&recorded_payment).bind(user).execute(&pool).await.unwrap();
        assert_eq!(close_pending_orders(&pool).await.unwrap(), 1);
        assert_eq!(
            order_status(&pool, &overdue).await.as_deref(),
            Some("closed")
        );
        assert_eq!(
            order_status(&pool, &fresh).await.as_deref(),
            Some("pending")
        );
        assert_eq!(cleanup_orders(&pool, day).await.unwrap(), Some(2));
        assert!(order_status(&pool, &overdue).await.is_none());
        assert!(order_status(&pool, &closed).await.is_none());
        assert_eq!(order_status(&pool, &paid).await.as_deref(), Some("paid"));
        assert_eq!(
            order_status(&pool, &recorded_payment).await.as_deref(),
            Some("closed")
        );
        assert_eq!(
            order_status(&pool, &recent_closed).await.as_deref(),
            Some("closed")
        );
        let receipt: Value =
            sqlx::query_scalar("SELECT payload FROM payment_events WHERE event_key=$1")
                .bind(format!("closed:{overdue}"))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(receipt.get("payment").is_none());
        assert!(receipt.get("code_url").is_none());

        let later = fixture_order(&pool, user, 61, "pending").await;
        assert_eq!(close_pending_orders(&pool).await.unwrap(), 1);
        let restarted = PgPool::connect(&url).await.unwrap();
        assert_eq!(cleanup_orders(&restarted, day).await.unwrap(), Some(0));
        assert_eq!(order_status(&pool, &later).await.as_deref(), Some("closed"));
        assert_eq!(
            cleanup_orders(&restarted, day + ChronoDuration::days(1))
                .await
                .unwrap(),
            Some(1)
        );
        restarted.close().await;

        let request = PaymentWebhookRequest {
            out_trade_no: overdue.clone(),
            provider_trade_no: Some("test-provider".into()),
            status: "paid".into(),
        };
        let (_, granted, duplicate) = settle_payment(&pool, &request, json!({"status":"paid"}))
            .await
            .unwrap();
        assert!(!duplicate);
        assert_eq!(order_status(&pool, &overdue).await.as_deref(), Some("paid"));
        let (_, repeated, duplicate) = settle_payment(&pool, &request, json!({"status":"paid"}))
            .await
            .unwrap();
        assert!(duplicate);
        assert_eq!(granted, repeated);
        let closed_events: i64 =
            sqlx::query_scalar("SELECT count(*) FROM payment_events WHERE event_key=$1")
                .bind(format!("closed:{overdue}"))
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(closed_events, 0);

        // Either cleanup or the callback may lock first. The account must get
        // exactly one grant and a durable paid order in both cases.
        for index in 2..10 {
            let user = fixture_owner(&pool).await;
            let number = fixture_order(&pool, user, 61, "closed").await;
            let request = PaymentWebhookRequest {
                out_trade_no: number.clone(),
                provider_trade_no: None,
                status: "paid".into(),
            };
            let (cleaned, settled) = tokio::join!(
                cleanup_orders(&pool, day + ChronoDuration::days(index)),
                settle_payment(&pool, &request, json!({"status":"paid"})),
            );
            cleaned.unwrap();
            let (_, granted, duplicate) = settled.unwrap();
            assert!(!duplicate);
            assert_eq!(order_status(&pool, &number).await.as_deref(), Some("paid"));
            let (_, again, duplicate) = settle_payment(&pool, &request, json!({"status":"paid"}))
                .await
                .unwrap();
            assert!(duplicate);
            assert_eq!(granted, again);
        }
        pool.close().await;
    }
}
