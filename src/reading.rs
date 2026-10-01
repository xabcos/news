//! Reading permissions are checked before shared content is loaded. Admin previews use
//! the same handlers under /api, where neither the gateway nor the browser caches them.
use crate::{
    auth::{self, User},
    error::{AppError, Result},
    site,
    state::AppState,
};
use axum::{
    Extension, Router,
    extract::{FromRequestParts, Request},
    http::{HeaderValue, header, request::Parts},
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

    pub(crate) async fn check(self, state: &AppState, topic: &str) -> Result<()> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM topics WHERE id=$1 AND enabled AND owner_user_id IS NULL AND (visible OR $2))",
        ).bind(topic).bind(self.is_admin()).fetch_one(&state.pool).await?;
        allowed
            .then_some(())
            .ok_or_else(|| AppError::NotFound("主题不存在".into()))
    }

    pub(crate) fn path(self, path: &str) -> String {
        if self.is_admin() && path != "/" && !path.starts_with("/?") {
            format!("{PREFIX}{path}")
        } else {
            path.to_owned()
        }
    }

    pub(crate) fn localized(self, path: &str, en: bool) -> String {
        site::localized(&self.path(path), en)
    }
}

pub(crate) fn public(routes: Router<AppState>) -> Router<AppState> {
    routes
        .layer(middleware::from_fn(redirect_admin))
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

// The role cookie only chooses a URL; the preview endpoint verifies the real session.
async fn redirect_admin(request: Request, next: Next) -> Response {
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
    if html && admin {
        let mut response =
            Redirect::temporary(&format!("{PREFIX}{}", request.uri())).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
        response
    } else {
        next.run(request).await
    }
}
