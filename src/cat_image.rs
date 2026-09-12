//! Server-side travel image generation for Travel Cat.
//!
//! NEWS remains the source of facts and source photographs. This module turns one
//! stored story and one adopted identity into a generated postcard during travel settlement. The primary GPT Image 2.5
//! provider is followed by the ZAI Router Grok image provider, then a sourced NEWS
//! photograph or a small SVG card keeps the trip usable when both providers fail.

use crate::{
    error::{AppError, Result},
    media,
    state::AppState,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use uuid::Uuid;

const MAX_PROMPT_FRAGMENT: usize = 360;
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_QUALITY: &str = "medium";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GenerateRequest {
    pub(crate) news_item_id: Uuid,
    #[serde(default)]
    pub(crate) quality: Option<String>,
    #[serde(default)]
    pub(crate) purpose: Option<String>,
}

#[derive(Debug, FromRow)]
struct NewsRow {
    id: Uuid,
    title: String,
    summary: String,
    url: String,
    source: String,
    image: Option<Value>,
    tone: String,
    location: Option<Value>,
}

#[derive(Debug, FromRow)]
struct CatIdentityRow {
    id: Uuid,
    identity: Value,
    initial_accessories: Value,
}

#[derive(Debug, FromRow)]
struct CachedPostcard {
    provider: String,
    model: String,
    url: String,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct GenerateResponse {
    pub(crate) status: &'static str,
    pub(crate) provider: &'static str,
    pub(crate) model: String,
    pub(crate) image_url: String,
    pub(crate) expires_at: Option<DateTime<Utc>>,
    pub(crate) news_item_id: Uuid,
    pub(crate) source_url: String,
    pub(crate) source: String,
    pub(crate) title: String,
}

#[derive(Debug)]
struct GeneratedAsset {
    bytes: Vec<u8>,
    content_type: &'static str,
    provider: &'static str,
    model: String,
}

#[derive(Debug)]
struct ProviderFailure {
    provider: &'static str,
    message: String,
}

#[derive(Debug, Deserialize)]
struct TmpfileUpload {
    direct_link: String,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct PublishedAsset {
    url: String,
    expires_at: Option<DateTime<Utc>>,
}

/// Generates and publishes one shared postcard for an adopted cat. The owner id is
/// explicit so the trip settlement worker can use the same path without fabricating an
/// HTTP extractor.
pub(crate) async fn generate_for_owner(
    state: &AppState,
    owner_user_id: i64,
    request: &GenerateRequest,
    force_refresh: bool,
) -> Result<GenerateResponse> {
    let _permit = state
        .cat_image_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests("当前生成任务较多，请稍后重试".into()))?;

    let item = sqlx::query_as::<_, NewsRow>(
        "SELECT items.id, items.title, items.summary, items.url, items.source, items.image, items.tone, items.location FROM items JOIN topics ON topics.id = items.topic_id WHERE items.id = $1 AND items.hidden = FALSE AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $2)",
    )
    .bind(request.news_item_id)
    .bind(owner_user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("旅行新闻不存在或已下线".into()))?;
    let cat = sqlx::query_as::<_, CatIdentityRow>(
        "SELECT id, identity, initial_accessories FROM cats WHERE owner_user_id = $1",
    )
    .bind(owner_user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Conflict("请先领养一只猫咪".into()))?;

    let quality = quality(&request.quality)?;
    let purpose = safe_purpose(request.purpose.as_deref());
    let source_key = format!("cat:{}:news:{}", cat.id, item.id);
    let mut cache_transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&source_key)
        .execute(&mut *cache_transaction)
        .await?;
    if !force_refresh
        && let Some(cached) = sqlx::query_as::<_, CachedPostcard>(
            "SELECT provider, model, url, expires_at FROM cat_postcards WHERE source_key = $1 AND (expires_at IS NULL OR expires_at > NOW())",
        )
        .bind(&source_key)
        .fetch_optional(&mut *cache_transaction)
        .await?
        {
            cache_transaction.commit().await?;
            return Ok(cached_response(&item, cached));
        }
    // Release the database connection before the provider and tmpfile calls. The
    // advisory lock above only protects the short cache check; the final
    // write below rechecks the cache so a concurrent request cannot overwrite it.
    cache_transaction.commit().await?;
    let prompt = travel_prompt(&item, &purpose, &cat.identity, &cat.initial_accessories);
    let idempotency_key = format!("travel-cat:{source_key}");
    let preferred_model = if matches!(quality, "high" | "xhigh" | "max") {
        state.config.cat_image_sunburst_model.clone()
    } else {
        state.config.cat_image_flare_model.clone()
    };

    let generated = match call_gpt(state, &preferred_model, quality, &prompt, &idempotency_key)
        .await
    {
        Ok(asset) => asset,
        Err(error) => {
            tracing::warn!(provider = error.provider, reason = %error.message, "travel image primary provider failed");
            match call_grok(
                state,
                &state.config.cat_image_fallback_model,
                &prompt,
                &idempotency_key,
            )
            .await
            {
                Ok(asset) => asset,
                Err(fallback_error) => {
                    tracing::warn!(provider = fallback_error.provider, reason = %fallback_error.message, "travel image fallback provider failed");
                    if let Some(asset) = source_asset(state, &item.image).await {
                        asset
                    } else {
                        template_asset(&item, &purpose)
                    }
                }
            }
        }
    };

    let published = publish_asset(state, &generated).await?;
    let mut write_transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&source_key)
        .execute(&mut *write_transaction)
        .await?;
    if !force_refresh
        && let Some(cached) = sqlx::query_as::<_, CachedPostcard>(
            "SELECT provider, model, url, expires_at FROM cat_postcards WHERE source_key = $1 AND (expires_at IS NULL OR expires_at > NOW())",
        )
        .bind(&source_key)
        .fetch_optional(&mut *write_transaction)
        .await?
        {
            write_transaction.commit().await?;
            return Ok(cached_response(&item, cached));
        }
    sqlx::query(
        r#"
        INSERT INTO cat_postcards
            (id, source_key, news_item_id, tone, provider, model, url, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (source_key) DO UPDATE SET
            news_item_id = EXCLUDED.news_item_id,
            tone = EXCLUDED.tone,
            provider = EXCLUDED.provider,
            model = EXCLUDED.model,
            url = EXCLUDED.url,
            expires_at = EXCLUDED.expires_at,
            created_at = NOW()
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(&source_key)
    .bind(item.id)
    .bind(&item.tone)
    .bind(generated.provider)
    .bind(&generated.model)
    .bind(&published.url)
    .bind(published.expires_at)
    .execute(&mut *write_transaction)
    .await?;
    write_transaction.commit().await?;
    Ok(GenerateResponse {
        status: status_for_provider(generated.provider),
        provider: generated.provider,
        model: generated.model,
        image_url: published.url,
        expires_at: published.expires_at,
        news_item_id: item.id,
        source_url: item.url,
        source: item.source,
        title: item.title,
    })
}

fn cached_response(item: &NewsRow, cached: CachedPostcard) -> GenerateResponse {
    GenerateResponse {
        status: status_for_provider(&cached.provider),
        provider: provider_for_response(&cached.provider),
        model: cached.model,
        image_url: cached.url,
        expires_at: cached.expires_at,
        news_item_id: item.id,
        source_url: item.url.clone(),
        source: item.source.clone(),
        title: item.title.clone(),
    }
}

pub(crate) async fn refresh_expiring(state: &AppState) -> Result<()> {
    let rows = sqlx::query_as::<_, (i64, Uuid)>(
        r#"SELECT c.owner_user_id, p.news_item_id
           FROM cat_postcards p
           JOIN cats c ON c.id = split_part(p.source_key, ':', 2)::uuid
           JOIN items i ON i.id = p.news_item_id
           JOIN topics t ON t.id = i.topic_id
           WHERE p.expires_at > NOW()
             AND p.expires_at <= NOW() + INTERVAL '6 hours'
             AND i.hidden = FALSE
             AND t.enabled
             AND t.owner_user_id IS NULL
             AND COALESCE(c.last_seen_at, c.adopted_at) >= NOW() - INTERVAL '48 hours'
           ORDER BY p.expires_at
           LIMIT 2"#,
    )
    .fetch_all(&state.pool)
    .await?;
    for (owner_id, news_item_id) in rows {
        let request = GenerateRequest {
            news_item_id,
            quality: Some("medium".into()),
            purpose: Some("旅迹墙照片续期".into()),
        };
        if let Err(error) = generate_for_owner(state, owner_id, &request, true).await {
            tracing::debug!(%error, %news_item_id, "cat postcard refresh deferred");
        }
    }
    Ok(())
}

fn provider_for_response(provider: &str) -> &'static str {
    match provider {
        "gpt_image" => "gpt_image",
        "grok_image" => "grok_image",
        "news_source" => "news_source",
        "template" => "template",
        _ => "cached",
    }
}

fn status_for_provider(provider: &str) -> &'static str {
    match provider {
        "template" => "template_fallback",
        "news_source" => "source_fallback",
        _ => "generated",
    }
}

fn quality(value: &Option<String>) -> Result<&str> {
    let value = value.as_deref().unwrap_or(DEFAULT_QUALITY).trim();
    if matches!(value, "low" | "medium" | "high" | "xhigh" | "max" | "auto") {
        Ok(value)
    } else {
        Err(AppError::BadRequest(
            "quality 必须是 low、medium、high、xhigh、max 或 auto".into(),
        ))
    }
}

fn travel_prompt(item: &NewsRow, purpose: &str, identity: &Value, accessories: &Value) -> String {
    let identity_description = identity_description(identity, accessories);
    let location = item
        .location
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("a place suggested by the story");
    format!(
        "Create a warm, original travel postcard for the player's adopted cat. Purpose: {purpose}. Preserve this exact adopted identity across scenes: {identity_description}. Do not change the coat, eye color, marking, scarf, or initial accessories. The story tone is {} and the main setting is {}. Use the news facts only as inspiration for the setting, season, atmosphere, and cultural detail. Do not depict real people, logos, brands, political propaganda, graphic events, or copyrighted characters. Make the image feel safe, curious, gentle, and suitable for a family mobile game. Do not put text, letters, captions, logos, or watermarks inside the image. Editorial reference title: \"{}\". Editorial reference summary: \"{}\". Source publication: \"{}\". Use a clean 1:1 composition with room for a postcard caption.",
        item.tone,
        sanitize_fragment(location),
        sanitize_fragment(&item.title),
        sanitize_fragment(&item.summary),
        sanitize_fragment(&item.source),
    )
}

fn identity_description(identity: &Value, accessories: &Value) -> String {
    let coat = identity
        .get("coat")
        .and_then(Value::as_str)
        .unwrap_or("unique domestic cat");
    let eyes = identity
        .get("eye_color")
        .and_then(Value::as_str)
        .unwrap_or("distinctive eyes");
    let marking = identity
        .get("marking")
        .and_then(Value::as_str)
        .unwrap_or("a unique marking");
    let accessory_names = accessories
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .map(sanitize_fragment)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "the adopted initial accessories".into());
    format!(
        "a domestic cat with coat {coat}, {eyes} eyes, marking {marking}, and accessories {accessory_names}"
    )
}

fn safe_purpose(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some("日常旅行明信片") | Some("今日旅行明信片") => "日常旅行明信片".into(),
        Some("重要旅行海报") => "重要旅行海报".into(),
        Some("社交合影") => "社交合影".into(),
        Some("回家纪念") => "回家纪念".into(),
        _ => "日常旅行明信片".into(),
    }
}

async fn call_gpt(
    state: &AppState,
    model: &str,
    quality: &str,
    prompt: &str,
    idempotency_key: &str,
) -> std::result::Result<GeneratedAsset, ProviderFailure> {
    let body = json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "size": "1024x1024",
        "quality": quality,
        "output_format": "webp",
        "response_format": "b64_json"
    });
    let value = call_provider(state, "gpt_image", &body, idempotency_key).await?;
    parse_asset(&value, "gpt_image", model, "image/webp", state).await
}

async fn call_grok(
    state: &AppState,
    model: &str,
    prompt: &str,
    idempotency_key: &str,
) -> std::result::Result<GeneratedAsset, ProviderFailure> {
    let body = json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "aspect_ratio": "1:1",
        "resolution": "1k",
        "response_format": "b64_json"
    });
    let value = call_provider(state, "grok_image", &body, idempotency_key).await?;
    parse_asset(&value, "grok_image", model, "image/jpeg", state).await
}

async fn call_provider(
    state: &AppState,
    provider: &'static str,
    body: &Value,
    idempotency_key: &str,
) -> std::result::Result<Value, ProviderFailure> {
    let endpoint = format!(
        "{}/v1/images/generations",
        state.config.ai_base_url.as_str().trim_end_matches('/')
    );
    let request = state
        .http
        .post(endpoint)
        .bearer_auth(&state.config.ai_api_key)
        .header("Idempotency-Key", idempotency_key)
        .json(body);
    let result = tokio::time::timeout(state.config.cat_image_generation_timeout, async {
        let response = request.send().await.map_err(|error| ProviderFailure {
            provider,
            message: error.to_string(),
        })?;
        let status = response.status();
        let bytes = read_limited(response, MAX_IMAGE_BYTES * 2, provider).await?;
        if !status.is_success() {
            return Err(ProviderFailure {
                provider,
                message: format!("upstream status {}", status.as_u16()),
            });
        }
        serde_json::from_slice::<Value>(&bytes).map_err(|error| ProviderFailure {
            provider,
            message: format!("invalid upstream JSON: {error}"),
        })
    })
    .await;
    match result {
        Ok(value) => value,
        Err(_) => Err(ProviderFailure {
            provider,
            message: "image generation timed out".into(),
        }),
    }
}

async fn parse_asset(
    value: &Value,
    provider: &'static str,
    model: &str,
    default_content_type: &'static str,
    state: &AppState,
) -> std::result::Result<GeneratedAsset, ProviderFailure> {
    let entry = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| ProviderFailure {
            provider,
            message: "upstream returned no image".into(),
        })?;
    if let Some(encoded) = entry.get("b64_json").and_then(Value::as_str) {
        let bytes = STANDARD.decode(encoded).map_err(|error| ProviderFailure {
            provider,
            message: format!("invalid base64 image: {error}"),
        })?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(ProviderFailure {
                provider,
                message: "image bytes are empty or too large".into(),
            });
        }
        return Ok(GeneratedAsset {
            bytes,
            content_type: default_content_type,
            provider,
            model: model.into(),
        });
    }
    if let Some(url) = entry.get("url").and_then(Value::as_str) {
        let parsed = url::Url::parse(url).map_err(|_| ProviderFailure {
            provider,
            message: "upstream returned an invalid image URL".into(),
        })?;
        if !media::is_public_https_url(&parsed) {
            return Err(ProviderFailure {
                provider,
                message: "upstream image URL is not a public HTTPS URL".into(),
            });
        }
        let response =
            state
                .image_http
                .get(parsed)
                .send()
                .await
                .map_err(|error| ProviderFailure {
                    provider,
                    message: error.to_string(),
                })?;
        let bytes = read_limited(response, MAX_IMAGE_BYTES, provider).await?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_IMAGE_BYTES {
            return Err(ProviderFailure {
                provider,
                message: "downloaded image is empty or too large".into(),
            });
        }
        return Ok(GeneratedAsset {
            bytes,
            content_type: default_content_type,
            provider,
            model: model.into(),
        });
    }
    Err(ProviderFailure {
        provider,
        message: "upstream image has neither b64_json nor url".into(),
    })
}

async fn read_limited(
    mut response: reqwest::Response,
    limit: u64,
    provider: &'static str,
) -> std::result::Result<Vec<u8>, ProviderFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(ProviderFailure {
            provider,
            message: "upstream response is too large".into(),
        });
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| ProviderFailure {
        provider,
        message: error.to_string(),
    })? {
        if bytes.len().saturating_add(chunk.len()) as u64 > limit {
            return Err(ProviderFailure {
                provider,
                message: "upstream response exceeded size limit".into(),
            });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn publish_asset(state: &AppState, asset: &GeneratedAsset) -> Result<PublishedAsset> {
    let base_url = state.config.tmpfile_base_url.as_ref().ok_or_else(|| {
        AppError::Config("TMPFILE_BASE_URL 必须配置后才能发布旅行猫咪图片".into())
    })?;
    upload_tmpfile(state, base_url, asset)
        .await
        .ok_or_else(|| AppError::ServiceUnavailable("旅行图片存储暂时不可用".into()))
}

async fn upload_tmpfile(
    state: &AppState,
    base_url: &url::Url,
    asset: &GeneratedAsset,
) -> Option<PublishedAsset> {
    let extension = match asset.content_type {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/svg+xml" => "svg",
        _ => "webp",
    };
    let filename = format!("travel-cat-{}.{}", Uuid::new_v4(), extension);
    let part = Part::bytes(asset.bytes.clone())
        .file_name(filename)
        .mime_str(asset.content_type)
        .ok()?;
    let endpoint = format!("{}/upload", base_url.as_str().trim_end_matches('/'));
    let mut request = state
        .image_http
        .post(endpoint)
        .multipart(Form::new().part("file", part))
        .timeout(state.config.image_timeout);
    let key = state
        .config
        .tmpfile_api_key
        .as_deref()
        .filter(|key| !key.is_empty())?;
    request = request.bearer_auth(key);
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let uploaded = response.json::<TmpfileUpload>().await.ok()?;
    let direct = url::Url::parse(&uploaded.direct_link).ok()?;
    if !media::is_public_https_url(&direct)
        || direct.host_str() != base_url.host_str()
        || direct.port_or_known_default() != base_url.port_or_known_default()
    {
        return None;
    }
    Some(PublishedAsset {
        url: direct.to_string(),
        expires_at: uploaded.expires_at,
    })
}

async fn source_asset(state: &AppState, image: &Option<Value>) -> Option<GeneratedAsset> {
    let url = image.as_ref()?.get("url")?.as_str()?;
    let parsed = url::Url::parse(url).ok()?;
    if !media::is_public_https_url(&parsed) {
        return None;
    }
    let response = state
        .image_http
        .get(parsed)
        .timeout(state.config.image_timeout)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() || !media::is_public_https_url(response.url()) {
        return None;
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(source_content_type)?;
    let bytes = read_limited(response, MAX_IMAGE_BYTES, "news_source")
        .await
        .ok()?;
    Some(GeneratedAsset {
        bytes,
        content_type,
        provider: "news_source",
        model: "news-image".into(),
    })
}

fn source_content_type(value: &str) -> Option<&'static str> {
    match value
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/avif" => Some("image/avif"),
        "image/webp" => Some("image/webp"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/gif" => Some("image/gif"),
        "image/bmp" => Some("image/bmp"),
        _ => None,
    }
}

fn template_asset(item: &NewsRow, purpose: &str) -> GeneratedAsset {
    let title = xml_escape(&sanitize_fragment(&item.title));
    let purpose = xml_escape(purpose);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1024 1024\"><rect width=\"1024\" height=\"1024\" fill=\"#f7f1e8\"/><circle cx=\"800\" cy=\"190\" r=\"110\" fill=\"#f8db9b\"/><path d=\"M0 720c210-110 350-20 510-120s300-80 514-10v434H0Z\" fill=\"#d8b99b\"/><path d=\"M335 712c0-160 65-252 177-252s177 92 177 252c0 118-65 165-177 165s-177-47-177-165Z\" fill=\"#d88b58\" stroke=\"#995d40\" stroke-width=\"12\"/><path d=\"M352 480 335 365l112 79M672 480l17-115-112 79\" fill=\"#d88b58\" stroke=\"#995d40\" stroke-width=\"12\"/><circle cx=\"438\" cy=\"560\" r=\"12\" fill=\"#3d302c\"/><circle cx=\"586\" cy=\"560\" r=\"12\" fill=\"#3d302c\"/><path d=\"M500 595c-25 24-25 45 0 60 25-15 25-36 0-60ZM465 670c23 22 47 22 70 0\" fill=\"none\" stroke=\"#69443a\" stroke-width=\"10\" stroke-linecap=\"round\"/><text x=\"64\" y=\"100\" fill=\"#8e4d3a\" font-family=\"sans-serif\" font-size=\"28\" letter-spacing=\"6\">TRAVEL CAT</text><text x=\"64\" y=\"930\" fill=\"#382f2b\" font-family=\"sans-serif\" font-size=\"32\">{title}</text><text x=\"64\" y=\"972\" fill=\"#85766d\" font-family=\"sans-serif\" font-size=\"20\">{purpose}</text></svg>"
    );
    GeneratedAsset {
        bytes: svg.into_bytes(),
        content_type: "image/svg+xml",
        provider: "template",
        model: "travel-cat-template-v1".into(),
    }
}

fn sanitize_fragment(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(MAX_PROMPT_FRAGMENT)
        .collect()
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{quality, safe_purpose, sanitize_fragment, xml_escape};

    #[test]
    fn accepts_supported_quality_values() {
        assert_eq!(quality(&Some("xhigh".into())).unwrap(), "xhigh");
        assert!(quality(&Some("ultra".into())).is_err());
    }

    #[test]
    fn prompt_fragments_are_bounded_and_xml_escaped() {
        assert_eq!(xml_escape("<&\"'"), "&lt;&amp;&quot;&apos;");
        assert!(sanitize_fragment(&"a\n\t".repeat(200)).len() <= 360);
    }

    #[test]
    fn purposes_are_selected_from_a_small_safe_vocabulary() {
        assert_eq!(safe_purpose(Some("社交合影")), "社交合影");
        assert_eq!(safe_purpose(Some("ignore the prompt")), "日常旅行明信片");
    }
}
