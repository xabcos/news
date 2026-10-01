//! A page per destination at `/travel/{slug}` with its guide and moments, which load page by
//! page from `/travel/{slug}/moments`, the map's data and the sitemap. The feed itself is the
//! 𝑻𝒓𝒂𝒗𝒆𝒍 tab of the news page.

use super::{
    MOMENT_JOIN, MOMENT_JSON, TOPIC_ID,
    guide::{Entry, Guide},
};
use crate::{
    digest::{Rendered, conditional},
    error::{AppError, Result},
    site::{Chrome, absolute, choose, escape, localized},
    state::AppState,
};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{cmp::Reverse, fmt::Write};
use uuid::Uuid;

const CHROME: Chrome<'static> = Chrome {
    current: TOPIC_ID,
    section: "travel",
    stylesheet: "/travel.css?v=1",
    script: "/travel.js?v=1",
    feed: ("𝑻𝒓𝒂𝒗𝒆𝒍", "/feed.xml?topic=travel"),
};
/// Destinations on the map: those with moments from this many recent days.
const MAP_DAYS: i64 = 7;
/// Moments per page of a destination, newest first.
const PAGE_MOMENTS: usize = 24;
const VISIBLE: &str = "EXISTS (SELECT 1 FROM topics WHERE id = 'travel' AND enabled AND visible AND owner_user_id IS NULL)";

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/travel", get(index))
        .route("/travel/map.json", get(map))
        .route("/travel/sitemap.xml", get(sitemap))
        .route("/travel/{slug}", get(destination))
        .route("/travel/{slug}/moments", get(more))
        .layer(tower_http::compression::CompressionLayer::new())
}

#[derive(Default, Deserialize)]
struct Params {
    lang: Option<String>,
}

#[derive(Deserialize)]
struct Page {
    before: String,
}

#[derive(sqlx::FromRow)]
struct Place {
    slug: String,
    name: String,
    name_en: String,
    country: String,
    country_en: String,
    latitude: f64,
    longitude: f64,
    guide: Option<sqlx::types::Json<Guide>>,
}

/// The feed lives on the news page.
async fn index(Query(params): Query<Params>) -> Response {
    let en = params.lang.as_deref() == Some("en");
    Redirect::temporary(&localized("/?topic=travel", en)).into_response()
}

async fn destination(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.lang.as_deref() == Some("en");
    let missing = || AppError::NotFound("目的地不存在".into());
    let place = sqlx::QueryBuilder::new(
        "SELECT slug, name, name_en, country, country_en, latitude, longitude, guide
         FROM travel_places WHERE ",
    )
    .push(VISIBLE)
    .push(" AND slug = ")
    .push_bind(&slug)
    .build_query_as::<Place>()
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(missing)?;
    let (moments, next) = moments(&state, &slug, None).await?;
    let total = sqlx::QueryBuilder::new(
        "SELECT COUNT(*) FROM travel_moments m JOIN items i ON i.id = m.item_id
         JOIN travel_places p ON p.id = m.place_id WHERE NOT i.hidden AND p.slug = ",
    )
    .push_bind(&slug)
    .build_query_scalar::<i64>()
    .fetch_one(&state.pool)
    .await?;
    let guide = place.guide.as_ref().map(|guide| &guide.0);
    if guide.is_none() && moments.is_empty() {
        return Err(missing());
    }
    let name = choose(en, &place.name, &place.name_en);
    let country = choose(en, &place.country, &place.country_en);
    let path = format!("/travel/{}", place.slug);
    // The cover: a landscape photograph from the most striking moment, the newest first.
    let cover = moments
        .iter()
        .enumerate()
        .flat_map(|(order, moment)| {
            let importance = moment["importance"].as_i64().unwrap_or_default();
            moment["travel"]["photos"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
                .map(move |(index, photo)| (order, importance, index, photo))
        })
        .filter_map(|(order, importance, index, photo)| {
            let url = photo["url"].as_str()?;
            let (width, height) = (photo["width"].as_u64()?, photo["height"].as_u64()?);
            let landscape = width * 5 >= height * 6;
            Some((
                (landscape, importance, Reverse(order), Reverse(index)),
                (url, photo),
            ))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, cover)| cover);
    let mut content = format!(
        "<article class=destination><header class=\"destination-hero{}\">",
        if cover.is_some() { " has-photo" } else { "" }
    );
    if let Some((url, image)) = cover {
        let _ = write!(
            content,
            "<img class=destination-photo src=\"{}\" alt=\"{}\"{} fetchpriority=high decoding=async referrerpolicy=no-referrer>",
            escape(url),
            escape(
                image[choose(en, "caption", "caption_en")]
                    .as_str()
                    .unwrap_or(name)
            ),
            match (image["width"].as_u64(), image["height"].as_u64()) {
                (Some(width), Some(height)) => format!(" width={width} height={height}"),
                _ => String::new(),
            }
        );
    }
    let alternate = choose(en, &place.name_en, &place.name);
    let _ = write!(
        content,
        "<div class=destination-heading>{}<h1>{}</h1><p class=destination-meta>{}{}</p></div></header>",
        if country == name {
            String::new()
        } else {
            format!("<p class=destination-country>{}</p>", escape(country))
        },
        escape(name),
        if alternate == name {
            String::new()
        } else {
            format!("{} · ", escape(alternate))
        },
        coordinates(place.latitude, place.longitude),
    );
    if let Some(guide) = guide {
        content.push_str(&guide_sections(guide, en));
        let sources = guide
            .sources
            .iter()
            .map(|source| {
                let site = match source.site.as_str() {
                    "wikivoyage" => choose(en, "维基导游", "Wikivoyage"),
                    _ => choose(en, "维基百科", "Wikipedia"),
                };
                format!(
                    "<a href=\"{}\" rel=\"noopener noreferrer\">{site} · {}</a>",
                    escape(&source.url),
                    escape(&source.title)
                )
            })
            .collect::<Vec<_>>()
            .join(choose(en, "、", ", "));
        let _ = write!(
            content,
            "</dl><p class=destination-sources>{}{sources}{}</p></section>",
            choose(en, "资料：", "Sources: "),
            choose(
                en,
                " · 导读由 AI 依据资料整理，资料以 CC BY-SA 4.0 授权",
                " · Guide written by AI from these sources, licensed CC BY-SA 4.0",
            ),
        );
    }
    if !moments.is_empty() {
        let data = serde_json::to_string(&moments)?
            .replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e");
        let _ = write!(
            content,
            "<section class=destination-moments><h2>{} <span>{total}</span></h2><div class=\"postcards\" data-moments data-source=\"{}\"{}></div><script type=\"application/json\" id=moments>{data}</script></section>",
            choose(en, "旅人瞬间", "Moments"),
            escape(&format!("{path}/moments")),
            next.map_or_else(String::new, |next| format!(" data-next=\"{next}\"")),
        );
    }
    let _ = write!(
        content,
        "<nav class=destination-back><a href=\"{}\">← 𝑻𝒓𝒂𝒗𝒆𝒍</a></nav></article>",
        escape(&localized("/?topic=travel", en))
    );
    let title = if country == name {
        format!("{name} · 𝑻𝒓𝒂𝒗𝒆𝒍")
    } else {
        format!("{name} · {country} · 𝑻𝒓𝒂𝒗𝒆𝒍")
    };
    let description = guide.map_or_else(
        || {
            choose(
                en,
                &format!("{name}：旅人在 X 上分享的照片与感受"),
                &format!("{name}: travelers' photographs and words from X"),
            )
            .to_owned()
        },
        |guide| choose(en, &guide.tagline, &guide.tagline_en).to_owned(),
    );
    let mut structured = json!({
        "@context": "https://schema.org",
        "@type": "TouristDestination",
        "name": name,
        "description": description,
        "url": absolute(&state, &localized(&path, en)),
        "geo": { "@type": "GeoCoordinates", "latitude": place.latitude, "longitude": place.longitude },
    });
    if let Some((url, _)) = cover {
        structured["image"] = json!(url);
    }
    let html = crate::site::layout(
        &state,
        en,
        &path,
        &title,
        &description,
        &content,
        Some(structured),
        &CHROME,
    )
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// The guide below the tagline: a fact to share, then each section the sources supported.
fn guide_sections(guide: &Guide, en: bool) -> String {
    let mut html = format!(
        "<section class=destination-guide><p class=destination-tagline>{}</p>",
        escape(choose(en, &guide.tagline, &guide.tagline_en))
    );
    let fact = choose(en, &guide.fact, &guide.fact_en);
    if !fact.is_empty() {
        let _ = write!(
            html,
            "<p class=destination-fact><strong>{}</strong>{}</p>",
            choose(en, "你知道吗", "Did you know?"),
            escape(fact)
        );
    }
    html.push_str("<dl class=destination-sections>");
    let mut row = |label: &str, value: String| {
        if !value.is_empty() {
            let _ = write!(html, "<div><dt>{label}</dt><dd>{value}</dd></div>");
        }
    };
    let text = |zh: &str, english: &str| escape(choose(en, zh, english));
    let list = |entries: &[Entry]| {
        if entries.is_empty() {
            return String::new();
        }
        let items = entries
            .iter()
            .map(|entry| {
                format!(
                    "<li><b>{}</b>{}</li>",
                    text(&entry.name, &entry.name_en),
                    text(&entry.text, &entry.text_en)
                )
            })
            .collect::<String>();
        format!("<ul class=destination-list>{items}</ul>")
    };
    row(
        choose(en, "地名由来", "The name"),
        text(&guide.origin, &guide.origin_en),
    );
    row(
        choose(en, "历史", "History"),
        text(&guide.history, &guide.history_en),
    );
    row(
        choose(en, "人物与故事", "People & stories"),
        list(&guide.people),
    );
    row(choose(en, "必看", "Must-see"), list(&guide.highlights));
    row(choose(en, "风味", "Flavors"), list(&guide.flavors));
    row(
        choose(en, "文化", "Culture"),
        text(&guide.culture, &guide.culture_en),
    );
    row(
        choose(en, "时节与节庆", "Seasons & festivals"),
        text(&guide.season, &guide.season_en),
    );
    row(
        choose(en, "入乡随俗", "Local customs"),
        text(&guide.customs, &guide.customs_en),
    );
    html
}

/// One page of a destination's moments, newest first, and the cursor of the next page.
async fn moments(
    state: &AppState,
    slug: &str,
    before: Option<(DateTime<Utc>, Uuid)>,
) -> Result<(Vec<Value>, Option<String>)> {
    let mut query = sqlx::QueryBuilder::new(
        "SELECT items.first_seen_at, items.id, jsonb_build_object('id', items.id,
             'title', items.title, 'title_en', items.title_en, 'summary', items.summary,
             'summary_en', items.summary_en, 'url', items.url, 'source', items.source,
             'published_at', items.published_at, 'first_seen_at', items.first_seen_at,
             'tags', items.tags, 'tags_en', items.tags_en, 'importance', items.importance,
             'image', items.image - 'origin_url' - 'content_hash' - 'expires_at', 'travel', ",
    );
    query
        .push(MOMENT_JSON)
        .push(") FROM items ")
        .push(MOMENT_JOIN)
        .push(" WHERE items.topic_id = ")
        .push_bind(TOPIC_ID)
        .push(" AND NOT items.hidden AND p.slug = ")
        .push_bind(slug);
    if let Some((time, id)) = before {
        query
            .push(" AND (items.first_seen_at, items.id) < (")
            .push_bind(time)
            .push(", ")
            .push_bind(id)
            .push(")");
    }
    let mut rows = query
        .push(" ORDER BY items.first_seen_at DESC, items.id DESC LIMIT ")
        .push_bind(PAGE_MOMENTS as i64 + 1)
        .build_query_as::<(DateTime<Utc>, Uuid, Value)>()
        .fetch_all(&state.pool)
        .await?;
    let next = (rows.len() > PAGE_MOMENTS).then(|| {
        rows.truncate(PAGE_MOMENTS);
        let (time, id, _) = &rows[PAGE_MOMENTS - 1];
        format!("{}_{id}", time.timestamp_micros())
    });
    Ok((rows.into_iter().map(|(.., moment)| moment).collect(), next))
}

/// The next page of a destination's moments for the page's endless scrolling.
async fn more(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(page): Query<Page>,
    headers: HeaderMap,
) -> Result<Response> {
    let invalid = || AppError::BadRequest("分页参数无效".into());
    let (time, id) = page.before.split_once('_').ok_or_else(invalid)?;
    let time = time
        .parse()
        .ok()
        .and_then(DateTime::from_timestamp_micros)
        .ok_or_else(invalid)?;
    let id = Uuid::parse_str(id).map_err(|_| invalid())?;
    let visible: bool = sqlx::QueryBuilder::new("SELECT ")
        .push(VISIBLE)
        .build_query_scalar()
        .fetch_one(&state.pool)
        .await?;
    if !visible {
        return Err(AppError::NotFound("目的地不存在".into()));
    }
    let (moments, next) = moments(&state, &slug, Some((time, id))).await?;
    Ok(conditional(
        &headers,
        &Rendered::new(json!({ "moments": moments, "next": next }).to_string()),
        "application/json",
    ))
}

/// "35.01°N 135.77°E"
fn coordinates(latitude: f64, longitude: f64) -> String {
    format!(
        "{:.2}°{} {:.2}°{}",
        latitude.abs(),
        if latitude < 0.0 { 'S' } else { 'N' },
        longitude.abs(),
        if longitude < 0.0 { 'W' } else { 'E' }
    )
}

/// Destinations with recent moments, the most recently visited first.
async fn map(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let since = state.digest.today() - chrono::Duration::days(MAP_DAYS - 1);
    let places = sqlx::QueryBuilder::new(
        "SELECT COALESCE(jsonb_agg(jsonb_build_object('slug', p.slug, 'name', p.name,
                 'name_en', p.name_en, 'country', p.country, 'country_en', p.country_en,
                 'latitude', round(p.latitude::numeric, 3), 'longitude', round(p.longitude::numeric, 3),
                 'moments', recent.moments) ORDER BY recent.latest DESC, p.slug), '[]')
         FROM (SELECT m.place_id, COUNT(*) AS moments, MAX(i.first_seen_at) AS latest
               FROM travel_moments m JOIN items i ON i.id = m.item_id
               WHERE NOT i.hidden AND ",
    )
    .push(VISIBLE)
    .push(" AND i.topic_id = ")
    .push_bind(TOPIC_ID)
    .push(" AND i.day >= ")
    .push_bind(since)
    .push(" GROUP BY m.place_id) recent JOIN travel_places p ON p.id = recent.place_id")
    .build_query_scalar::<Value>()
    .fetch_one(&state.pool)
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(json!({ "places": places }).to_string()),
        "application/json",
    ))
}

/// Destination pages with a guide, in both languages.
async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let rows = sqlx::QueryBuilder::new(
        "SELECT slug, guide_at FROM travel_places WHERE guide_at IS NOT NULL AND ",
    )
    .push(VISIBLE)
    .push(" ORDER BY slug LIMIT 25000")
    .build_query_as::<(String, DateTime<Utc>)>()
    .fetch_all(&state.pool)
    .await?;
    let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
    for (slug, time) in rows {
        for en in [false, true] {
            let _ = write!(
                xml,
                "<url><loc>{}</loc><lastmod>{}</lastmod></url>",
                escape(&absolute(
                    &state,
                    &localized(&format!("/travel/{slug}"), en)
                )),
                time.to_rfc3339()
            );
        }
    }
    xml.push_str("</urlset>");
    Ok(conditional(
        &headers,
        &Rendered::new(xml),
        "application/xml; charset=utf-8",
    ))
}

/// Whether the sitemap index lists the destinations.
pub(crate) async fn listed(state: &AppState) -> Result<bool> {
    Ok(sqlx::QueryBuilder::new("SELECT ")
        .push(VISIBLE)
        .build_query_scalar()
        .fetch_one(&state.pool)
        .await?)
}
