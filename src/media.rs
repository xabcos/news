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
use std::time::{Duration, Instant};
use url::Url;
use uuid::Uuid;

const RENEW_BEFORE_SECS: i64 = 6 * 3600;
const MAX_SYNC_SECS: u64 = 300;
// Tiny square thumbnails are commonly used by publishers for "image unavailable"
// placeholders. They are not useful as visual-news cards even when they return 200.
const MIN_IMAGE_BYTES: usize = 4096;
const MIN_IMAGE_DIMENSION: u32 = 320;

#[derive(Debug, Deserialize)]
struct UploadResponse {
    direct_link: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredImageRow {
    id: Uuid,
    topic_id: String,
    day: chrono::NaiveDate,
    image: Option<Value>,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredCoverRow {
    id: Uuid,
    payload: Option<Value>,
}

#[derive(Debug, sqlx::FromRow)]
struct StoredInsightRow {
    id: Uuid,
    payload: Option<Value>,
}

/// Mirrors one image when tmpfile is configured. A failed mirror returns `None` so a
/// visual card never falls back to a source URL that may be blocked by the browser.
/// Local read-only deployments without tmpfile keep the original URL for development.
pub(crate) async fn mirror_image(state: &AppState, mut image: NewsImage) -> Option<NewsImage> {
    image = image.validated()?;
    let Some(base_url) = state.config.tmpfile_base_url.as_ref() else {
        return Some(image);
    };
    if is_fresh_tmpfile_url(state, &image) {
        return Some(image);
    }

    let origin = image
        .origin_url
        .clone()
        .unwrap_or_else(|| image.url.clone());
    let mut response = match state
        .image_http
        .get(&origin)
        .header(header::USER_AGENT, "xabcnews.com image mirror")
        .header(
            header::ACCEPT,
            "image/avif,image/webp,image/jpeg,image/png,image/gif",
        )
        .timeout(state.config.image_timeout)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::debug!(status = %response.status(), url = %origin, "news image download failed");
            return None;
        }
        Err(error) => {
            tracing::debug!(%error, url = %origin, "news image download failed");
            return None;
        }
    };
    if !is_public_https_url(response.url()) {
        tracing::debug!(url = %origin, "news image redirected to an insecure URL");
        return None;
    }

    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(normalize_content_type);
    let Some(content_type) = content_type else {
        tracing::debug!(url = %origin, "news image has an unsupported content type");
        return None;
    };
    if response
        .content_length()
        .is_some_and(|size| size > state.config.image_max_bytes)
    {
        tracing::debug!(url = %origin, "news image exceeds configured size limit");
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = match response.chunk().await {
        Ok(chunk) => chunk,
        Err(error) => {
            tracing::debug!(%error, url = %origin, "reading news image failed");
            return None;
        }
    } {
        if bytes.len().saturating_add(chunk.len()) as u64 > state.config.image_max_bytes {
            tracing::debug!(url = %origin, "news image exceeds configured size limit");
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        tracing::debug!(url = %origin, "news image is empty");
        return None;
    }
    if !image_is_usable(&bytes) {
        tracing::debug!(url = %origin, size = bytes.len(), "news image is too small or likely a placeholder");
        return None;
    }

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
    let mut request = state
        .image_http
        .post(upload_url)
        .multipart(Form::new().part("file", part))
        .timeout(state.config.image_timeout);
    if let Some(key) = state.config.tmpfile_api_key.as_deref() {
        request = request.bearer_auth(key);
    }
    let response = match request.send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::debug!(status = %response.status(), url = %origin, "tmpfile image upload failed");
            return None;
        }
        Err(error) => {
            tracing::debug!(%error, url = %origin, "tmpfile image upload failed");
            return None;
        }
    };
    let uploaded = match response.json::<UploadResponse>().await {
        Ok(uploaded) => uploaded,
        Err(error) => {
            tracing::debug!(%error, url = %origin, "tmpfile returned an invalid image response");
            return None;
        }
    };
    if uploaded.expires_at <= Utc::now() {
        tracing::debug!(url = %origin, "tmpfile returned an expired image link");
        return None;
    }
    let Ok(mut direct_url) = Url::parse(&uploaded.direct_link) else {
        tracing::debug!(url = %origin, "tmpfile returned an invalid direct link");
        return None;
    };
    // Older tmpfile deployments may derive the link from an HTTP forwarded header even
    // though the public origin is HTTPS. Upgrade that same-authority link to HTTPS; never
    // accept a different host or an insecure URL from the upstream response.
    if direct_url.scheme() == "http"
        && base_url.scheme() == "https"
        && same_http_to_https_authority(&direct_url, base_url)
    {
        let _ = direct_url.set_scheme("https");
    }
    if !is_public_https_url(&direct_url) || !same_origin(&direct_url, base_url) {
        tracing::debug!(url = %origin, "tmpfile returned a non-HTTPS direct link");
        return None;
    }
    image.origin_url = Some(origin);
    image.url = direct_url.to_string();
    image.expires_at = Some(uploaded.expires_at);
    Some(image)
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
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
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

/// Mirrors the optional cover embedded in a completed fetch payload. The payload is
/// updated in place; a failed mirror clears the cover when tmpfile is configured.
pub(crate) async fn mirror_payload_cover(state: &AppState, payload: &mut Value) {
    let Some(value) = payload
        .get("cover")
        .cloned()
        .filter(|value| !value.is_null())
    else {
        return;
    };
    let Ok(image) = serde_json::from_value::<NewsImage>(value) else {
        payload["cover"] = Value::Null;
        return;
    };
    payload["cover"] = mirror_image(state, image)
        .await
        .and_then(|image| serde_json::to_value(image).ok())
        .unwrap_or(Value::Null);
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

/// Mirrors current visual-feed assets after a restart. The operation is deliberately
/// bounded to one week because tmpfile URLs are temporary and old archive cards can
/// gracefully fall back to their text when their source is no longer available.
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
    let deadline = Instant::now() + Duration::from_secs(MAX_SYNC_SECS);
    let mut changed_topics = std::collections::HashSet::new();
    for row in rows {
        if Instant::now() >= deadline {
            tracing::warn!("image sync reached its time budget; remaining assets will retry later");
            break;
        }
        let Some(value) = row.image.clone() else {
            continue;
        };
        let Ok(image) = serde_json::from_value::<NewsImage>(value) else {
            continue;
        };
        let image = match tokio::time::timeout_at(deadline.into(), mirror_image(state, image)).await
        {
            Ok(Some(image)) => image,
            Ok(None) => continue,
            Err(_) => break,
        };
        let value = serde_json::to_value(image)?;
        if row.image.as_ref() == Some(&value) {
            continue;
        }
        sqlx::query("UPDATE items SET image = $2 WHERE id = $1")
            .bind(row.id)
            .bind(value)
            .execute(&state.pool)
            .await?;
        changed_topics.insert((row.topic_id, row.day));
    }

    let covers = sqlx::query_as::<_, StoredCoverRow>(
        "SELECT runs.id, runs.payload FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id IS NULL AND jsonb_typeof(runs.payload->'cover') = 'object' AND runs.started_at >= NOW() - INTERVAL '7 days'",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut insight_changed = false;
    for row in covers {
        if Instant::now() >= deadline {
            break;
        }
        let Some(mut payload) = row.payload else {
            continue;
        };
        let Some(value) = payload
            .get("cover")
            .cloned()
            .filter(|value| !value.is_null())
        else {
            continue;
        };
        let Ok(image) = serde_json::from_value::<NewsImage>(value) else {
            continue;
        };
        let image = match tokio::time::timeout_at(deadline.into(), mirror_image(state, image)).await
        {
            Ok(Some(image)) => image,
            Ok(None) => continue,
            Err(_) => break,
        };
        let value = serde_json::to_value(image)?;
        if payload.get("cover") == Some(&value) {
            continue;
        }
        payload["cover"] = value;
        sqlx::query("UPDATE runs SET payload = $2 WHERE id = $1")
            .bind(row.id)
            .bind(payload)
            .execute(&state.pool)
            .await?;
        insight_changed = true;
    }
    // Older published briefings may have selected a source image before mirroring was
    // added. Keep the selected cover usable after a restart as well.
    let insights = sqlx::query_as::<_, StoredInsightRow>(
        "SELECT id, payload FROM ai_insights WHERE status = 'ok' AND payload IS NOT NULL AND started_at >= NOW() - INTERVAL '7 days'",
    )
    .fetch_all(&state.pool)
    .await?;
    for row in insights {
        if Instant::now() >= deadline {
            break;
        }
        let Some(mut payload) = row.payload else {
            continue;
        };
        let Some(index) = payload
            .get("analysis")
            .and_then(|analysis| analysis.get("cover_source_index"))
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
        else {
            continue;
        };
        let Some(sources) = payload.get_mut("sources").and_then(Value::as_array_mut) else {
            continue;
        };
        let Some(source) = sources
            .iter_mut()
            .find(|source| source.get("index").and_then(Value::as_u64) == Some(index as u64))
        else {
            continue;
        };
        let Some(value) = source
            .get("image")
            .cloned()
            .filter(|value| !value.is_null())
        else {
            continue;
        };
        let Ok(image) = serde_json::from_value::<NewsImage>(value) else {
            continue;
        };
        let image = match tokio::time::timeout_at(deadline.into(), mirror_image(state, image)).await
        {
            Ok(Some(image)) => image,
            Ok(None) => continue,
            Err(_) => break,
        };
        let value = serde_json::to_value(image)?;
        if source.get("image") == Some(&value) {
            continue;
        }
        source["image"] = value;
        sqlx::query("UPDATE ai_insights SET payload = $2 WHERE id = $1")
            .bind(row.id)
            .bind(payload)
            .execute(&state.pool)
            .await?;
        insight_changed = true;
    }
    for (topic, day) in changed_topics {
        state.digest.refresh(state, &topic, day).await?;
    }
    if insight_changed {
        state.insight.invalidate();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{image_dimensions, image_is_usable, same_http_to_https_authority};
    use url::Url;

    #[test]
    fn accepts_tmpfile_http_link_for_https_origin() {
        let direct = Url::parse("http://dl.xabclabs.com/files/token").unwrap();
        let base = Url::parse("https://dl.xabclabs.com").unwrap();
        assert!(same_http_to_https_authority(&direct, &base));
    }

    #[test]
    fn rejects_different_host_or_non_default_port() {
        let base = Url::parse("https://dl.xabclabs.com").unwrap();
        assert!(!same_http_to_https_authority(
            &Url::parse("http://other.example/files/token").unwrap(),
            &base,
        ));
        assert!(!same_http_to_https_authority(
            &Url::parse("http://dl.xabclabs.com:8080/files/token").unwrap(),
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
}
