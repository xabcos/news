//! Workspace view of the catalogue: each provider's standing, pages and last check, recent
//! changes, and switches to pause a provider or hide a model. Values and providers are
//! never edited here; they come only from the providers' pages and the weekly pass.
use crate::{
    auth,
    error::{ApiJson, AppError, Result},
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, patch},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, types::Json as Jsonb};
use std::collections::HashSet;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/admin/llm", get(overview))
        .route("/api/admin/llm/vendors/{id}", patch(update_vendor))
        .route(
            "/api/admin/llm/models/{vendor}/{model}",
            patch(update_model),
        )
}

#[derive(Serialize, FromRow)]
struct VendorStatus {
    id: String,
    name: String,
    region: String,
    homepage: String,
    enabled: bool,
    /// `active`, `candidate` or `offline`.
    state: String,
    /// `seed` or `discovered`.
    origin: String,
    state_changed_at: Option<DateTime<Utc>>,
    held_since: Option<DateTime<Utc>>,
    checked_at: Option<DateTime<Utc>>,
    verified_at: Option<DateTime<Utc>>,
    error: Option<String>,
    models: i64,
    sources: Jsonb<Value>,
}

#[derive(Serialize, FromRow)]
struct ModelStatus {
    vendor_id: String,
    model_id: String,
    name: String,
    status: String,
    hidden: bool,
    removed_at: Option<DateTime<Utc>>,
    verified_at: DateTime<Utc>,
    #[serde(skip)]
    priced: bool,
    #[serde(skip)]
    listed: bool,
    /// Whether the public page shows it: the newest priced version of its line.
    #[sqlx(skip)]
    shown: bool,
}

#[derive(Serialize, FromRow)]
struct ChangeRow {
    id: i64,
    vendor_id: String,
    model_id: Option<String>,
    kind: String,
    before: Option<Jsonb<Value>>,
    after: Option<Jsonb<Value>>,
    source_url: String,
    detected_at: DateTime<Utc>,
}

async fn overview(State(state): State<AppState>, user: auth::User) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let vendors: Vec<VendorStatus> = sqlx::query_as(
        "SELECT v.id, v.name, v.region, v.homepage, v.enabled, v.state, v.origin,
                v.state_changed_at, v.held_since, v.checked_at, v.verified_at, v.error,
                (SELECT COUNT(*) FROM llm_models m WHERE m.vendor_id = v.id AND m.removed_at IS NULL) AS models,
                COALESCE((SELECT jsonb_agg(jsonb_build_object(
                    'id', s.id, 'kind', s.kind, 'url', s.url, 'currency', s.currency,
                    'checked_at', s.checked_at, 'changed_at', s.changed_at, 'failures', s.failures, 'error', s.error
                ) ORDER BY s.sort_weight, s.id) FROM llm_sources s WHERE s.vendor_id = v.id), '[]') AS sources
         FROM llm_vendors v
         ORDER BY CASE v.state WHEN 'active' THEN 0 WHEN 'candidate' THEN 1 ELSE 2 END,
                  v.sort_weight, v.id",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut models: Vec<ModelStatus> = sqlx::query_as(
        "SELECT m.vendor_id, m.model_id, m.name, m.status, m.hidden, m.removed_at, m.verified_at,
                jsonb_array_length(m.prices) > 0 AS priced,
                v.enabled AND v.state = 'active' AND NOT m.hidden AND m.removed_at IS NULL AS listed
         FROM llm_models m JOIN llm_vendors v ON v.id = m.vendor_id
         ORDER BY v.sort_weight, m.removed_at IS NOT NULL, m.model_id",
    )
    .fetch_all(&state.pool)
    .await?;
    let shown: HashSet<(String, String)> = super::current(&models, |model| {
        (
            model.vendor_id.as_str(),
            model.model_id.as_str(),
            model.status.as_str(),
            model.priced && model.listed,
        )
    })
    .into_iter()
    .map(|(vendor, model)| (vendor.to_owned(), model.to_owned()))
    .collect();
    for model in &mut models {
        model.shown = shown.contains(&(model.vendor_id.clone(), model.model_id.clone()));
    }
    let changes: Vec<ChangeRow> = sqlx::query_as(
        "SELECT id, vendor_id, model_id, kind, before, after, source_url, detected_at
         FROM llm_changes ORDER BY detected_at DESC, id DESC LIMIT 30",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        json!({"vendors": vendors, "models": models, "changes": changes}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VendorPatch {
    enabled: Option<bool>,
}

async fn update_vendor(
    State(state): State<AppState>,
    user: auth::User,
    Path(id): Path<String>,
    ApiJson(patch): ApiJson<VendorPatch>,
) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let updated =
        sqlx::query("UPDATE llm_vendors SET enabled = COALESCE($2, enabled) WHERE id = $1")
            .bind(&id)
            .bind(patch.enabled)
            .execute(&state.pool)
            .await?
            .rows_affected();
    if updated == 0 {
        return Err(AppError::NotFound("服务商不存在".into()));
    }
    Ok(Json(json!({"id": id})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelPatch {
    hidden: Option<bool>,
}

async fn update_model(
    State(state): State<AppState>,
    user: auth::User,
    Path((vendor, model)): Path<(String, String)>,
    ApiJson(patch): ApiJson<ModelPatch>,
) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let updated = sqlx::query(
        "UPDATE llm_models SET hidden = COALESCE($3, hidden) WHERE vendor_id = $1 AND model_id = $2",
    )
    .bind(&vendor)
    .bind(&model)
    .bind(patch.hidden)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if updated == 0 {
        return Err(AppError::NotFound("模型不存在".into()));
    }
    Ok(Json(json!({"vendor_id": vendor, "model_id": model})))
}
