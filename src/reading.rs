//! Reading permissions are checked before shared content is loaded. A page has one address
//! for everyone, administrators included, and carries only public addresses; its scripts read
//! through `PublicPage.readPath`. What readers can't see yet, an administrator previews through
//! the same handlers under /api, where neither the gateway nor the browser caches them.
use crate::{
    auth::{self, User},
    error::{AppError, Result},
    state::AppState,
};
use axum::{
    Extension, Router,
    extract::{FromRequestParts, Request},
    http::{HeaderValue, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
};

#[cfg(test)]
pub(crate) mod tests;

pub(crate) const PREFIX: &str = "/api/admin/preview";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Reader {
    Public,
    Admin,
}

impl FromRequestParts<AppState> for Reader {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let reader = *parts
            .extensions
            .get::<Self>()
            .expect("reading routes carry their scope");
        if reader.is_admin() {
            auth::require_admin(&User::from_request_parts(parts, state).await?)?;
        }
        Ok(reader)
    }
}

impl Reader {
    pub(crate) fn is_admin(self) -> bool {
        self == Self::Admin
    }

    /// An enabled site topic, and a visible one unless the reader is an administrator.
    pub(crate) fn check(self, state: &AppState, topic: &str) -> Result<()> {
        state
            .digest
            .topics_for(self)
            .iter()
            .any(|known| known.id == topic)
            .then_some(())
            .ok_or_else(|| AppError::NotFound("主题不存在".into()))
    }
}

pub(crate) fn public(routes: Router<AppState>) -> Router<AppState> {
    routes
        .layer(middleware::from_fn(preview_unpublished))
        .layer(Extension(Reader::Public))
}

pub(crate) fn admin(routes: Router<AppState>) -> Router<AppState> {
    Router::new().nest(
        PREFIX,
        routes
            .layer(Extension(Reader::Admin))
            .route_layer(middleware::from_fn(private_response)),
    )
}

pub(crate) fn pages(routes: Router<AppState>) -> Router<AppState> {
    public(routes.clone()).merge(admin(routes))
}

async fn private_response(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

// A page readers can't see sends an administrator on to its preview. The role cookie only
// chooses the address; the preview endpoint verifies the real session. Errors are never
// cached, so the gateway always passes such a request on to here.
async fn preview_unpublished(request: Request, next: Next) -> Response {
    let html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/html"));
    let admin = request
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .any(|v| v.trim() == "news_role=admin");
    let preview = (html && admin).then(|| format!("{PREFIX}{}", request.uri()));
    let response = next.run(request).await;
    match preview {
        Some(preview) if response.status() == StatusCode::NOT_FOUND => {
            let mut response = Redirect::temporary(&preview).into_response();
            response.headers_mut().insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
            response
        }
        _ => response,
    }
}
