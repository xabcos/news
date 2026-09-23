use crate::{auth, digest, error::Result, insight, payment, state::AppState, topics};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use std::time::{Duration, Instant};
use tower_http::{services::ServeDir, trace::TraceLayer};
use url::Url;

const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Comfortably above any real file name under `pages/`, but well under the OS's
/// `NAME_MAX` (255 bytes). Scanners probing for encoding-confusion cache bugs send a
/// single path segment far longer than that; without this guard it reaches the
/// `ServeDir` fallback, which fails the filesystem lookup with ENAMETOOLONG and
/// tower-http logs that as a 500.
const MAX_PATH_SEGMENT_BYTES: usize = 200;
const HTTP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const STATIC_MAX_AGE: &str = "public, max-age=300";
const HTML_MAX_AGE: &str = "public, max-age=60";
/// The public page hosts the Broadcast embed and sends anonymous page views to Stats;
/// news images may come from any HTTPS origin. Everything else stays same-origin.
const CSP: &str = "default-src 'self'; script-src 'self' https://broadcast.xairouter.com; style-src 'self'; img-src 'self' data: blob: https:; font-src 'self'; connect-src 'self' https://stats.xairouter.com https://broadcast.xairouter.com; media-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

pub(crate) fn router(state: AppState) -> Router {
    let pages_dir = state.config.pages_dir.clone();
    Router::new()
        .route("/api/auth/email/start", post(auth::start_email_login))
        .route("/api/auth/email/consume", post(auth::consume_email_login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/plans", get(payment::list_plans))
        .route(
            "/api/orders",
            get(payment::list_orders).post(payment::create_order),
        )
        .route("/api/orders/{out_trade_no}", get(payment::get_order))
        .route("/api/orders/{out_trade_no}/qr.svg", get(payment::order_qr))
        .route("/api/admin/metrics", get(admin_metrics))
        .route(
            "/api/admin/insight",
            get(insight::admin_latest).post(insight::admin_run),
        )
        .route("/api/webhooks/payment", post(payment::payment_webhook))
        .merge(topics::router())
        .merge(digest::router())
        .merge(crate::reads::router())
        .route("/api", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .fallback_service(
            ServeDir::new(&pages_dir)
                .precompressed_gzip()
                .append_index_html_on_directories(true),
        )
        .layer(middleware::from_fn(reject_oversized_path))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            validate_origin,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            observe_metrics,
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn observe_metrics(State(state): State<AppState>, request: Request, next: Next) -> Response {
    state.metrics.request_started();
    let started = Instant::now();
    let response = next.run(request).await;
    state
        .metrics
        .request_finished(response.status().as_u16(), started.elapsed());
    response
}

async fn admin_metrics(
    State(state): State<AppState>,
    user: auth::User,
) -> Result<Json<serde_json::Value>> {
    auth::require_admin(&user)?;
    let requests = state.metrics.snapshot();
    let cache = state.digest.stats();
    let average_duration_us = requests.total_duration_us / requests.total_requests.max(1);
    let pool_size = state.pool.size();
    let pool_idle = state.pool.num_idle();
    let pool_busy = usize::try_from(pool_size)
        .unwrap_or(usize::MAX)
        .saturating_sub(pool_idle);
    Ok(Json(serde_json::json!({
        "requests": {
            "active": requests.active_requests,
            "total": requests.total_requests,
            "errors": requests.error_requests,
            "average_duration_us": average_duration_us,
            "max_duration_us": requests.max_duration_us,
        },
        "database": {
            "size": pool_size,
            "idle": pool_idle,
            "busy": pool_busy,
        },
        "digest_cache": {
            "memory_hits": cache.memory_hits,
            "redis_hits": cache.redis_hits,
            "database_loads": cache.database_loads,
        },
    })))
}

async fn reject_oversized_path(request: Request, next: Next) -> Response {
    if path_too_long(request.uri().path()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    next.run(request).await
}

fn path_too_long(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment.len() > MAX_PATH_SEGMENT_BYTES)
}

async fn api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "error": { "message": "接口不存在", "status": 404 } })),
    )
        .into_response()
}

async fn validate_origin(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let is_payment_webhook = request.uri().path() == "/api/webhooks/payment";
    if !is_payment_webhook
        && !matches!(
            *request.method(),
            Method::GET | Method::HEAD | Method::OPTIONS
        )
    {
        let expected = state.config.public_base_url.origin().ascii_serialization();
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok());
        let fetch_site = request
            .headers()
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok());

        // Either signal alone is enough to accept, but a wrong value in either is fatal.
        // Requiring at least one of them present closes the "no Origin header, therefore
        // allowed" path that a plain Origin comparison leaves open.
        let origin_is_valid = origin.is_some_and(|value| {
            origin_matches(&expected, value, state.config.email_login_dev_links)
        });
        let fetch_is_valid = fetch_site == Some("same-origin");
        let explicitly_invalid = origin.is_some_and(|value| {
            !origin_matches(&expected, value, state.config.email_login_dev_links)
        }) || fetch_site.is_some_and(|value| value != "same-origin");
        if explicitly_invalid || (!origin_is_valid && !fetch_is_valid) {
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": { "message": "请求来源无效", "status": 403 } })),
            )
                .into_response();
        }
    }
    next.run(request).await
}

fn origin_matches(expected: &str, actual: &str, allow_loopback_alias: bool) -> bool {
    if actual == expected {
        return true;
    }
    if !allow_loopback_alias {
        return false;
    }
    let Ok(expected_url) = Url::parse(expected) else {
        return false;
    };
    let Ok(actual_url) = Url::parse(actual) else {
        return false;
    };
    expected_url.scheme() == actual_url.scheme()
        && expected_url.port_or_known_default() == actual_url.port_or_known_default()
        && is_loopback_host(expected_url.host_str())
        && is_loopback_host(actual_url.host_str())
}

fn is_loopback_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

async fn security_headers(request: Request, next: Next) -> Response {
    let is_private = {
        let path = request.uri().path();
        path == "/api" || path.starts_with("/api/") || path == "/me" || path.starts_with("/me/")
    };
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        header::HeaderName::from_static("cross-origin-opener-policy"),
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        header::HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000"),
    );
    // The public site is meant to be found; the workspace and API are not.
    if is_private {
        headers.insert(
            header::HeaderName::from_static("x-robots-tag"),
            HeaderValue::from_static("noindex, nofollow"),
        );
    }
    // Static assets are served with precompressed gzip variants, so shared caches must
    // key on the negotiated encoding to avoid handing gzip bytes to identity clients.
    // Compressed public routes already say so.
    if !headers
        .get_all(header::VARY)
        .iter()
        .any(|value| value.as_bytes().eq_ignore_ascii_case(b"accept-encoding"))
    {
        headers.append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }

    // Public data routes set their own CORS policy and are then readable cross-origin;
    // everything else stays same-origin.
    if !headers.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN) {
        headers.insert(
            header::HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        );
    }
    if !headers.contains_key(header::CACHE_CONTROL) {
        // HTML is detected on the response rather than the request path, so a directory
        // index is handled correctly even though it is served by ServeDir.
        let is_html = headers
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"));
        let value = match (is_private, is_html) {
            (true, _) => "no-store",
            (false, true) => HTML_MAX_AGE,
            (false, false) => STATIC_MAX_AGE,
        };
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    }
    response
}

pub async fn serve(state: AppState) -> Result<()> {
    let bind_addr = state.config.bind_addr;
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|error| {
            crate::error::AppError::Internal(format!("监听地址不可用: {error}").into())
        })?;
    tracing::info!(%bind_addr, "news listening");

    // AppState loads the topic catalogue before serving. Warm news snapshots in the
    // background; requests can load any missing snapshot directly from the database.
    let warmup = tokio::spawn({
        let state = state.clone();
        async move {
            if let Err(error) = state.digest.warm(&state).await {
                tracing::warn!(%error, "digest cache warm-up failed; continuing with on-demand loading");
            }
        }
    });

    let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
    let mut graceful_shutdown = shutdown_receiver.clone();
    let mut shutdown_deadline = shutdown_receiver;
    let worker_shutdown = state.shutdown.clone();
    let signal_task = tokio::spawn(async move {
        shutdown_signal().await;
        // Stop scheduling and cancel detached provider calls while existing HTTP requests
        // drain, so the process shutdown budget is shared by both layers.
        worker_shutdown.cancel();
        let _ = shutdown_sender.send(true);
    });
    let server = std::future::IntoFuture::into_future(
        axum::serve(listener, router(state)).with_graceful_shutdown(async move {
            let _ = graceful_shutdown.changed().await;
        }),
    );
    tokio::pin!(server);

    // Without the deadline a single stuck connection keeps the process alive past the
    // container's stop grace period and it gets killed mid-request instead.
    let result = tokio::select! {
        result = &mut server => map_server_result(result),
        _ = shutdown_deadline.changed() => {
            match tokio::time::timeout(HTTP_SHUTDOWN_TIMEOUT, &mut server).await {
                Ok(result) => map_server_result(result),
                Err(_) => {
                    tracing::warn!("HTTP graceful shutdown timed out; closing connections");
                    Ok(())
                }
            }
        }
    };
    signal_task.abort();
    warmup.abort();
    let _ = warmup.await;
    result
}

fn map_server_result(result: std::io::Result<()>) -> Result<()> {
    result
        .map_err(|error| crate::error::AppError::Internal(format!("HTTP 服务失败: {error}").into()))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "failed to install Ctrl+C handler");
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::warn!(%error, "failed to install terminate handler"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use super::{origin_matches, path_too_long};

    #[test]
    fn path_too_long_flags_a_single_oversized_segment() {
        assert!(!path_too_long("/assets/photo.png"));
        assert!(!path_too_long(&format!("/{}", "a".repeat(200))));
        assert!(path_too_long(&format!("/{}", "a".repeat(201))));
        // A long total path made of short segments is fine; the OS limit is per-segment.
        assert!(!path_too_long(&"/a".repeat(200)));
    }

    #[test]
    fn origin_matching_only_relaxes_loopback_hosts_in_dev() {
        assert!(origin_matches(
            "http://localhost:8080",
            "http://localhost:8080",
            false
        ));
        assert!(!origin_matches(
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            false
        ));
        assert!(origin_matches(
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            true
        ));
        assert!(!origin_matches(
            "http://localhost:8080",
            "https://127.0.0.1:8080",
            true
        ));
        assert!(!origin_matches(
            "http://localhost:8080",
            "http://127.0.0.1:8081",
            true
        ));
        assert!(!origin_matches(
            "https://news.example.com",
            "https://127.0.0.1",
            true
        ));
    }
}
