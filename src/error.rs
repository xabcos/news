use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Query, Request},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::IntoResponse,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::borrow::Cow;

pub type Result<T> = std::result::Result<T, AppError>;

// `Cow<'static, str>` lets the pervasive literal messages stay borrowed instead of
// allocating a String at every construction site; dynamic messages still fit as owned.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Config(Cow<'static, str>),
    #[error("{0}")]
    BadRequest(Cow<'static, str>),
    #[error("{0}")]
    PayloadTooLarge(Cow<'static, str>),
    #[error("{0}")]
    UnsupportedMediaType(Cow<'static, str>),
    #[error("{0}")]
    Unauthorized(Cow<'static, str>),
    #[error("{0}")]
    Forbidden(Cow<'static, str>),
    #[error("{0}")]
    NotFound(Cow<'static, str>),
    #[error("{0}")]
    Conflict(Cow<'static, str>),
    #[error("{0}")]
    TooManyRequests(Cow<'static, str>),
    #[error("{0}")]
    ServiceUnavailable(Cow<'static, str>),
    #[error("{0}")]
    Internal(Cow<'static, str>),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Redis(#[from] redis::RedisError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::ServiceUnavailable(_) | Self::Redis(_) | Self::Http(_) => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::Config(_)
            | Self::Internal(_)
            | Self::Sqlx(_)
            | Self::Migrate(_)
            | Self::Json(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn public_message(&self) -> String {
        match self {
            Self::Config(_)
            | Self::Internal(_)
            | Self::Sqlx(_)
            | Self::Migrate(_)
            | Self::Redis(_)
            | Self::Http(_)
            | Self::Json(_) => "服务暂时不可用，请稍后重试".to_string(),
            _ => self.to_string(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ApiJson<T>(pub(crate) T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection| match rejection.status() {
                StatusCode::PAYLOAD_TOO_LARGE => AppError::PayloadTooLarge("请求内容过大".into()),
                StatusCode::UNSUPPORTED_MEDIA_TYPE => {
                    AppError::UnsupportedMediaType("请求必须使用 application/json".into())
                }
                _ => AppError::BadRequest("请求内容不是有效的 JSON".into()),
            })
    }
}

/// `Query` with the JSON error contract instead of axum's plain-text rejection.
#[derive(Debug)]
pub(crate) struct ApiQuery<T>(pub(crate) T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_| AppError::BadRequest("查询参数无效".into()))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            tracing::warn!(error = %self, "request temporarily unavailable");
        } else if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        let mut response = (
            status,
            Json(json!({
                "error": { "message": self.public_message(), "status": status.as_u16() }
            })),
        )
            .into_response();
        // Errors are per-request state and must never be reused by a shared cache, not even
        // on the public routes where successful responses are deliberately cacheable.
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiJson, ApiQuery, AppError};
    use axum::{
        body::Body,
        extract::{FromRequest, FromRequestParts},
        http::{Request, StatusCode, header},
    };
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        #[serde(rename = "value")]
        _value: String,
    }

    #[tokio::test]
    async fn maps_json_rejections_to_the_api_error_contract() {
        let invalid = Request::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"value":"ok","unknown":true}"#))
            .expect("request builds");
        let error = ApiJson::<Input>::from_request(invalid, &())
            .await
            .expect_err("unknown field must be rejected");
        assert!(matches!(error, AppError::BadRequest(_)));
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn maps_query_rejections_to_the_api_error_contract() {
        #[derive(Debug, Deserialize)]
        struct Q {
            #[allow(dead_code)]
            limit: u8,
        }
        let (mut parts, _) = Request::builder()
            .uri("/digest?limit=abc")
            .body(Body::empty())
            .expect("request builds")
            .into_parts();
        let error = ApiQuery::<Q>::from_request_parts(&mut parts, &())
            .await
            .expect_err("non-numeric limit must be rejected");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn hides_internal_details_but_keeps_client_messages() {
        assert_eq!(
            AppError::Internal("connection refused".into()).public_message(),
            "服务暂时不可用，请稍后重试"
        );
        assert_eq!(
            AppError::Conflict("该主题正在抓取中".into()).public_message(),
            "该主题正在抓取中"
        );
        assert_eq!(AppError::Conflict("".into()).status(), StatusCode::CONFLICT);
    }
}
