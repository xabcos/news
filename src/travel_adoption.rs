//! First meetings: upload, create a resumable draft, then confirm its exact identity.
use super::{CompanionRow, autonomous_tick, load_companion, travel_select_sql, travel_view};
use crate::{
    auth::User,
    error::{ApiJson, AppError, Result},
    state::AppState,
    travel_identity::IDENTITY_VERSION,
};
use axum::{
    Json,
    extract::{Multipart, State},
};
use chrono::{DateTime, Utc};
use getrandom::fill;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfirmAdoptionRequest {
    draft_id: Uuid,
}

#[derive(Debug, FromRow)]
pub(super) struct DraftRow {
    id: Uuid,
    identity_version: String,
    identity_seed: Vec<u8>,
    identity: Option<Value>,
    initial_accessories: Value,
    creation_spec: Value,
    request_fingerprint: Option<Vec<u8>>,
    request_id: Option<Uuid>,
    stage: String,
    generation_error: Option<String>,
    portrait_asset_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

impl DraftRow {
    fn reuse_for(&self, request_id: Option<Uuid>, fingerprint: &[u8]) -> Result<bool> {
        if self.identity_version != IDENTITY_VERSION {
            return Ok(false);
        }
        let same_content = self.request_fingerprint.as_deref() == Some(fingerprint);
        if let Some(request_id) = request_id {
            if self.request_id != Some(request_id) {
                return Ok(false);
            }
            if !same_content {
                return Err(AppError::Conflict(
                    "这次创建请求的内容已改变，请重新提交".into(),
                ));
            }
            // Retrying a request observes its result, including failure. Only a
            // new request ID authorizes another generation with the same input.
            return Ok(true);
        }
        // Cached clients can finish their existing meeting without a request ID.
        Ok(same_content
            && self.stage != "failed"
            && (self.stage == "ready"
                || Utc::now() - self.created_at < chrono::Duration::minutes(25)))
    }
}

pub(super) async fn load_draft(pool: &PgPool, user_id: i64) -> Result<Option<DraftRow>> {
    Ok(sqlx::query_as(draft_select_sql())
        .bind(user_id)
        .fetch_optional(pool)
        .await?)
}

pub(crate) async fn draft_status(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let draft = load_draft(&state.pool, user.id).await?;
    Ok(Json(
        draft
            .map(draft_view)
            .unwrap_or_else(|| json!({"status":"empty"})),
    ))
}

fn draft_select_sql() -> &'static str {
    "SELECT id, identity_version, identity_seed, identity, initial_accessories, creation_spec, request_fingerprint, request_id, stage, generation_error, portrait_asset_id, created_at FROM travel_adoption_drafts WHERE user_id = $1"
}

pub(crate) async fn preview(
    State(state): State<AppState>,
    user: User,
    multipart: std::result::Result<Multipart, axum::extract::multipart::MultipartRejection>,
) -> Result<Json<Value>> {
    use crate::travel_assets::{MAX_REFERENCES, MAX_UPLOAD_BYTES, validate_upload};
    if load_companion(&state.pool, user.id).await?.is_some() {
        return Err(AppError::Conflict("你已经有一位同行伙伴".into()));
    }
    let permit = state
        .travel_identity_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests("新朋友正在准备中，请稍后再试".into()))?;
    let mut multipart = multipart
        .map_err(|_| AppError::UnsupportedMediaType("请通过初遇页面提交形象资料".into()))?;
    let mut fields = std::collections::BTreeMap::new();
    let mut uploads = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| AppError::BadRequest("上传内容不完整，请重新选择照片".into()))?
    {
        let key = field.name().unwrap_or_default().to_owned();
        if key == "photos" {
            if uploads.len() >= MAX_REFERENCES {
                return Err(AppError::BadRequest("最多上传 7 张形象照片".into()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| AppError::BadRequest("照片上传中断，请重试".into()))?
            {
                if bytes.len().saturating_add(chunk.len()) > MAX_UPLOAD_BYTES {
                    return Err(AppError::PayloadTooLarge("每张照片最大 10 MB".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            uploads.push(validate_upload(bytes).await?);
        } else {
            if !matches!(
                key.as_str(),
                "source" | "gender" | "name" | "description" | "consent" | "request_id"
            ) || fields.contains_key(&key)
            {
                return Err(AppError::BadRequest("相遇资料包含重复或未知字段".into()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = field
                .chunk()
                .await
                .map_err(|_| AppError::BadRequest("相遇资料不完整".into()))?
            {
                if bytes.len().saturating_add(chunk.len()) > 4096 {
                    return Err(AppError::BadRequest("人物描述过长".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            fields.insert(
                key,
                String::from_utf8(bytes)
                    .map_err(|_| AppError::BadRequest("人物描述编码无效".into()))?
                    .trim()
                    .to_owned(),
            );
        }
    }
    let mut take = |key: &str| fields.remove(key).filter(|v| !v.is_empty());
    let request_id = take("request_id")
        .map(|value| Uuid::parse_str(&value))
        .transpose()
        .map_err(|_| AppError::BadRequest("创建请求编号无效，请刷新页面后重试".into()))?;
    let specification = crate::travel_identity::IdentityRequest {
        source: take("source").unwrap_or_default(),
        gender: take("gender").unwrap_or_default(),
        name: take("name"),
        description: take("description"),
    };
    let consent = take("consent");
    specification.validate(uploads.len())?;
    if specification.source == "photos" && consent.as_deref() != Some("true") {
        return Err(AppError::BadRequest("请确认你有权使用这些照片".into()));
    }
    let specification_json = serde_json::to_value(&specification)?;
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&specification_json)?);
    for upload in &uploads {
        hasher.update(&upload.checksum);
    }
    let fingerprint = hasher.finalize().to_vec();
    let mut seed = [0_u8; 32];
    fill(&mut seed).map_err(|_| AppError::Internal("无法准备相遇".into()))?;
    let draft_id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    if let Some(existing) = sqlx::query_as::<_, DraftRow>(draft_select_sql())
        .bind(user.id)
        .fetch_optional(&mut *tx)
        .await?
    {
        if existing.reuse_for(request_id, &fingerprint)? {
            tx.commit().await?;
            return Ok(Json(draft_view(existing)));
        }
        // Only drafts may be replaced. Confirmed people are never recreated here.
        sqlx::query("DELETE FROM travel_adoption_drafts WHERE user_id=$1")
            .bind(user.id)
            .execute(&mut *tx)
            .await?;
    }
    let adopted: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM travel_companions WHERE owner_user_id=$1)")
            .bind(user.id)
            .fetch_one(&mut *tx)
            .await?;
    if adopted {
        return Err(AppError::Conflict("你已经开始同行，请刷新页面".into()));
    }
    sqlx::query("INSERT INTO travel_adoption_drafts(user_id,id,identity_version,identity_seed,creation_spec,request_fingerprint,request_id) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(user.id).bind(draft_id).bind(IDENTITY_VERSION).bind(seed.as_slice()).bind(specification_json).bind(fingerprint).bind(request_id).execute(&mut *tx).await?;
    for (ordinal, upload) in uploads.iter().enumerate() {
        sqlx::query("INSERT INTO travel_identity_assets(id,draft_id,role,ordinal,content_type,image_content_type,original_bytes,image_bytes,sha256,provenance) VALUES($1,$2,'source',$3,$4,$5,$6,$7,$8,$9)")
            .bind(Uuid::new_v4()).bind(draft_id).bind(ordinal as i16).bind(&upload.content_type).bind(&upload.reference.content_type)
            .bind(&upload.original).bind(&upload.reference.bytes).bind(&upload.checksum).bind(json!({"origin":"user_upload","consent":"authorized_subject"})).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    state.clone().spawn_worker(async move {
        let outcome = tokio::select! {
            _ = state.shutdown.cancelled() => Err(AppError::ServiceUnavailable("服务正在关闭".into())),
            outcome = prepare_first_meeting(&state, draft_id, &seed, &specification) => outcome,
        };
        if let Err(error) = outcome {
            tracing::warn!(draft_id=%draft_id,"Along first meeting was not completed");
            let public_error = match error {
                AppError::BadRequest(message) => message.to_string(),
                _ => "这次相遇还没准备好，请稍后重新尝试".into(),
            };
            let _ = sqlx::query(
                "UPDATE travel_adoption_drafts SET stage='failed',generation_error=$2 WHERE id=$1",
            )
            .bind(draft_id)
            .bind(public_error)
            .execute(&state.pool)
            .await;
        }
        drop(permit);
    });
    Ok(Json(
        json!({"status":"preparing","draft_id":draft_id,"request_id":request_id,"stage":"identity"}),
    ))
}

async fn prepare_first_meeting(
    state: &AppState,
    draft_id: Uuid,
    seed: &[u8],
    specification: &crate::travel_identity::IdentityRequest,
) -> Result<()> {
    let references = crate::travel_assets::draft_references(&state.pool, draft_id).await?;
    let preview =
        crate::travel_identity::generate_identity(state, seed, specification, &references).await?;
    let updated=sqlx::query("UPDATE travel_adoption_drafts SET identity=$2,initial_accessories=$3,stage='portrait' WHERE id=$1")
        .bind(draft_id).bind(&preview.identity).bind(&preview.initial_accessories).execute(&state.pool).await?;
    if updated.rows_affected() == 0 {
        return Ok(());
    }
    let (bytes, content_type) = crate::travel_image::generate_first_portrait(
        state,
        &preview.identity,
        &preview.initial_accessories,
        &references,
    )
    .await?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM travel_adoption_drafts WHERE id=$1)")
            .bind(draft_id)
            .fetch_one(&state.pool)
            .await?;
    if !exists {
        return Ok(());
    }
    let asset_id=crate::travel_assets::store_draft_portrait(state,draft_id,bytes,&content_type,json!({"model":state.config.travel_image_model,"origin":"first_meeting","identity_version":IDENTITY_VERSION})).await?;
    sqlx::query("UPDATE travel_adoption_drafts SET portrait_asset_id=$2,stage='ready',generation_error=NULL WHERE id=$1")
        .bind(draft_id).bind(asset_id).execute(&state.pool).await?;
    Ok(())
}

pub(crate) async fn confirm(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<ConfirmAdoptionRequest>,
) -> Result<Json<Value>> {
    let mut transaction = state.pool.begin().await?;

    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
    let draft = sqlx::query_as::<_, DraftRow>(&format!("{} FOR UPDATE", draft_select_sql()))
        .bind(user.id)
        .fetch_optional(&mut *transaction)
        .await?;

    let Some(draft) = draft else {
        let existing = sqlx::query_as::<_, CompanionRow>(travel_select_sql())
            .bind(user.id)
            .fetch_optional(&mut *transaction)
            .await?;
        transaction.commit().await?;
        return match existing {
            Some(companion) => Ok(Json(travel_view(&state.pool, companion).await?)),
            None => Err(AppError::Conflict("相遇预览不存在或已过期".into())),
        };
    };
    if draft.id != request.draft_id || draft.identity_version != IDENTITY_VERSION {
        return Err(AppError::Conflict("相遇预览已更新，请重新确认".into()));
    }

    if let Some(existing) = sqlx::query_as::<_, CompanionRow>(travel_select_sql())
        .bind(user.id)
        .fetch_optional(&mut *transaction)
        .await?
    {
        sqlx::query("DELETE FROM travel_adoption_drafts WHERE user_id = $1")
            .bind(user.id)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        return Ok(Json(travel_view(&state.pool, existing).await?));
    }

    if draft.stage != "ready" || draft.portrait_asset_id.is_none() {
        return Err(AppError::Conflict("初见形象尚未准备好，请稍候".into()));
    }
    let identity = draft
        .identity
        .as_ref()
        .ok_or_else(|| AppError::Conflict("人物资料尚未完成".into()))?;
    let assets=sqlx::query_as::<_,crate::travel_assets::IdentityAsset>("SELECT id,role,ordinal,content_type,image_content_type,original_bytes,image_bytes,sha256 FROM travel_identity_assets WHERE draft_id=$1 ORDER BY CASE role WHEN 'portrait' THEN 0 ELSE 1 END,ordinal")
        .bind(draft.id).fetch_all(&mut *transaction).await?;
    crate::travel_assets::verify_assets(&assets)?;
    let archive_assets = assets
        .iter()
        .map(|asset| asset.archive())
        .collect::<Vec<_>>();
    crate::travel_archive::write_identity_archive(
        &state.config.travel_archive_dir,
        draft.id,
        identity,
        &draft.initial_accessories,
        &archive_assets,
    )
    .await?;
    let fingerprint = identity_fingerprint(
        &draft.identity_version,
        &draft.identity_seed,
        identity,
        &draft.initial_accessories,
    );
    let companion_id = draft.id;
    let companion = sqlx::query_as::<_, CompanionRow>(
        r#"
        INSERT INTO travel_companions (
            id, owner_user_id, name, identity_version, identity_seed,
            identity_fingerprint, identity, initial_accessories, gender, last_seen_at,identity_archive_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW(),NOW())
        ON CONFLICT (owner_user_id) DO NOTHING
            RETURNING id, name, gender, identity_version,
                  identity, initial_accessories,
                  adopted_at, name_updated_at, last_seen_at
        "#,
    )
    .bind(companion_id)
    .bind(user.id)
    .bind(identity["name_seed"].as_str().unwrap_or("旅伴"))
    .bind(&draft.identity_version)
    .bind(&draft.identity_seed)
    .bind(fingerprint)
    .bind(identity)
    .bind(&draft.initial_accessories)
    .bind(
        identity["gender"]
            .as_str()
            .or_else(|| draft.creation_spec["gender"].as_str())
            .unwrap_or("female"),
    )
    .fetch_optional(&mut *transaction)
    .await;
    let companion = match companion {
        Err(sqlx::Error::Database(error))
            if error.constraint() == Some("travel_companions_appearance_unique") =>
        {
            transaction.rollback().await?;
            sqlx::query("DELETE FROM travel_adoption_drafts WHERE id = $1")
                .bind(draft.id)
                .execute(&state.pool)
                .await?;
            return Err(AppError::Conflict(
                "这份外观已被选定，请重新预览一位旅伴".into(),
            ));
        }
        result => result?,
    };

    let companion = match companion {
        Some(companion) => companion,
        None => {
            let existing = sqlx::query_as::<_, CompanionRow>(travel_select_sql())
                .bind(user.id)
                .fetch_one(&mut *transaction)
                .await?;
            sqlx::query("DELETE FROM travel_adoption_drafts WHERE user_id = $1")
                .bind(user.id)
                .execute(&mut *transaction)
                .await?;
            transaction.commit().await?;
            return Ok(Json(travel_view(&state.pool, existing).await?));
        }
    };

    sqlx::query(
        r#"
        INSERT INTO travel_growth_events (id, companion_id, event_type, source_key, payload)
        VALUES ($1, $2, 'adopted', $3, $4)
        ON CONFLICT (companion_id, event_type, source_key) DO NOTHING
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(companion.id)
    .bind(format!("adoption:{}", companion.id))
    .bind(json!({ "identity_version": companion.identity_version }))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO travel_autonomy_state (companion_id, next_trip_at, next_scene_at) VALUES ($1, NOW() + INTERVAL '6 hours', NOW() + INTERVAL '4 hours') ON CONFLICT (companion_id) DO NOTHING",
    )
    .bind(companion.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO travel_messages (id, companion_id, kind, dedupe_key, payload) VALUES ($1, $2, 'welcome', $3, $4) ON CONFLICT (dedupe_key) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(companion.id)
    .bind(format!("welcome:{}", companion.id))
    .bind(json!({
        "title": "欢迎来到旅居空间",
        "text": format!("{}记住了你们第一次相遇。往后的日子，一起看见更大的世界。", companion.name),
        "postcard_url": format!("/api/travel/identity/assets/{}",draft.portrait_asset_id.unwrap()),
    }))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE travel_identity_assets SET companion_id=$2,draft_id=NULL WHERE draft_id=$1",
    )
    .bind(draft.id)
    .bind(companion.id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM travel_adoption_drafts WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;

    if let Err(error) = autonomous_tick(&state).await {
        tracing::warn!(%error, "initial travel photo deferred to scheduler");
    }
    Ok(Json(travel_view(&state.pool, companion).await?))
}

pub(super) fn draft_view(draft: DraftRow) -> Value {
    let stale = draft.stage != "ready"
        && draft.stage != "failed"
        && Utc::now() - draft.created_at > chrono::Duration::minutes(25);
    let status = if stale || draft.stage == "failed" {
        "failed"
    } else if draft.stage == "ready" {
        "ready"
    } else {
        "preparing"
    };
    json!({"status":status,"stage":draft.stage,"draft_id":draft.id,"request_id":draft.request_id,"created_at":draft.created_at,
        "identity_version":draft.identity_version,"identity":draft.identity,"initial_accessories":draft.initial_accessories,
        "source":draft.creation_spec["source"],"gender":draft.creation_spec["gender"],
        "portrait_url":draft.portrait_asset_id.map(|id|format!("/api/travel/identity/assets/{id}")),
        "error":if stale {Some("这次相遇已暂停，请重新准备".to_owned())} else {draft.generation_error}})
}

fn identity_fingerprint(
    version: &str,
    seed: &[u8],
    identity: &Value,
    initial_accessories: &Value,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(version.as_bytes());
    hasher.update([0]);
    hasher.update(seed);
    hasher.update([0]);
    hasher.update(serde_json::to_vec(identity).expect("identity is serialisable"));
    hasher.update([0]);
    hasher.update(serde_json::to_vec(initial_accessories).expect("accessories are serialisable"));
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(request_id: Option<Uuid>, stage: &str) -> DraftRow {
        DraftRow {
            id: Uuid::new_v4(),
            identity_version: IDENTITY_VERSION.into(),
            identity_seed: vec![1; 32],
            identity: None,
            initial_accessories: json!([]),
            creation_spec: json!({"source":"ai","gender":"male"}),
            request_fingerprint: Some(vec![7; 32]),
            request_id,
            stage: stage.into(),
            generation_error: None,
            portrait_asset_id: None,
            created_at: Utc::now() - chrono::Duration::days(2),
        }
    }

    #[test]
    fn retries_observe_the_same_request_but_new_actions_can_recreate() {
        let request_id = Uuid::new_v4();
        for stage in ["identity", "portrait", "ready", "failed"] {
            let saved = draft(Some(request_id), stage);
            assert!(saved.reuse_for(Some(request_id), &[7; 32]).unwrap());
            assert!(!saved.reuse_for(Some(Uuid::new_v4()), &[7; 32]).unwrap());
            assert!(matches!(
                saved.reuse_for(Some(request_id), &[8; 32]),
                Err(AppError::Conflict(_))
            ));
        }
    }

    #[test]
    fn cached_clients_can_resume_legacy_drafts() {
        assert!(draft(None, "ready").reuse_for(None, &[7; 32]).unwrap());
        assert!(!draft(None, "failed").reuse_for(None, &[7; 32]).unwrap());
        assert!(!draft(None, "portrait").reuse_for(None, &[7; 32]).unwrap());
        assert_eq!(draft_view(draft(None, "portrait"))["status"], "failed");
    }

    #[tokio::test]
    #[ignore = "requires an explicitly named disposable PostgreSQL database"]
    async fn postgres_adoption_reads_a_legacy_companion_without_a_portrait() {
        let url = std::env::var("ALONG_WORLD_TEST_DATABASE_URL").expect("disposable database URL");
        let pool = PgPool::connect(&url).await.unwrap();
        let database: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(database.starts_with("along_world_audit_"));
        let id = Uuid::new_v4();
        let owner: i64 = sqlx::query_scalar("INSERT INTO users(email) VALUES($1) RETURNING id")
            .bind(format!("legacy-{id}@example.test"))
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO travel_companions(id,owner_user_id,name,identity_version,identity_seed,identity_fingerprint,identity,initial_accessories) VALUES($1,$2,'旧旅伴','legacy-test',$3,$3,'{}','[]')")
            .bind(id).bind(owner).bind(id.as_bytes().repeat(2))
            .execute(&pool).await.unwrap();
        let companion = load_companion(&pool, owner).await.unwrap().unwrap();
        let view = travel_view(&pool, companion).await.unwrap();
        assert_eq!(view["id"], json!(id));
        assert!(view["portrait_url"].is_null());
        assert!(view["scene"].is_object());
    }

    #[test]
    fn identity_fingerprint_covers_the_saved_ai_draft() {
        let identity = serde_json::json!({"age":23,"face":{"shape":"unique AI face"}});
        let accessories = serde_json::json!([]);
        let first = identity_fingerprint("v4", &[7; 32], &identity, &accessories);
        assert_eq!(
            first,
            identity_fingerprint("v4", &[7; 32], &identity, &accessories)
        );
        assert_ne!(
            first,
            identity_fingerprint("v4", &[8; 32], &identity, &accessories)
        );
        assert_ne!(
            first,
            identity_fingerprint("v4", &[7; 32], &serde_json::json!({"age":24}), &accessories)
        );
    }
}
