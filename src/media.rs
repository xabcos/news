//! Server-side mirroring of news photographs into tmpfile, and the HTTP client that
//! reaches untrusted origins.
//!
//! News sources often put images behind hotlink protection or short-lived URLs. The
//! browser therefore receives only the capability URL returned by tmpfile; the original
//! image URL is retained in the stored metadata so a later startup can renew it.

use crate::{
    error::{AppError, Result},
    fetch::NewsImage,
    state::AppState,
};
use chrono::{DateTime, TimeDelta, Utc};
use reqwest::{
    dns::{Addrs, Name, Resolve, Resolving},
    header,
    multipart::{Form, Part},
    redirect::Policy,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, future::Future, net::IpAddr, sync::Arc, time::Duration};
use tokio::{sync::OwnedSemaphorePermit, task::JoinSet, time::Instant};
use url::Url;
use uuid::Uuid;

/// Mirrored copies are renewed this long before tmpfile expires them.
pub(crate) const RENEW_BEFORE: TimeDelta = TimeDelta::hours(6);
/// News images of this many recent days are renewed, even after a copy has lapsed.
pub(crate) const RENEW_DAYS: i64 = 7;
const IMAGE_BATCH_TIMEOUT: Duration = Duration::from_secs(90);
const ORIGIN_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
// Tiny square thumbnails are commonly used by publishers for "image unavailable"
// placeholders. They are not useful as visual-news cards even when they return 200.
pub(crate) const MIN_IMAGE_BYTES: usize = 4096;
pub(crate) const MIN_IMAGE_DIMENSION: u32 = 320;
// Large placeholders are nearly flat, so they compress to almost nothing; photographs and
// product shots keep well over this many bytes per thousand pixels.
const MIN_BYTES_PER_KILOPIXEL: u64 = 10;

#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
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

/// Image jobs share `image_slots` and one batch budget. Each job owns its slot permit, so
/// blocking work that outlives a cancelled batch still counts against the limit. Jobs
/// unfinished at the deadline or on shutdown are dropped, cancelling their requests.
pub(crate) async fn batch<J, F, T>(state: &AppState, jobs: impl IntoIterator<Item = J>) -> Vec<T>
where
    J: FnOnce(OwnedSemaphorePermit) -> F + Send + 'static,
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let deadline = Instant::now() + IMAGE_BATCH_TIMEOUT;
    let mut tasks = JoinSet::new();
    for job in jobs {
        let slots = Arc::clone(&state.image_slots);
        tasks.spawn(async move {
            job(slots.acquire_owned().await.expect("image slots stay open")).await
        });
    }
    let mut done = Vec::new();
    loop {
        let result = tokio::select! {
            _ = state.shutdown.cancelled() => break,
            result = tokio::time::timeout_at(deadline, tasks.join_next()) => result,
        };
        match result {
            Ok(Some(Ok(value))) => done.push(value),
            Ok(Some(Err(error))) => tracing::warn!(%error, "image task failed"),
            Ok(None) => break,
            Err(_) => {
                tracing::debug!("image batch reached its time budget");
                break;
            }
        }
    }
    done
}

/// One transfer per origin, preserving each story's caption and attribution. Images still
/// unfinished at the batch deadline become None.
pub(crate) async fn mirror_images(state: &AppState, images: &mut [Option<NewsImage>]) {
    let mut origins = HashMap::new();
    let mut groups: Vec<Vec<(usize, NewsImage)>> = Vec::new();
    for (index, slot) in images.iter_mut().enumerate() {
        let Some(image) = slot.take().and_then(NewsImage::validated) else {
            continue;
        };
        let origin = image.origin_url.as_ref().unwrap_or(&image.url).clone();
        let group = *origins.entry(origin).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[group].push((index, image));
    }
    let mut jobs = Vec::new();
    for group in groups {
        // A copy that is still fresh serves its whole group without a transfer or a slot.
        let fresh = group
            .iter()
            .find(|(_, image)| is_fresh_tmpfile_url(state, image))
            .map(|(_, image)| image.clone());
        match fresh {
            Some(fresh) => share(images, group, &fresh),
            None => {
                let state = state.clone();
                jobs.push(move |slot| async move {
                    let _slot = slot;
                    let image = group[0].1.clone();
                    let mirrored = tokio::time::timeout(
                        state.config.image_timeout,
                        transfer_image(&state, image),
                    )
                    .await
                    .ok()
                    .flatten();
                    (group, mirrored)
                });
            }
        }
    }
    for (group, mirrored) in batch(state, jobs).await {
        if let Some(mirrored) = mirrored {
            share(images, group, &mirrored);
        }
    }
}

fn share(images: &mut [Option<NewsImage>], group: Vec<(usize, NewsImage)>, mirrored: &NewsImage) {
    for (index, mut image) in group {
        image.url.clone_from(&mirrored.url);
        image.origin_url.clone_from(&mirrored.origin_url);
        image.expires_at = mirrored.expires_at;
        image.content_hash.clone_from(&mirrored.content_hash);
        images[index] = Some(image);
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
    let base_url = &state.config.tmpfile_base_url;
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
    // tmpfile is configured and trusted, so it may sit on a private network.
    let response = match state
        .http
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

/// Image types the mirror keeps, in the order it asks for them.
pub(crate) const IMAGE_TYPES: [&str; 6] = [
    "image/avif",
    "image/webp",
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/bmp",
];

/// Why a bounded download was refused.
pub(crate) enum Refused {
    Request(reqwest::Error),
    Type,
    Size,
}

impl Refused {
    /// The error for a download of `what`, such as "原文".
    pub(crate) fn error(self, what: &str) -> AppError {
        match self {
            Self::Request(error) => error.into(),
            Self::Type => AppError::BadRequest(format!("{what}的内容类型不符").into()),
            Self::Size => AppError::BadRequest(format!("{what}过大").into()),
        }
    }
}

/// Sends a request built on `origin_http` and reads a successful response whose media type is
/// one of `types`, compared without case (`image/jpg` counts as `image/jpeg`), up to
/// `max_bytes`. Returns the final address, the matched type and the body.
pub(crate) async fn read_bounded(
    request: reqwest::RequestBuilder,
    types: &[&'static str],
    max_bytes: u64,
) -> std::result::Result<(Url, &'static str, Vec<u8>), Refused> {
    let mut response = request
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(Refused::Request)?;
    let media_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let media_type = if media_type == "image/jpg" {
        "image/jpeg"
    } else {
        media_type.as_str()
    };
    let matched = types
        .iter()
        .copied()
        .find(|kind| *kind == media_type)
        .ok_or(Refused::Type)?;
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes)
    {
        return Err(Refused::Size);
    }
    let url = response.url().clone();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Refused::Request)? {
        if (bytes.len() + chunk.len()) as u64 > max_bytes {
            return Err(Refused::Size);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((url, matched, bytes))
}

/// An image for browser mirroring or model input, or `None` with the reason logged.
pub(crate) async fn download_image(
    state: &AppState,
    url: &str,
    accept: &str,
) -> Option<(Vec<u8>, &'static str)> {
    let request = state.origin_http.get(url).header(header::ACCEPT, accept);
    match read_bounded(request, &IMAGE_TYPES, state.config.image_max_bytes).await {
        Ok((_, content_type, bytes)) => Some((bytes, content_type)),
        Err(refused) => {
            let reason = match refused {
                Refused::Request(error) => error.to_string(),
                Refused::Type => "unsupported content type".into(),
                Refused::Size => "exceeds the configured size limit".into(),
            };
            tracing::debug!(url = %url, %reason, "news image download failed");
            None
        }
    }
}

fn image_is_usable(bytes: &[u8]) -> bool {
    usable(bytes.len(), image_dimensions(bytes))
}

/// Large enough, and not so compressible that it is a flat placeholder.
fn usable(size: usize, dimensions: Option<(u32, u32)>) -> bool {
    size >= MIN_IMAGE_BYTES
        && !dimensions.is_some_and(|(width, height)| {
            width < MIN_IMAGE_DIMENSION
                || height < MIN_IMAGE_DIMENSION
                || (size as u64) * 1000
                    < u64::from(width) * u64::from(height) * MIN_BYTES_PER_KILOPIXEL
        })
}

/// Dimensions from the image header, without decoding the pixels. Formats the `image` crate
/// is built without (AVIF, BMP) give `None`; the byte-size guard still applies to them.
pub(crate) fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    // Reading the header can inflate metadata such as a PNG's colour profile; keep that small.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(4 * 1024 * 1024);
    reader.limits(limits);
    reader.into_dimensions().ok()
}

pub(crate) fn is_public_https_url(url: &Url) -> bool {
    url.scheme() == "https"
        && matches!(url.host(), Some(url::Host::Domain(host)) if host.contains('.') && !host.ends_with(".local") && !host.ends_with(".localhost"))
        && url.username().is_empty()
        && url.password().is_none()
}

/// Public HTTPS on the default port: where Reads downloads start and every redirect lands.
pub(crate) fn is_public_origin(url: &Url) -> bool {
    is_public_https_url(url) && url.port_or_known_default() == Some(443)
}

/// The client for untrusted origins: news images and every Reads download. Each host,
/// redirect hops included, must resolve only to public addresses, and the connection uses
/// exactly the addresses checked, so neither a redirect nor DNS rebinding reaches internal
/// services. No proxy, cookies or application credentials are involved.
/// Who reads other sites, with where to find out more, as Wikimedia asks of automated clients.
const USER_AGENT: &str = "XABCNEWS/1.0 (+https://xabcnews.com)";

pub(crate) fn origin_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .no_proxy()
        .dns_resolver(PublicDns)
        .connect_timeout(ORIGIN_CONNECT_TIMEOUT)
        .timeout(timeout)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if is_public_origin(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("redirect leaves public HTTPS")
            }
        }))
        .build()
}

struct PublicDns;

impl Resolve for PublicDns {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let addresses: Vec<_> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addresses.is_empty() || !addresses.iter().all(|address| public_ip(address.ip())) {
                return Err("origin does not resolve to public addresses".into());
            }
            let addresses: Addrs = Box::new(addresses.into_iter());
            Ok(addresses)
        })
    }
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || a == 0
                || a >= 224
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0))
        }
        IpAddr::V6(ip) => {
            let [a, b, ..] = ip.segments();
            // Global unicast only; exclude transition, documentation and protocol ranges.
            (0x2000..0x4000).contains(&a)
                && a != 0x2002
                && !(a == 0x2001 && (b < 0x200 || b == 0xdb8))
                && !(a == 0x3fff && b < 0x1000)
        }
    }
}

fn is_fresh_tmpfile_url(state: &AppState, image: &NewsImage) -> bool {
    let Ok(image_url) = Url::parse(&image.url) else {
        return false;
    };
    same_origin(&image_url, &state.config.tmpfile_base_url)
        && image
            .expires_at
            .is_some_and(|expires_at| expires_at > Utc::now() + RENEW_BEFORE)
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
/// reuse a transfer. Published daily images enter the batch before archive cards. What a
/// catalogue keeps, 𝑮𝒂𝒅𝒈𝒆𝒕𝒔' reviews and shares and every entry's pictures, joins once a copy
/// is due, ahead of the recent days, until a week past expiry.
pub async fn startup_sync(state: &AppState) -> Result<()> {
    let keep_from = state.digest.today() - chrono::Duration::days(RENEW_DAYS);
    let rows = sqlx::query_as::<_, StoredImageRow>(
        "SELECT items.id, items.topic_id, items.day, items.image FROM items JOIN topics ON topics.id = items.topic_id
         WHERE items.image IS NOT NULL AND topics.owner_user_id IS NULL
           AND (items.day >= $1 OR (items.image->>'expires_at')::timestamptz BETWEEN NOW() - INTERVAL '7 days' AND NOW() + $2
               AND items.topic_id = ANY($3)
               AND EXISTS (SELECT 1 FROM catalog_items c WHERE c.item_id = items.id AND c.role <> 'news'))
         ORDER BY items.day >= $1, items.day DESC LIMIT 500",
    )
    .bind(keep_from)
    .bind(RENEW_BEFORE)
    .bind(crate::catalog::KEPT_TOPICS)
    .fetch_all(&state.pool)
    .await?;
    // An entry's cover, then its official pictures.
    let entries = sqlx::query_as::<_, (Uuid, Value)>(
        "SELECT id, jsonb_build_array(image) || images FROM catalog_entries
         WHERE EXISTS (SELECT 1 FROM jsonb_array_elements(jsonb_build_array(image) || images) picture
             WHERE (picture->>'expires_at')::timestamptz BETWEEN NOW() - INTERVAL '7 days' AND NOW() + $1)
         LIMIT 100",
    )
    .bind(RENEW_BEFORE)
    .fetch_all(&state.pool)
    .await?;
    let covers = sqlx::query_as::<_, StoredPayloadRow>(
        "SELECT runs.id, runs.payload FROM runs JOIN topics ON topics.id = runs.topic_id WHERE topics.owner_user_id IS NULL AND jsonb_typeof(runs.payload->'cover') = 'object' AND runs.started_at >= NOW() - INTERVAL '7 days'",
    )
    .fetch_all(&state.pool)
    .await?;
    let insights = sqlx::query_as::<_, StoredPayloadRow>(
        "SELECT id, payload FROM ai_insights WHERE kind = 'ai' AND status = 'ok' AND payload IS NOT NULL AND started_at >= NOW() - INTERVAL '7 days' ORDER BY started_at DESC",
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
    let mut images =
        insights
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
            .chain(entries.iter().flat_map(|(_, pictures)| {
                pictures.as_array().into_iter().flatten().map(stored_image)
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
    for (id, pictures) in entries {
        let renewed = pictures
            .as_array()
            .into_iter()
            .flatten()
            .map(|old| {
                images
                    .next()
                    .flatten()
                    .map_or_else(|| Ok(old.clone()), serde_json::to_value)
            })
            .collect::<serde_json::Result<Vec<_>>>()?;
        // A guide written meanwhile keeps its newer pictures.
        sqlx::query(
            "UPDATE catalog_entries SET image = NULLIF($3->0, 'null'), images = $3 - 0
                     WHERE id = $1 AND jsonb_build_array(image) || images = $2",
        )
        .bind(id)
        .bind(pictures)
        .bind(Value::from(renewed))
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
    // The rows are committed; one failed refresh must not leave the other feeds serving
    // links that are about to expire.
    for (topic, day) in changed_topics {
        if let Err(error) = state.digest.refresh(state, &topic, day).await {
            tracing::warn!(%error, topic, %day, "news cache refresh after image renewal failed");
        }
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
    fn refuses_private_and_special_networks() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()));
        }
    }

    #[tokio::test]
    async fn origin_client_refuses_hosts_that_resolve_privately() {
        let client = origin_client(Duration::from_secs(5)).unwrap();
        let error = client.get("https://localhost:9/").send().await.unwrap_err();
        assert!(error.is_dns(), "{error:?}");
    }

    /// A picture of the given size: noise, which compresses like a photograph, or one flat
    /// colour, which compresses like a placeholder.
    fn picture(format: image::ImageFormat, width: u32, height: u32, noise: bool) -> Vec<u8> {
        let pixels = image::RgbImage::from_fn(width, height, |x, y| {
            let n = if noise {
                x.wrapping_mul(1_664_525)
                    .wrapping_add(y.wrapping_mul(1_013_904_223))
            } else {
                0x0080_8080
            };
            image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut bytes, format)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn rejects_tiny_placeholder_dimensions() {
        let small = picture(image::ImageFormat::Png, 220, 220, true);
        assert_eq!(image_dimensions(&small), Some((220, 220)));
        assert!(!image_is_usable(&small));
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::WebP,
            image::ImageFormat::Gif,
        ] {
            let photo = picture(format, 640, 480, true);
            assert_eq!(image_dimensions(&photo), Some((640, 480)), "{format:?}");
            assert!(image_is_usable(&photo), "{format:?}");
        }
        // A large picture that compresses to almost nothing is a placeholder too.
        let flat = picture(image::ImageFormat::WebP, 1200, 600, false);
        assert_eq!(image_dimensions(&flat), Some((1200, 600)));
        assert!(!image_is_usable(&flat));
        assert!(!usable(5_000, Some((1200, 600))));
        assert!(usable(5_000, None));
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
