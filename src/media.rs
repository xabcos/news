//! Server-side mirroring of news photographs into tmpfile.
//!
//! News sources often put images behind hotlink protection or short-lived URLs. The
//! browser therefore receives only the capability URL returned by tmpfile; the original
//! image URL is retained in the stored metadata so a later startup can renew it.

use crate::{error::Result, fetch::NewsImage, state::AppState};
use chrono::{DateTime, Utc};
use reqwest::{
    header,
    multipart::{Form, Part},
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, time::Duration};
use tokio::{task::JoinSet, time::Instant};
use url::Url;
use uuid::Uuid;

const RENEW_BEFORE_SECS: i64 = 6 * 3600;
const IMAGE_BATCH_TIMEOUT: Duration = Duration::from_secs(90);
// Tiny square thumbnails are commonly used by publishers for "image unavailable"
// placeholders. They are not useful as visual-news cards even when they return 200.
const MIN_IMAGE_BYTES: usize = 4096;
const MIN_IMAGE_DIMENSION: u32 = 320;

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
pub(crate) struct UploadResponse {
    pub direct_link: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredImageRow {
    id: Uuid,
    topic_id: String,
    day: chrono::NaiveDate,
    image: Option<Value>,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredPayloadRow {
    id: Uuid,
    payload: Option<Value>,
}

/// One transfer per origin, preserving each story's caption and attribution. Unfinished
/// images become None at the batch deadline; dropping JoinSet cancels their requests.
pub(crate) async fn mirror_images(state: &AppState, images: &mut [Option<NewsImage>]) {
    let deadline = Instant::now() + IMAGE_BATCH_TIMEOUT;
    let mut origins = HashMap::new();
    let mut groups: Vec<Vec<(usize, NewsImage)>> = Vec::new();
    for (index, slot) in images.iter_mut().enumerate() {
        let Some(image) = slot.take().and_then(NewsImage::validated) else {
            continue;
        };
        if state.config.tmpfile_base_url.is_none() {
            *slot = Some(image);
            continue;
        }
        let origin = image.origin_url.as_ref().unwrap_or(&image.url).clone();
        let group = *origins.entry(origin).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push((index, image));
    }
    let mut jobs = JoinSet::new();
    for group in groups {
        let state = state.clone();
        jobs.spawn(async move {
            let image = group
                .iter()
                .find(|(_, image)| is_fresh_tmpfile_url(&state, image))
                .unwrap_or(&group[0])
                .1
                .clone();
            (group, mirror_image(&state, image).await)
        });
    }
    loop {
        let result = tokio::select! {
            _ = state.shutdown.cancelled() => break,
            result = tokio::time::timeout_at(deadline, jobs.join_next()) => result,
        };
        match result {
            Ok(Some(Ok((group, Some(mirrored))))) => {
                for (index, mut image) in group {
                    image.url.clone_from(&mirrored.url);
                    image.origin_url.clone_from(&mirrored.origin_url);
                    image.expires_at = mirrored.expires_at;
                    image.content_hash.clone_from(&mirrored.content_hash);
                    images[index] = Some(image);
                }
            }
            Ok(Some(Ok((_, None)))) => {}
            Ok(Some(Err(error))) => tracing::warn!(%error, "image task failed"),
            Ok(None) => break,
            Err(_) => {
                tracing::debug!("image batch reached its time budget");
                break;
            }
        }
    }
}

async fn mirror_image(state: &AppState, image: NewsImage) -> Option<NewsImage> {
    if is_fresh_tmpfile_url(state, &image) {
        return Some(image);
    }
    let _permit = state.image_slots.acquire().await.ok()?;
    match tokio::time::timeout(state.config.image_timeout, transfer_image(state, image)).await {
        Ok(image) => image,
        Err(_) => {
            tracing::debug!("image transfer reached its time budget");
            None
        }
    }
}

async fn transfer_image(state: &AppState, mut image: NewsImage) -> Option<NewsImage> {
    let origin = image
        .origin_url
        .clone()
        .unwrap_or_else(|| image.url.clone());
    let (bytes, content_type) = download_image(
        state,
        &origin,
        "image/avif,image/webp,image/jpeg,image/png,image/gif",
    )
    .await?;
    if !image_is_usable(&bytes) {
        tracing::debug!(url = %origin, size = bytes.len(), "news image is too small or likely a placeholder");
        return None;
    }
    image.content_hash = Some(hex::encode(Sha256::digest(&bytes)));
    let uploaded = upload_image(state, bytes, content_type).await?;
    image.origin_url = Some(origin);
    image.url = uploaded.direct_link;
    image.expires_at = Some(uploaded.expires_at);
    Some(image)
}

/// Shared storage transport. Callers validate source bytes for their own use case.
pub(crate) async fn upload_image(
    state: &AppState,
    bytes: Vec<u8>,
    content_type: &str,
) -> Option<UploadResponse> {
    let base_url = state.config.tmpfile_base_url.as_ref()?;
    let extension = extension_for(content_type);
    let filename = format!("news-{}.{}", Uuid::new_v4(), extension);
    let part = match Part::bytes(bytes)
        .file_name(filename)
        .mime_str(content_type)
    {
        Ok(part) => part,
        Err(error) => {
            tracing::debug!(%error, "could not build tmpfile multipart image");
            return None;
        }
    };
    let upload_url = format!("{}/upload", base_url.as_str().trim_end_matches('/'));
    let response = match state
        .image_http
        .post(upload_url)
        .bearer_auth(&state.config.tmpfile_api_key)
        .multipart(Form::new().part("file", part))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::debug!(status = %response.status(), "tmpfile image upload failed");
            return None;
        }
        Err(error) => {
            tracing::debug!(%error, "tmpfile image upload failed");
            return None;
        }
    };
    let uploaded = match response.json::<UploadResponse>().await {
        Ok(uploaded) => uploaded,
        Err(error) => {
            tracing::debug!(%error, "tmpfile returned an invalid image response");
            return None;
        }
    };
    if uploaded.expires_at <= Utc::now() {
        tracing::debug!("tmpfile returned an expired image link");
        return None;
    }
    let Ok(mut direct_url) = Url::parse(&uploaded.direct_link) else {
        tracing::debug!("tmpfile returned an invalid direct link");
        return None;
    };
    // Reverse proxies may forward HTTP internally. Use the configured HTTPS origin
    // for its own links, and reject links to a different host.
    if direct_url.scheme() == "http"
        && base_url.scheme() == "https"
        && same_http_to_https_authority(&direct_url, base_url)
    {
        let _ = direct_url.set_scheme("https");
    }
    if !is_public_https_url(&direct_url) || !same_origin(&direct_url, base_url) {
        tracing::debug!("tmpfile returned a non-HTTPS direct link");
        return None;
    }
    Some(UploadResponse {
        direct_link: direct_url.to_string(),
        expires_at: uploaded.expires_at,
    })
}

/// Bounded HTTPS download shared by browser mirroring and model input preparation.
pub(crate) async fn download_image(
    state: &AppState,
    url: &str,
    accept: &str,
) -> Option<(Vec<u8>, &'static str)> {
    let mut response = match state
        .image_http
        .get(url)
        .header(header::USER_AGENT, "xabcnews.com image mirror")
        .header(header::ACCEPT, accept)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::debug!(status = %response.status(), url = %url, "news image download failed");
            return None;
        }
        Err(error) => {
            tracing::debug!(%error, url = %url, "news image download failed");
            return None;
        }
    };
    if !is_public_https_url(response.url()) {
        tracing::debug!(url = %url, "news image redirected to an insecure URL");
        return None;
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_content_type);
    let Some(content_type) = content_type else {
        tracing::debug!(url = %url, "news image has an unsupported content type");
        return None;
    };
    if response
        .content_length()
        .is_some_and(|size| size > state.config.image_max_bytes)
    {
        tracing::debug!(url = %url, "news image exceeds configured size limit");
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = match response.chunk().await {
        Ok(chunk) => chunk,
        Err(error) => {
            tracing::debug!(%error, url = %url, "reading news image failed");
            return None;
        }
    } {
        if bytes.len().saturating_add(chunk.len()) as u64 > state.config.image_max_bytes {
            tracing::debug!(url = %url, "news image exceeds configured size limit");
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some((bytes, content_type))
}

fn image_is_usable(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_IMAGE_BYTES
        && !image_dimensions(bytes).is_some_and(|(width, height)| {
            width < MIN_IMAGE_DIMENSION || height < MIN_IMAGE_DIMENSION
        })
}

/// Reads dimensions from the formats accepted above without decoding or trusting image
/// metadata. Unknown formats are allowed through; the byte-size guard still rejects the
/// tiny placeholder class this protects against.
pub(crate) fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        return Some((
            u32::from_be_bytes(bytes[16..20].try_into().ok()?),
            u32::from_be_bytes(bytes[20..24].try_into().ok()?),
        ));
    }
    if (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) && bytes.len() >= 10 {
        return Some((
            u16::from_le_bytes(bytes[6..8].try_into().ok()?) as u32,
            u16::from_le_bytes(bytes[8..10].try_into().ok()?) as u32,
        ));
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        let chunk = bytes.get(12..16)?;
        if chunk == b"VP8X" && bytes.len() >= 30 {
            let width = 1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]);
            let height = 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]);
            return Some((width, height));
        }
        if chunk == b"VP8 " {
            let start = bytes
                .windows(3)
                .position(|window| window == b"\x9d\x01\x2a")?;
            let start = start + 3;
            if bytes.len() >= start + 4 {
                let width = u16::from_le_bytes(bytes[start..start + 2].try_into().ok()?) & 0x3fff;
                let height =
                    u16::from_le_bytes(bytes[start + 2..start + 4].try_into().ok()?) & 0x3fff;
                return Some((u32::from(width), u32::from(height)));
            }
        }
    }
    if bytes.starts_with(b"\xff\xd8") {
        let mut index = 2;
        while index + 9 < bytes.len() {
            if bytes[index] != 0xff {
                index += 1;
                continue;
            }
            while index < bytes.len() && bytes[index] == 0xff {
                index += 1;
            }
            let marker = *bytes.get(index)?;
            index += 1;
            if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
                continue;
            }
            let length = u16::from_be_bytes(bytes.get(index..index + 2)?.try_into().ok()?) as usize;
            if length < 2 || index + length > bytes.len() {
                return None;
            }
            if (0xc0..=0xc3).contains(&marker)
                || (0xc5..=0xc7).contains(&marker)
                || (0xc9..=0xcb).contains(&marker)
                || (0xcd..=0xcf).contains(&marker)
            {
                let height = u16::from_be_bytes(bytes.get(index + 3..index + 5)?.try_into().ok()?);
                let width = u16::from_be_bytes(bytes.get(index + 5..index + 7)?.try_into().ok()?);
                return Some((u32::from(width), u32::from(height)));
            }
            index += length;
        }
    }
    None
}

pub(crate) fn is_public_https_url(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(url.host(), Some(url::Host::Domain(host)) if host.contains('.') && !host.ends_with(".local") && !host.ends_with(".localhost"))
        && url.username().is_empty()
        && url.password().is_none()
}

fn is_fresh_tmpfile_url(state: &AppState, image: &NewsImage) -> bool {
    let Some(base) = state.config.tmpfile_base_url.as_ref() else {
        return false;
    };
    let Ok(image_url) = Url::parse(&image.url) else {
        return false;
    };
    same_origin(&image_url, base)
        && image.expires_at.is_some_and(|expires_at| {
            expires_at > Utc::now() + chrono::Duration::seconds(RENEW_BEFORE_SECS)
        })
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme() && same_authority(left, right)
}

fn same_authority(left: &Url, right: &Url) -> bool {
    left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

/// A tmpfile response can be built from an HTTP forwarded host even when the public
/// endpoint is HTTPS. The default ports differ before the scheme upgrade, so compare
/// each URL against its own default port here and let `same_origin` re-check after the
/// upgrade.
fn same_http_to_https_authority(left: &Url, right: &Url) -> bool {
    left.host_str() == right.host_str()
        && left.port_or_known_default() == Some(80)
        && right.port_or_known_default() == Some(443)
}

fn normalize_content_type(value: &str) -> Option<&'static str> {
    let value = value.split(';').next()?.trim().to_ascii_lowercase();
    match value.as_str() {
        "image/avif" => Some("image/avif"),
        "image/webp" => Some("image/webp"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/gif" => Some("image/gif"),
        "image/bmp" => Some("image/bmp"),
        _ => None,
    }
}

fn extension_for(content_type: &str) -> &'static str {
    match content_type {
        "image/avif" => "avif",
        "image/webp" => "webp",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "jpg",
    }
}

/// Renews one week's media in one batch so stored copies of the same photograph
/// reuse a transfer. Published daily images enter the batch before archive cards.
pub async fn startup_sync(state: &AppState) -> Result<()> {
    if state.config.tmpfile_base_url.is_none() {
        return Ok(());
    }
    let keep_from = state.digest.today() - chrono::Duration::days(7);
    let rows = sqlx::query_as::<_, StoredImageRow>(
        "SELECT items.id, items.topic_id, items.day, items.image FROM items JOIN topics ON topics.id = items.topic_id WHERE items.image IS NOT NULL AND items.day >= $1 AND topics.owner_user_id IS NULL ORDER BY items.day DESC LIMIT 500",
    )
    .bind(keep_from)
    .fetch_all(&state.pool)
    .await?;
    let covers = sqlx::query_as::<_, StoredPayloadRow>(
        "SELECT runs.id, runs.payload FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id IS NULL AND jsonb_typeof(runs.payload->'cover') = 'object' AND runs.started_at >= NOW() - INTERVAL '7 days'",
    )
    .fetch_all(&state.pool)
    .await?;
    let insights = sqlx::query_as::<_, StoredPayloadRow>(
        "SELECT id, payload FROM ai_insights WHERE status = 'ok' AND payload IS NOT NULL AND started_at >= NOW() - INTERVAL '7 days' ORDER BY started_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;
    let positions = insights
        .iter()
        .map(|row| {
            row.payload
                .as_ref()
                .map_or_else(Vec::new, selected_image_positions)
        })
        .collect::<Vec<_>>();
    let mut images = insights
        .iter()
        .zip(&positions)
        .flat_map(|(row, positions)| {
            positions.iter().map(|&position| {
                stored_image(&row.payload.as_ref().unwrap()["sources"][position]["image"])
            })
        })
        .chain(
            rows.iter()
                .map(|row| row.image.as_ref().and_then(stored_image)),
        )
        .chain(covers.iter().map(|row| {
            row.payload
                .as_ref()
                .and_then(|payload| stored_image(&payload["cover"]))
        }))
        .collect::<Vec<_>>();
    mirror_images(state, &mut images).await;
    let mut images = images.into_iter();
    let mut changed_topics = std::collections::HashSet::new();
    let mut insight_changed = false;
    // Network work is finished. Persist image URLs and feed versions atomically.
    let mut tx = state.pool.begin().await?;
    for (row, positions) in insights.into_iter().zip(positions) {
        let Some(mut payload) = row.payload else {
            continue;
        };
        let mut changed = false;
        for position in positions {
            if let Some(image) = images.next().flatten() {
                changed |= update_insight_image(&mut payload, position, image)?;
            }
        }
        if changed {
            sqlx::query("UPDATE ai_insights SET payload = $2 WHERE id = $1")
                .bind(row.id)
                .bind(payload)
                .execute(&mut *tx)
                .await?;
            insight_changed = true;
        }
    }
    for row in rows {
        let Some(image) = images.next().flatten() else {
            continue;
        };
        let value = serde_json::to_value(image)?;
        if row.image.as_ref() == Some(&value) {
            continue;
        }
        sqlx::query("UPDATE items SET image = $2 WHERE id = $1")
            .bind(row.id)
            .bind(value)
            .execute(&mut *tx)
            .await?;
        changed_topics.insert((row.topic_id, row.day));
    }
    for row in covers {
        let image = images.next().flatten();
        let (Some(mut payload), Some(image)) = (row.payload, image) else {
            continue;
        };
        let value = serde_json::to_value(image)?;
        if payload.get("cover") == Some(&value) {
            continue;
        }
        payload["cover"] = value;
        sqlx::query("UPDATE runs SET payload = $2 WHERE id = $1")
            .bind(row.id)
            .bind(payload)
            .execute(&mut *tx)
            .await?;
    }
    if !changed_topics.is_empty() {
        let topics = changed_topics
            .iter()
            .map(|(topic, _)| topic.as_str())
            .collect::<Vec<_>>();
        sqlx::query("UPDATE topics SET feed_revision = feed_revision + 1 WHERE id = ANY($1)")
            .bind(topics)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    if insight_changed {
        state.insight.invalidate();
    }
    for (topic, day) in changed_topics {
        state.digest.refresh(state, &topic, day).await?;
    }
    Ok(())
}

pub(crate) fn stored_image(value: &Value) -> Option<NewsImage> {
    serde_json::from_value(value.clone()).ok()
}

fn selected_image_positions(payload: &Value) -> Vec<usize> {
    let Some(sources) = payload["sources"].as_array() else {
        return Vec::new();
    };
    let indexes = payload["analysis"]["cover_source_index"]
        .as_u64()
        .into_iter()
        .chain(
            payload["analysis"]["picture_source_indexes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64),
        )
        .collect::<std::collections::HashSet<_>>();
    sources
        .iter()
        .enumerate()
        .filter(|(_, source)| {
            !source["image"].is_null()
                && source["index"]
                    .as_u64()
                    .is_some_and(|index| indexes.contains(&index))
        })
        .map(|(position, _)| position)
        .collect()
}

fn update_insight_image(payload: &mut Value, position: usize, image: NewsImage) -> Result<bool> {
    let value = serde_json::to_value(image)?;
    let source = &mut payload["sources"][position];
    if source["image"] == value {
        return Ok(false);
    }
    source["image"] = value;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use url::Url;

    #[test]
    fn accepts_tmpfile_http_link_for_https_origin() {
        let direct = Url::parse("http://media.example.com/files/token").unwrap();
        let base = Url::parse("https://media.example.com").unwrap();
        assert!(same_http_to_https_authority(&direct, &base));
    }

    #[test]
    fn rejects_different_host_or_non_default_port() {
        let base = Url::parse("https://media.example.com").unwrap();
        assert!(!same_http_to_https_authority(
            &Url::parse("http://other.example/files/token").unwrap(),
            &base,
        ));
        assert!(!same_http_to_https_authority(
            &Url::parse("http://media.example.com:8080/files/token").unwrap(),
            &base,
        ));
    }

    #[test]
    fn rejects_tiny_placeholder_dimensions() {
        let mut png = vec![0; 4096];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[19] = 220;
        png[23] = 220;
        assert_eq!(image_dimensions(&png), Some((220, 220)));
        assert!(!image_is_usable(&png));
        png[18] = 2;
        png[19] = 128;
        png[22] = 2;
        png[23] = 128;
        assert_eq!(image_dimensions(&png), Some((640, 640)));
        assert!(image_is_usable(&png));
    }

    #[test]
    fn renews_only_explicitly_selected_images() {
        let image = json!({"url":"https://cdn.example.com/old.jpg",
            "source_url":"https://example.com/story", "caption":"图说",
            "caption_en":"Caption", "credit":"摄影师"});
        let mut payload = json!({"analysis": {"cover_source_index":7,"picture_source_indexes":[]},
            "sources":[{"index":7,"image":image}]});
        assert_eq!(selected_image_positions(&payload), vec![0]);
        let mut renewed = stored_image(&payload["sources"][0]["image"]).unwrap();
        renewed.url = "https://cdn.example.com/new.jpg".into();
        assert!(update_insight_image(&mut payload, 0, renewed.clone()).unwrap());
        assert!(!update_insight_image(&mut payload, 0, renewed).unwrap());
        payload["analysis"]["cover_source_index"] = Value::Null;
        assert!(selected_image_positions(&payload).is_empty());
        payload["analysis"]["picture_source_indexes"] = json!([7]);
        assert_eq!(selected_image_positions(&payload), vec![0]);
    }
}
