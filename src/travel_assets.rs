//! Private, immutable identity material. Original uploads never go to public media.
use crate::{
    auth::User,
    error::{AppError, Result},
    state::AppState,
    travel_archive::ArchiveAsset,
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Response, header},
};
use image::{ImageDecoder, ImageFormat, ImageReader, Limits, codecs::jpeg::JpegEncoder};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use std::io::Cursor;
use uuid::Uuid;

pub(crate) const MAX_REFERENCES: usize = 7;
pub(crate) const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_UPLOAD_BODY: usize = MAX_REFERENCES * MAX_UPLOAD_BYTES + 64 * 1024;

#[derive(Debug, Clone, FromRow)]
pub(crate) struct ReferenceImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) content_type: String,
}

#[derive(Debug)]
pub(crate) struct ValidatedUpload {
    pub(crate) original: Vec<u8>,
    pub(crate) reference: ReferenceImage,
    pub(crate) content_type: String,
    pub(crate) checksum: Vec<u8>,
}

pub(crate) async fn validate_upload(bytes: Vec<u8>) -> Result<ValidatedUpload> {
    if bytes.is_empty() || bytes.len() > MAX_UPLOAD_BYTES {
        return Err(AppError::PayloadTooLarge(
            "每张参考照片需在 10 MB 以内".into(),
        ));
    }
    tokio::task::spawn_blocking(move || normalize_upload(bytes))
        .await
        .map_err(|_| AppError::Internal("照片处理任务中断".into()))?
}

fn normalize_upload(bytes: Vec<u8>) -> Result<ValidatedUpload> {
    let invalid = || AppError::BadRequest("请使用清晰有效的 JPEG、PNG 或 WebP 照片".into());
    let format = image::guess_format(&bytes).map_err(|_| invalid())?;
    let content_type = match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Png => "image/png",
        ImageFormat::WebP => "image/webp",
        _ => return Err(invalid()),
    };
    let mut reader = ImageReader::with_format(Cursor::new(&bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| invalid())?;
    let (width, height) = decoder.dimensions();
    if width < 256 || height < 256 || u64::from(width) * u64::from(height) > 32_000_000 {
        return Err(AppError::BadRequest(
            "照片短边至少 256 像素，总像素不超过 3200 万".into(),
        ));
    }
    let orientation = decoder.orientation().map_err(|_| invalid())?;
    let mut decoded = image::DynamicImage::from_decoder(decoder).map_err(|_| invalid())?;
    decoded.apply_orientation(orientation);
    if decoded.width() > 1536 || decoded.height() > 1536 {
        decoded = decoded.resize(1536, 1536, image::imageops::FilterType::Lanczos3);
    }
    let rgb = decoded.to_rgb8();
    let mut normalized = Vec::new();
    JpegEncoder::new_with_quality(&mut normalized, 92)
        .encode_image(&rgb)
        .map_err(|_| invalid())?;
    let checksum = Sha256::digest(&bytes).to_vec();
    Ok(ValidatedUpload {
        original: bytes,
        reference: ReferenceImage {
            bytes: normalized,
            content_type: "image/jpeg".into(),
        },
        content_type: content_type.into(),
        checksum,
    })
}

#[derive(Debug, FromRow)]
pub(crate) struct IdentityAsset {
    pub(crate) id: Uuid,
    pub(crate) role: String,
    pub(crate) ordinal: i16,
    pub(crate) content_type: String,
    pub(crate) image_content_type: String,
    pub(crate) original_bytes: Vec<u8>,
    pub(crate) image_bytes: Vec<u8>,
    pub(crate) sha256: Vec<u8>,
}
impl IdentityAsset {
    pub(crate) fn archive(&self) -> ArchiveAsset {
        ArchiveAsset {
            id: self.id,
            role: self.role.clone(),
            ordinal: self.ordinal,
            content_type: self.content_type.clone(),
            image_content_type: self.image_content_type.clone(),
            original_bytes: self.original_bytes.clone(),
            image_bytes: self.image_bytes.clone(),
            sha256: self.sha256.clone(),
        }
    }
}

pub(crate) async fn companion_assets(pool: &PgPool, id: Uuid) -> Result<Vec<IdentityAsset>> {
    let rows=sqlx::query_as::<_,IdentityAsset>("SELECT id,role,ordinal,content_type,image_content_type,original_bytes,image_bytes,sha256 FROM travel_identity_assets WHERE companion_id=$1 ORDER BY CASE role WHEN 'portrait' THEN 0 ELSE 1 END,ordinal")
        .bind(id).fetch_all(pool).await?;
    verify_assets(&rows)?;
    Ok(rows)
}
// Original bytes are validated on ingestion, frozen by database constraints,
// and fully checked when confirming or archiving. Generation only needs the
// normalized references; it must not load every private original into memory.
pub(crate) async fn portrait_reference(pool: &PgPool, id: Uuid) -> Result<Option<ReferenceImage>> {
    Ok(sqlx::query_as("SELECT image_bytes AS bytes,image_content_type AS content_type FROM travel_identity_assets WHERE companion_id=$1 AND role='portrait' AND ordinal=0")
        .bind(id).fetch_optional(pool).await?)
}

pub(crate) async fn companion_references(pool: &PgPool, id: Uuid) -> Result<Vec<ReferenceImage>> {
    if let Some(portrait) = portrait_reference(pool, id).await? {
        return Ok(vec![portrait]);
    }
    Ok(sqlx::query_as("SELECT image_bytes AS bytes,image_content_type AS content_type FROM travel_identity_assets WHERE companion_id=$1 AND role='source' ORDER BY ordinal")
        .bind(id).fetch_all(pool).await?)
}

pub(crate) async fn draft_references(pool: &PgPool, id: Uuid) -> Result<Vec<ReferenceImage>> {
    Ok(sqlx::query_as("SELECT image_bytes AS bytes,image_content_type AS content_type FROM travel_identity_assets WHERE draft_id=$1 AND role='source' ORDER BY ordinal")
        .bind(id).fetch_all(pool).await?)
}
pub(crate) fn verify_assets(assets: &[IdentityAsset]) -> Result<()> {
    for asset in assets {
        if Sha256::digest(&asset.original_bytes).as_slice() != asset.sha256 {
            return Err(AppError::Internal(
                "人物原图校验失败，请从档案副本恢复".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) async fn store_draft_portrait(
    state: &AppState,
    draft_id: Uuid,
    bytes: Vec<u8>,
    content_type: &str,
    provenance: Value,
) -> Result<Uuid> {
    // AI outputs are bounded and decoded too, never accepted from a MIME label alone.
    let checked = validate_upload(bytes).await?;
    if checked.content_type != content_type {
        return Err(AppError::Internal("形象图片格式不一致".into()));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO travel_identity_assets(id,draft_id,role,ordinal,content_type,image_content_type,original_bytes,image_bytes,sha256,provenance) VALUES($1,$2,'portrait',0,$3,$4,$5,$6,$7,$8) ON CONFLICT(draft_id,role,ordinal) DO NOTHING")
        .bind(id).bind(draft_id).bind(&checked.content_type).bind(&checked.reference.content_type).bind(checked.original)
        .bind(checked.reference.bytes).bind(checked.checksum).bind(provenance).execute(&state.pool).await?;
    Ok(sqlx::query_scalar(
        "SELECT id FROM travel_identity_assets WHERE draft_id=$1 AND role='portrait' AND ordinal=0",
    )
    .bind(draft_id)
    .fetch_one(&state.pool)
    .await?)
}

pub(crate) async fn identity_image(
    State(state): State<AppState>,
    user: User,
    Path(asset_id): Path<Uuid>,
) -> Result<Response<Body>> {
    let row = sqlx::query_as::<_, (Vec<u8>, String)>(
        r#"SELECT CASE WHEN a.role='portrait' THEN a.original_bytes ELSE a.image_bytes END,
        CASE WHEN a.role='portrait' THEN a.content_type ELSE a.image_content_type END
        FROM travel_identity_assets a LEFT JOIN travel_adoption_drafts d ON d.id=a.draft_id
        LEFT JOIN travel_companions c ON c.id=a.companion_id
        WHERE a.id=$1 AND (d.user_id=$2 OR c.owner_user_id=$2)"#,
    )
    .bind(asset_id)
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("形象照片不存在".into()))?;
    image_response(row.0, &row.1)
}

pub(crate) async fn media_image(
    State(state): State<AppState>,
    user: User,
    Path(asset_id): Path<Uuid>,
) -> Result<Response<Body>> {
    let row=sqlx::query_as::<_,(Vec<u8>,String)>(r#"SELECT a.image_bytes,a.content_type FROM travel_media_assets a
        WHERE a.id=$1 AND (EXISTS(SELECT 1 FROM travel_companions c WHERE c.id=a.companion_id AND c.owner_user_id=$2)
        OR EXISTS(SELECT 1 FROM travel_shared_event_members m JOIN travel_companions c ON c.id=m.companion_id
                  WHERE m.event_id=a.shared_event_id AND c.owner_user_id=$2))"#)
        .bind(asset_id).bind(user.id).fetch_optional(&state.pool).await?
        .ok_or_else(||AppError::NotFound("纪念照片不存在".into()))?;
    image_response(row.0, &row.1)
}
fn image_response(bytes: Vec<u8>, content_type: &str) -> Result<Response<Body>> {
    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .map_err(|_| AppError::Internal("无法读取形象照片".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_and_preserves_original_without_passing_metadata_to_ai() {
        let original = image::DynamicImage::new_rgb8(400, 300);
        let mut data = Cursor::new(Vec::new());
        original.write_to(&mut data, ImageFormat::Png).unwrap();
        let data = data.into_inner();
        let result = normalize_upload(data.clone()).unwrap();
        assert_eq!(result.original, data);
        assert_eq!(result.content_type, "image/png");
        assert_eq!(result.reference.content_type, "image/jpeg");
        assert_eq!(result.checksum, Sha256::digest(&data).to_vec());
        assert_eq!(
            image::load_from_memory(&result.reference.bytes)
                .unwrap()
                .width(),
            400
        );
    }
    #[test]
    fn rejects_fake_image_signatures_and_tiny_images() {
        assert!(normalize_upload(b"\x89PNG\r\n\x1a\nnot a real PNG".to_vec()).is_err());
        let mut data = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(8, 8)
            .write_to(&mut data, ImageFormat::Png)
            .unwrap();
        assert!(normalize_upload(data.into_inner()).is_err());
    }
}
