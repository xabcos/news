use crate::{auth, cat, cat_guide, digest, error::Result, payment, state::AppState, topics};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use std::time::Duration;
use tower_http::{services::ServeDir, trace::TraceLayer};
use url::Url;

const MAX_REQUEST_BYTES: usize = 64 * 1024;
const HTTP_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const STATIC_MAX_AGE: &str = "public, max-age=300";
const HTML_MAX_AGE: &str = "public, max-age=60";
/// The public page hosts the Broadcast embed and sends anonymous page views to Stats;
/// news images may come from any HTTPS origin. Everything else stays same-origin.
const CSP: &str = "default-src 'self'; script-src 'self' https://broadcast.xairouter.com; style-src 'self'; img-src 'self' data: https:; font-src 'self'; connect-src 'self' https://stats.xairouter.com https://broadcast.xairouter.com; media-src 'none'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

pub(crate) fn router(state: AppState) -> Router {
    let pages_dir = state.config.pages_dir.clone();
    Router::new()
        .route("/api/auth/email/start", post(auth::start_email_login))
        .route("/api/auth/email/consume", post(auth::consume_email_login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/me", get(auth::me))
        .route("/api/cat/state", get(cat::state))
        .route("/api/cat/home", get(cat::home))
        .route("/api/cat/adoption/preview", post(cat::preview))
        .route("/api/cat/adoption/confirm", post(cat::confirm))
        .route("/api/cat/reading", post(cat::reading))
        .route("/api/cat/guide", get(cat_guide::today))
        .route("/api/cat/trips", get(cat::list_trips))
        .route("/api/cat/journal", get(cat::journal))
        .route("/api/cat/album", get(cat::album))
        .route("/api/cat/atlas", get(cat::atlas))
        .route("/api/cat/encounters", get(cat::encounters))
        .route("/api/plans", get(payment::list_plans))
        .route(
            "/api/orders",
            get(payment::list_orders).post(payment::create_order),
        )
        .route("/api/orders/{out_trade_no}", get(payment::get_order))
        .route("/api/orders/{out_trade_no}/qr.svg", get(payment::order_qr))
        .route("/api/webhooks/payment", post(payment::payment_webhook))
        .merge(topics::router())
        .merge(digest::router())
        .route("/api", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .fallback_service(
            ServeDir::new(&pages_dir)
                .precompressed_gzip()
                .append_index_html_on_directories(true),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            validate_origin,
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
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

    // Populate the digest cache after the listener is ready so a cold cache never delays
    // startup. Requests arriving during warm-up simply fill the same cache on demand.
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
    let signal_task = tokio::spawn(async move {
        shutdown_signal().await;
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
    use super::origin_matches;

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
