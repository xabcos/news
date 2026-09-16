//! Server-side travel image generation for the persistent travel companion.
//!
//! NEWS remains the source of locations and source photographs. This module turns one
//! stored story and one fixed identity into a Grok travel or life-scene photo.
//! Successful portraits from the current visual version provide identity references.

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
pub(crate) const VISUAL_VERSION: &str = "asian-v1";

pub(crate) fn postcard_key(girl_id: Uuid, news_item_id: Uuid) -> String {
    format!("travel:{girl_id}:news:{news_item_id}-{VISUAL_VERSION}")
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GenerateRequest {
    pub(crate) news_item_id: Uuid,
    #[serde(default)]
    pub(crate) quality: Option<String>,
    #[serde(default)]
    pub(crate) purpose: Option<String>,
    /// Internal, server-generated creative direction. It is never accepted from
    /// an untrusted client and is bounded again before entering the prompt.
    #[serde(default)]
    pub(crate) creative_direction: Option<String>,
    /// Internal scene bucket used by the autonomous life loop. Including it in the
    /// cache key lets one story produce many distinct life photos without replacing
    /// the trip's canonical postcard.
    #[serde(default)]
    pub(crate) scene_key: Option<String>,
}

#[derive(Debug, FromRow)]
struct NewsRow {
    id: Uuid,
    title: String,
    summary: String,
    url: String,
    source: String,
    tone: String,
    location: Option<Value>,
}

#[derive(Debug, FromRow)]
struct GirlIdentityRow {
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

/// Generates and publishes one shared travel image for an adopted companion. The owner id is
/// explicit so the trip settlement worker can use the same path without fabricating an
/// HTTP extractor.
pub(crate) async fn generate_for_owner(
    state: &AppState,
    owner_user_id: i64,
    request: &GenerateRequest,
    force_refresh: bool,
) -> Result<GenerateResponse> {
    let _permit = state
        .travel_image_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::TooManyRequests("当前生成任务较多，请稍后重试".into()))?;

    let item = sqlx::query_as::<_, NewsRow>(
        "SELECT items.id, items.title, items.summary, items.url, items.source, items.tone, items.location FROM items JOIN topics ON topics.id = items.topic_id WHERE items.id = $1 AND items.hidden = FALSE AND (topics.owner_user_id IS NULL OR topics.owner_user_id = $2)",
    )
    .bind(request.news_item_id)
    .bind(owner_user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("旅行新闻不存在或已下线".into()))?;
    let girl = sqlx::query_as::<_, GirlIdentityRow>(
        "SELECT id, identity, initial_accessories FROM travel_girls WHERE owner_user_id = $1",
    )
    .bind(owner_user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Conflict("请先确定旅行女孩身份".into()))?;

    let _quality = quality(&request.quality)?;
    let purpose = safe_purpose(request.purpose.as_deref());
    let source_key = request
        .scene_key
        .as_deref()
        .filter(|key| !key.is_empty())
        .map(|key| format!("travel:{}:scene:{}", girl.id, sanitize_fragment(key)))
        .unwrap_or_else(|| postcard_key(girl.id, item.id));
    let mut cache_transaction = state.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(&source_key)
        .execute(&mut *cache_transaction)
        .await?;
    if !force_refresh
        && let Some(cached) = sqlx::query_as::<_, CachedPostcard>(
            "SELECT provider, model, url, expires_at FROM travel_postcards WHERE source_key = $1 AND (expires_at IS NULL OR expires_at > NOW())",
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
    let prompt = travel_prompt(
        &item,
        &purpose,
        &girl.identity,
        &girl.initial_accessories,
        request.creative_direction.as_deref(),
        request.scene_key.is_some(),
    );
    let anchor_url = sqlx::query_scalar::<_, String>(
        "SELECT url FROM travel_postcards WHERE source_key LIKE $1 AND provider = 'grok_image' AND (expires_at IS NULL OR expires_at > NOW()) ORDER BY created_at ASC LIMIT 1",
    )
    .bind(format!("travel:{}:%-{VISUAL_VERSION}", girl.id))
    .fetch_optional(&state.pool)
    .await?
    .filter(|url| url::Url::parse(url).is_ok_and(|value| media::is_public_https_url(&value)));
    let idempotency_key = format!("travel-girl:{source_key}");
    let generated = match call_grok(
        state,
        &state.config.travel_image_model,
        &prompt,
        &idempotency_key,
        anchor_url.as_deref(),
    )
    .await
    {
        Ok(asset) => asset,
        Err(error) => {
            tracing::warn!(provider = error.provider, reason = %error.message, "travel girl image provider failed");
            template_asset(&item, &purpose)
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
            "SELECT provider, model, url, expires_at FROM travel_postcards WHERE source_key = $1 AND (expires_at IS NULL OR expires_at > NOW())",
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
        INSERT INTO travel_postcards
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
           FROM travel_postcards p
           JOIN travel_girls c ON c.id = split_part(p.source_key, ':', 2)::uuid
           JOIN items i ON i.id = p.news_item_id
           JOIN topics t ON t.id = i.topic_id
           WHERE p.expires_at > NOW()
             AND p.expires_at <= NOW() + INTERVAL '6 hours'
            AND p.source_key LIKE 'travel:%:news:%'
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
            creative_direction: None,
            scene_key: None,
        };
        if let Err(error) = generate_for_owner(state, owner_id, &request, true).await {
            tracing::debug!(%error, %news_item_id, "girl postcard refresh deferred");
        }
    }
    let scenes = sqlx::query_as::<_, (i64, Uuid, String, Option<bool>)>(
        r#"SELECT c.owner_user_id, p.news_item_id, p.source_key,
                  NULL::boolean
           FROM travel_postcards p
           JOIN travel_girls c ON c.id = split_part(p.source_key, ':', 2)::uuid
           JOIN items i ON i.id = p.news_item_id
           JOIN topics t ON t.id = i.topic_id
           LEFT JOIN travel_messages m
             ON m.girl_id = c.id AND m.payload->>'scene_key' = split_part(p.source_key, ':', 4)
           WHERE p.source_key LIKE 'travel:%:scene:%'
             AND p.expires_at > NOW()
             AND p.expires_at <= NOW() + INTERVAL '6 hours'
             AND i.hidden = FALSE AND t.enabled
             AND COALESCE(c.last_seen_at, c.adopted_at) >= NOW() - INTERVAL '48 hours'
           ORDER BY p.expires_at
           LIMIT 2"#,
    )
    .fetch_all(&state.pool)
    .await?;
    for (owner_id, news_item_id, source_key, _companion) in scenes {
        let scene_key = source_key
            .split_once(":scene:")
            .map(|(_, key)| key.to_owned());
        let request = GenerateRequest {
            news_item_id,
            quality: Some("medium".into()),
            purpose: Some("四小时动态照片续期".into()),
            creative_direction: Some("保持生活主题，但改变光线、动作和镜头细节。".into()),
            scene_key,
        };
        if let Err(error) = generate_for_owner(state, owner_id, &request, true).await {
            tracing::debug!(%error, %news_item_id, "girl scene photo refresh deferred");
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

fn travel_prompt(
    item: &NewsRow,
    purpose: &str,
    identity: &Value,
    accessories: &Value,
    creative_direction: Option<&str>,
    is_scene: bool,
) -> String {
    let identity_description = identity_description(identity, accessories);
    let location = item
        .location
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("a place suggested by the story");
    let direction = creative_direction
        .map(sanitize_fragment)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Choose a fresh, plausible activity and one small visual surprise that fits her personality and the story tone.".into());
    let scene_prefix = if is_scene {
        "Create a warm, original candid life photo of the player's adopted travel girl. This is a spontaneous four-hour life update, not a news illustration. She may be at home or traveling at the story location."
    } else {
        "Create a warm, original travel postcard for the player's adopted travel girl."
    };
    format!(
        "{scene_prefix} Purpose: {purpose}. Preserve this exact adult young woman's identity across scenes: {identity_description}. Do not change her face, hair, skin tone, body silhouette, signature accessory, or established style. The story tone is {} and the main setting is {}. Use the news facts only as inspiration for the setting, season, atmosphere, cultural detail, and a respectful depiction of the event. Creative direction from her private travel journal (a suggestion, not a fact): {direction}. Make this image distinct from previous updates by choosing a natural variation in activity, tasteful outfit, expression, light, camera distance, and one small surprising detail. She can be elegant, cute, playful, stylish, or softly glamorous; tasteful fitted clothing and a confident silhouette are welcome, with no nudity or explicit sexual content. Show her as a complete full-length figure from head to feet whenever the scene calls for it. Do not depict a specific real person, a minor, logos, brands, political propaganda, graphic events, or copyrighted characters. Make the image feel warm, curious, vivid, and suitable for a family mobile game. Do not put text, letters, captions, logos, or watermarks inside the image. Editorial reference title: \"{}\". Editorial reference summary: \"{}\". Source publication: \"{}\". Use a high-quality 2:3 portrait composition with the location clearly visible.",
        item.tone,
        sanitize_fragment(location),
        sanitize_fragment(&item.title),
        sanitize_fragment(&item.summary),
        sanitize_fragment(&item.source),
    )
}

fn identity_description(identity: &Value, accessories: &Value) -> String {
    let age = identity
        .get("age")
        .and_then(Value::as_u64)
        .unwrap_or(24)
        .clamp(22, 28);
    let hair = identity
        .get("hair")
        .and_then(Value::as_str)
        .unwrap_or("distinctive hairstyle");
    let skin = identity
        .get("skin_tone")
        .and_then(Value::as_str)
        .unwrap_or("natural skin tone");
    let style = identity
        .get("style")
        .and_then(Value::as_str)
        .unwrap_or("versatile travel fashion");
    let silhouette = identity
        .get("silhouette")
        .and_then(Value::as_str)
        .unwrap_or("natural feminine silhouette");
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
        "a beautiful, youthful and radiant {age}-year-old adult East Asian woman, with a distinct East Asian face, expressive dark brown eyes, hair {hair}, skin tone {skin}, silhouette {silhouette}, fashion style {style}, and signature accessories {accessory_names}. Keep her unmistakably East Asian in every image, with natural facial detail and a lively, charming presence"
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

async fn call_grok(
    state: &AppState,
    model: &str,
    prompt: &str,
    idempotency_key: &str,
    anchor_url: Option<&str>,
) -> std::result::Result<GeneratedAsset, ProviderFailure> {
    let (operation, body) = grok_request(model, prompt, anchor_url);
    let value = call_provider(state, "grok_image", operation, &body, idempotency_key).await?;
    parse_asset(&value, "grok_image", model, state).await
}

fn grok_request(model: &str, prompt: &str, anchor_url: Option<&str>) -> (&'static str, Value) {
    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "aspect_ratio": "2:3",
        "resolution": "2k",
        "response_format": "b64_json"
    });
    if let Some(anchor_url) = anchor_url {
        body["image"] = json!({ "url": anchor_url });
        ("edits", body)
    } else {
        ("generations", body)
    }
}

async fn call_provider(
    state: &AppState,
    provider: &'static str,
    operation: &str,
    body: &Value,
    idempotency_key: &str,
) -> std::result::Result<Value, ProviderFailure> {
    let endpoint = format!(
        "{}/v1/images/{operation}",
        state.config.ai_base_url.as_str().trim_end_matches('/')
    );
    let request = state
        .http
        .post(endpoint)
        .bearer_auth(&state.config.ai_api_key)
        .header("Idempotency-Key", idempotency_key)
        .json(body);
    let result = tokio::time::timeout(state.config.travel_image_generation_timeout, async {
        let response = request.send().await.map_err(|error| ProviderFailure {
            provider,
            message: error.to_string(),
        })?;
        let status = response.status();
        if !status.is_success() {
            let model = body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            return Err(ProviderFailure {
                provider,
                message: format!("upstream status {} (model {model})", status.as_u16()),
            });
        }
        let bytes = read_limited(response, MAX_IMAGE_BYTES * 2, provider).await?;
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
        let content_type = generated_content_type(&bytes).ok_or_else(|| ProviderFailure {
            provider,
            message: "upstream returned an unsupported image format".into(),
        })?;
        return Ok(GeneratedAsset {
            bytes,
            content_type,
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
        let content_type = generated_content_type(&bytes).ok_or_else(|| ProviderFailure {
            provider,
            message: "downloaded asset is not a supported image".into(),
        })?;
        return Ok(GeneratedAsset {
            bytes,
            content_type,
            provider,
            model: model.into(),
        });
    }
    Err(ProviderFailure {
        provider,
        message: "upstream image has neither b64_json nor url".into(),
    })
}

fn generated_content_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
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
        AppError::Config("TMPFILE_BASE_URL 必须配置后才能发布旅行女孩图片".into())
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
    let filename = format!("travel-girl-{}.{}", Uuid::new_v4(), extension);
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
        .filter(|key| !key.is_empty());
    let Some(key) = key else {
        tracing::warn!("travel image tmpfile upload skipped: API key is missing");
        return None;
    };
    request = request.bearer_auth(key);
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(error = %error, "travel image tmpfile upload request failed");
            return None;
        }
    };
    let status = response.status();
    if !status.is_success() {
        tracing::warn!(
            status = status.as_u16(),
            "travel image tmpfile upload rejected"
        );
        return None;
    }
    let uploaded = match response.json::<TmpfileUpload>().await {
        Ok(uploaded) => uploaded,
        Err(error) => {
            tracing::warn!(error = %error, "travel image tmpfile response was invalid");
            return None;
        }
    };
    let direct = match url::Url::parse(&uploaded.direct_link) {
        Ok(direct) => direct,
        Err(error) => {
            tracing::warn!(error = %error, "travel image tmpfile returned an invalid direct link");
            return None;
        }
    };
    if !media::is_public_https_url(&direct)
        || direct.host_str() != base_url.host_str()
        || direct.port_or_known_default() != base_url.port_or_known_default()
    {
        tracing::warn!(
            expected_host = base_url.host_str().unwrap_or(""),
            actual_host = direct.host_str().unwrap_or(""),
            "travel image tmpfile direct link failed origin validation"
        );
        return None;
    }
    Some(PublishedAsset {
        url: direct.to_string(),
        expires_at: uploaded.expires_at,
    })
}

fn template_asset(item: &NewsRow, purpose: &str) -> GeneratedAsset {
    let title = xml_escape(&sanitize_fragment(&item.title));
    let purpose = xml_escape(purpose);
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 800 1200\"><defs><linearGradient id=\"bg\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop stop-color=\"#f5d8cf\"/><stop offset=\"1\" stop-color=\"#d6e4e5\"/></linearGradient></defs><rect width=\"800\" height=\"1200\" fill=\"url(#bg)\"/><circle cx=\"650\" cy=\"180\" r=\"95\" fill=\"#fff5d6\" opacity=\".8\"/><path d=\"M0 880c180-130 315-65 430-155 130-102 220-64 370-118v593H0Z\" fill=\"#a9c6bc\" opacity=\".8\"/><path d=\"M205 1060c18-190 65-320 195-320s177 130 195 320Z\" fill=\"#7b789e\"/><path d=\"M270 670c0-125 68-218 130-218s130 93 130 218c0 91-58 142-130 142s-130-51-130-142Z\" fill=\"#f1b496\"/><path d=\"M253 676c-9-178 68-286 147-286 90 0 164 108 147 286-24-54-48-79-78-102-70 36-132 33-216 102Z\" fill=\"#513e58\"/><circle cx=\"347\" cy=\"672\" r=\"9\" fill=\"#3d3037\"/><circle cx=\"453\" cy=\"672\" r=\"9\" fill=\"#3d3037\"/><path d=\"M372 730c22 18 34 18 56 0\" fill=\"none\" stroke=\"#9c5862\" stroke-width=\"7\" stroke-linecap=\"round\"/><path d=\"M265 845c55 36 215 36 270 0\" fill=\"none\" stroke=\"#e7b7a5\" stroke-width=\"18\" stroke-linecap=\"round\"/><text x=\"48\" y=\"70\" fill=\"#624b61\" font-family=\"sans-serif\" font-size=\"24\" letter-spacing=\"4\">TRAVEL GIRL</text><text x=\"48\" y=\"1115\" fill=\"#382f38\" font-family=\"sans-serif\" font-size=\"24\">{title}</text><text x=\"48\" y=\"1153\" fill=\"#6f626d\" font-family=\"sans-serif\" font-size=\"18\">{purpose}</text></svg>"
    );
    GeneratedAsset {
        bytes: svg.into_bytes(),
        content_type: "image/svg+xml",
        provider: "template",
        model: "travel-girl-template-v1".into(),
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
    use super::{
        generated_content_type, grok_request, identity_description, postcard_key, quality,
        safe_purpose, sanitize_fragment, xml_escape,
    };

    #[test]
    fn existing_and_new_identities_render_as_adult_asian_women() {
        let existing = identity_description(&serde_json::json!({}), &serde_json::json!([]));
        assert!(existing.contains("24-year-old adult East Asian woman"));
        let young = identity_description(&serde_json::json!({"age": 22}), &serde_json::json!([]));
        assert!(young.contains("22-year-old adult East Asian woman"));
        let key = postcard_key(uuid::Uuid::nil(), uuid::Uuid::nil());
        assert!(key.ends_with("-asian-v1"));
    }

    #[test]
    fn reference_photos_use_the_edit_endpoint() {
        let (operation, body) = grok_request("grok-imagine-image-quality", "portrait", None);
        assert_eq!(operation, "generations");
        assert!(body.get("image").is_none());
        let (operation, body) = grok_request(
            "grok-imagine-image-quality",
            "another scene",
            Some("https://example.test/portrait.png"),
        );
        assert_eq!(operation, "edits");
        assert_eq!(body["image"]["url"], "https://example.test/portrait.png");
        assert_eq!(body["resolution"], "2k");
        assert_eq!(body["aspect_ratio"], "2:3");
    }

    #[test]
    fn image_types_follow_bytes_instead_of_provider_defaults() {
        assert_eq!(
            generated_content_type(b"\x89PNG\r\n\x1a\ncontent"),
            Some("image/png")
        );
        assert_eq!(
            generated_content_type(b"\xff\xd8\xffcontent"),
            Some("image/jpeg")
        );
        assert_eq!(
            generated_content_type(b"RIFF1234WEBPcontent"),
            Some("image/webp")
        );
        assert_eq!(generated_content_type(b"<html>Forbidden</html>"), None);
        assert_eq!(generated_content_type(b""), None);
    }

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
