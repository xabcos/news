//! A page per destination at `/travel/{slug}` with its guide and moments, all of them or only
//! those mainly of people (`?kind=portraits`), which load page by page from
//! `/travel/{slug}/moments`, then the destinations nearby and in the same country to go on to;
//! its guide for the photograph viewer (`/travel/{slug}/guide.json`); the map's data with the
//! favorite moments shown beside it, and the sitemap. The feed itself is the 𝑻𝒓𝒂𝒗𝒆𝒍 tab of the news
//! page.

use super::{
    MOMENT_JOIN, MOMENT_JSON, PORTRAIT, TOPIC_ID,
    guide::{Entry, Guide},
};
use crate::{
    digest::{Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{After, Chrome, absolute, choose, escape, localized, next_page, parse_after, push_page},
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
    stylesheet: "/travel.css?v=5",
    script: "/travel.js?v=11",
    feed: ("𝑻𝒓𝒂𝒗𝒆𝒍", "/feed.xml?topic=travel"),
};
/// Map pins of destinations with moments from this many recent days stand out, and the most
/// liked of those moments, one per destination, are offered to show beside the map.
const MAP_DAYS: i64 = 7;
const FEATURED: i64 = 6;
/// Moments per page of a destination, newest first.
const PAGE_MOMENTS: usize = 24;
/// Destinations to go on to: the nearest with a photograph, then names in the same country.
const NEARBY: i64 = 6;
const SAME_COUNTRY: i64 = 40;
/// The `kind` that narrows a destination's moments to those mainly of people.
const PORTRAITS: &str = "portraits";

pub(crate) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route("/travel", get(index))
            .route("/travel/map.json", get(map))
            .route("/travel/{slug}", get(destination))
            .route("/travel/{slug}/moments", get(more))
            .route("/travel/{slug}/guide.json", get(guide)),
    )
    .route("/travel/sitemap.xml", get(sitemap))
    .layer(tower_http::compression::CompressionLayer::new())
}

#[derive(Default, Deserialize)]
struct Params {
    lang: Option<String>,
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Page {
    before: String,
    kind: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Place {
    slug: String,
    country_id: String,
    name: String,
    name_en: String,
    country: String,
    country_en: String,
    latitude: f64,
    longitude: f64,
    guide: Option<sqlx::types::Json<Guide>>,
}

/// The feed lives on the news page.
async fn index(_reader: Reader, Query(params): Query<Params>) -> Response {
    let en = params.lang.as_deref() == Some("en");
    Redirect::temporary(&localized("/?topic=travel", en)).into_response()
}

async fn destination(
    State(state): State<AppState>,
    reader: Reader,
    Path(slug): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    reader.check(&state, TOPIC_ID)?;
    let en = params.lang.as_deref() == Some("en");
    let missing = || AppError::NotFound("目的地不存在".into());
    let place = sqlx::QueryBuilder::new(
        "SELECT slug, country_id, name, name_en, country, country_en, latitude, longitude, guide
         FROM travel_places WHERE slug = ",
    )
    .push_bind(&slug)
    .build_query_as::<Place>()
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(missing)?;
    let (total, portraits) =
        sqlx::QueryBuilder::new("SELECT COUNT(*), COUNT(*) FILTER (WHERE i.tags[1] = ")
            .push_bind(PORTRAIT)
            .push(
                ") FROM travel_moments m JOIN items i ON i.id = m.item_id
                 JOIN travel_places p ON p.id = m.place_id WHERE NOT i.hidden AND p.slug = ",
            )
            .push_bind(&slug)
            .build_query_as::<(i64, i64)>()
            .fetch_one(&state.pool)
            .await?;
    // Portraits are offered beside all moments only when they narrow the list.
    let choice = portraits > 0 && portraits < total;
    let only_portraits = choice && params.kind.as_deref() == Some(PORTRAITS);
    let (moments, next) = moments(&state, &slug, only_portraits, None).await?;
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
            "</dl><p class=destination-sources>{}{sources} · <a href=\"{}\" rel=\"license noopener noreferrer\">CC BY-SA 4.0</a></p></section>",
            choose(en, "资料：", "Sources: "),
            choose(
                en,
                "https://creativecommons.org/licenses/by-sa/4.0/deed.zh-hans",
                "https://creativecommons.org/licenses/by-sa/4.0/",
            ),
        );
    }
    if !moments.is_empty() {
        let data = serde_json::to_string(&moments)?
            .replace('&', "\\u0026")
            .replace('<', "\\u003c")
            .replace('>', "\\u003e");
        // The filters link to the moments themselves, below the guide.
        let kinds = if choice {
            let link = |kind: Option<&str>, label: String| {
                let target =
                    kind.map_or_else(|| path.clone(), |kind| format!("{path}?kind={kind}"));
                format!(
                    "<a href=\"{}#photos\"{}>{label}</a>",
                    escape(&localized(&target, en)),
                    if kind.is_some() == only_portraits {
                        " aria-current=\"true\""
                    } else {
                        ""
                    }
                )
            };
            format!(
                "<nav class=moment-kinds>{}{}</nav>",
                link(None, choose(en, "全部", "All").to_owned()),
                link(
                    Some(PORTRAITS),
                    format!("{} {portraits}", choose(en, "人像", "Portraits"))
                )
            )
        } else {
            String::new()
        };
        let mut source = format!("{path}/moments");
        if only_portraits {
            let _ = write!(source, "?kind={PORTRAITS}");
        }
        let _ = write!(
            content,
            "<section class=destination-moments id=photos><h2>{} <span>{total}</span></h2>{kinds}<div class=\"postcards\" data-moments data-source=\"{}\"{}></div><script type=\"application/json\" id=moments>{data}</script></section>",
            choose(en, "旅人瞬间", "Moments"),
            escape(&source),
            next.map_or_else(String::new, |next| format!(" data-next=\"{next}\"")),
        );
    }
    content.push_str(&onward(&state, &place, en).await?);
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
        reader,
        en,
        &path,
        &title,
        &description,
        &content,
        Some(structured),
        &CHROME,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// Where to go next: the nearest destinations with a photograph, by great-circle distance, then
/// the other destinations of the same country, the most visited first. Only destinations with
/// a page are named: a guide or a moment.
async fn onward(state: &AppState, place: &Place, en: bool) -> Result<String> {
    let nearby = sqlx::query_as::<_, (String, String, String, String, String, f64, Value)>(
        "SELECT n.slug, n.name, n.name_en, n.country, n.country_en, n.km, c.image
         FROM (SELECT p.id, p.slug, p.name, p.name_en, p.country, p.country_en,
                      12742 * asin(sqrt(LEAST(1, power(sin(radians(p.latitude - $1) / 2), 2)
                          + cos(radians($1)) * cos(radians(p.latitude))
                          * power(sin(radians(p.longitude - $2) / 2), 2)))) AS km
               FROM travel_places p
               WHERE p.slug <> $3 AND EXISTS (SELECT 1 FROM travel_moments m
                   JOIN items i ON i.id = m.item_id WHERE m.place_id = p.id AND NOT i.hidden
                   AND jsonb_typeof(i.image) = 'object')
               ORDER BY km LIMIT $4) n
         CROSS JOIN LATERAL (SELECT i.image - 'origin_url' - 'content_hash' - 'expires_at' AS image
               FROM travel_moments m JOIN items i ON i.id = m.item_id
               WHERE m.place_id = n.id AND NOT i.hidden AND jsonb_typeof(i.image) = 'object'
               ORDER BY i.importance DESC, i.first_seen_at DESC LIMIT 1) c
         ORDER BY n.km",
    )
    .bind(place.latitude)
    .bind(place.longitude)
    .bind(&place.slug)
    .bind(NEARBY)
    .fetch_all(&state.pool)
    .await?;
    let shown = nearby
        .iter()
        .map(|(slug, ..)| slug.as_str())
        .chain([place.slug.as_str()])
        .collect::<Vec<_>>();
    let country = sqlx::query_as::<_, (String, String, String)>(
        "SELECT p.slug, p.name, p.name_en FROM travel_places p
         CROSS JOIN LATERAL (SELECT COUNT(*) AS moments FROM travel_moments m
             JOIN items i ON i.id = m.item_id WHERE m.place_id = p.id AND NOT i.hidden) v
         WHERE p.country_id = $1 AND p.slug <> ALL($2) AND (p.guide IS NOT NULL OR v.moments > 0)
         ORDER BY v.moments DESC, p.slug LIMIT $3",
    )
    .bind(&place.country_id)
    .bind(&shown)
    .bind(SAME_COUNTRY)
    .fetch_all(&state.pool)
    .await?;
    if nearby.is_empty() && country.is_empty() {
        return Ok(String::new());
    }
    let mut html = format!(
        "<section class=destination-nearby><h2>{}</h2>",
        choose(en, "附近", "Nearby")
    );
    if !nearby.is_empty() {
        html.push_str("<div class=nearby>");
        for (slug, name, name_en, country, country_en, km, image) in &nearby {
            let name = choose(en, name, name_en);
            let country = choose(en, country, country_en);
            let _ = write!(
                html,
                "<a class=nearby-card href=\"{}\"><img src=\"{}\" alt=\"{}\"{} loading=lazy decoding=async referrerpolicy=no-referrer><span class=nearby-name>{}</span><span class=nearby-meta>{}{} km</span></a>",
                escape(&localized(&format!("/travel/{slug}"), en)),
                escape(image["url"].as_str().unwrap_or_default()),
                escape(
                    image[choose(en, "caption", "caption_en")]
                        .as_str()
                        .unwrap_or(name)
                ),
                match (image["width"].as_u64(), image["height"].as_u64()) {
                    (Some(width), Some(height)) => format!(" width={width} height={height}"),
                    _ => String::new(),
                },
                escape(name),
                if country == name {
                    String::new()
                } else {
                    format!("{} · ", escape(country))
                },
                km.max(1.0).round(),
            );
        }
        html.push_str("</div>");
    }
    if !country.is_empty() {
        let label = choose(en, &place.country, &place.country_en);
        let _ = write!(
            html,
            "<p class=destination-more><span>{}</span>",
            escape(&if en {
                format!("More in {label}")
            } else {
                format!("{label}的更多目的地")
            })
        );
        for (slug, name, name_en) in &country {
            let _ = write!(
                html,
                "<a href=\"{}\">{}</a>",
                escape(&localized(&format!("/travel/{slug}"), en)),
                escape(choose(en, name, name_en))
            );
        }
        html.push_str("</p>");
    }
    html.push_str("</section>");
    Ok(html)
}

/// The guide below the tagline: a fact to share, then each section the sources supported.
fn guide_sections(guide: &Guide, en: bool) -> String {
    let mut html = format!(
        "<section class=destination-guide><p class=destination-tagline>{}</p>",
        escape(choose(en, &guide.tagline, &guide.tagline_en))
    );
    let fact = choose(en, &guide.fact, &guide.fact_en);
    if !fact.is_empty() {
        let _ = write!(html, "<p class=destination-fact>{}</p>", escape(fact));
    }
    html.push_str("<dl class=destination-sections>");
    let mut row = |label: &str, value: String| {
        if !value.is_empty() {
            let _ = write!(html, "<div><dt>{label}</dt><dd>{value}</dd></div>");
        }
    };
    let text = |zh: &str, english: &str| escape(choose(en, zh, english));
    // A long history comes in paragraphs, one per line.
    let paragraphs = |zh: &str, english: &str| {
        choose(en, zh, english)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| format!("<p>{}</p>", escape(line)))
            .collect::<String>()
    };
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
        paragraphs(&guide.history, &guide.history_en),
    );
    row(choose(en, "人物", "People"), list(&guide.people));
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

/// One page of a destination's moments, or of those mainly of people, newest first, and the
/// cursor of the next page.
async fn moments(
    state: &AppState,
    slug: &str,
    portraits: bool,
    before: Option<After>,
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
    if portraits {
        query.push(" AND items.tags[1] = ").push_bind(PORTRAIT);
    }
    push_page(&mut query, before, PAGE_MOMENTS);
    let mut rows = query
        .build_query_as::<(DateTime<Utc>, Uuid, Value)>()
        .fetch_all(&state.pool)
        .await?;
    let next = next_page(&mut rows, PAGE_MOMENTS, |(time, id, _)| (*time, *id));
    Ok((rows.into_iter().map(|(.., moment)| moment).collect(), next))
}

/// The next page of a destination's moments for the page's endless scrolling.
async fn more(
    State(state): State<AppState>,
    reader: Reader,
    Path(slug): Path<String>,
    Query(page): Query<Page>,
    headers: HeaderMap,
) -> Result<Response> {
    let after = parse_after(&page.before)?;
    reader.check(&state, TOPIC_ID)?;
    let portraits = page.kind.as_deref() == Some(PORTRAITS);
    let (moments, next) = moments(&state, &slug, portraits, Some(after)).await?;
    Ok(conditional(
        &headers,
        &Rendered::new(json!({ "moments": moments, "next": next }).to_string()),
        "application/json",
    ))
}

/// A destination's guide for the photograph viewer, which shows one part of it beside each
/// photograph. Its sources stay on the destination page.
async fn guide(
    State(state): State<AppState>,
    reader: Reader,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Response> {
    reader.check(&state, TOPIC_ID)?;
    let guide = sqlx::query_scalar::<_, Value>(
        "SELECT guide - 'sources' FROM travel_places WHERE slug = $1 AND guide IS NOT NULL",
    )
    .bind(&slug)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::NotFound("目的地没有导读".into()))?;
    Ok(conditional(
        &headers,
        &Rendered::new(json!({ "guide": guide }).to_string()),
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

/// Every destination with a page, the most recently visited first; those with moments from the
/// last `MAP_DAYS` days are marked recent. Beside them, the most liked moments of those days, one
/// per destination; likes are counted when X confirms a post, so older moments without them
/// follow by importance and time.
async fn map(
    State(state): State<AppState>,
    reader: Reader,
    headers: HeaderMap,
) -> Result<Response> {
    reader.check(&state, TOPIC_ID)?;
    let since = state.digest.today() - chrono::Duration::days(MAP_DAYS - 1);
    let places = sqlx::QueryBuilder::new(
        "SELECT COALESCE(jsonb_agg(jsonb_build_object('slug', p.slug, 'name', p.name,
                 'name_en', p.name_en, 'country', p.country, 'country_en', p.country_en,
                 'latitude', round(p.latitude::numeric, 3), 'longitude', round(p.longitude::numeric, 3),
                 'moments', COALESCE(visited.moments, 0), 'recent', COALESCE(visited.last_day >= ",
    )
    .push_bind(since)
    .push(
        ", false)) ORDER BY visited.latest DESC NULLS LAST, p.slug), '[]')
         FROM travel_places p LEFT JOIN (
             SELECT m.place_id, COUNT(*) AS moments, MAX(i.first_seen_at) AS latest, MAX(i.day) AS last_day
             FROM travel_moments m JOIN items i ON i.id = m.item_id
             WHERE NOT i.hidden AND i.topic_id = ",
    )
    .push_bind(TOPIC_ID)
    .push(
        " GROUP BY m.place_id) visited ON visited.place_id = p.id
         WHERE p.guide IS NOT NULL OR visited.place_id IS NOT NULL",
    )
    .build_query_scalar::<Value>()
    .fetch_one(&state.pool)
    .await?;
    let featured = sqlx::query_scalar::<_, Value>(
        "SELECT COALESCE(jsonb_agg(moment ORDER BY likes DESC NULLS LAST, importance DESC, first_seen_at DESC), '[]')
         FROM (SELECT * FROM (
                 SELECT DISTINCT ON (m.place_id) m.likes, i.importance, i.first_seen_at,
                        jsonb_build_object('slug', p.slug, 'name', p.name, 'name_en', p.name_en,
                            'country', p.country, 'country_en', p.country_en, 'title', i.title,
                            'title_en', i.title_en, 'likes', m.likes,
                            'photo', i.image - 'origin_url' - 'content_hash' - 'expires_at') AS moment
                 FROM travel_moments m JOIN items i ON i.id = m.item_id
                 JOIN travel_places p ON p.id = m.place_id
                 WHERE NOT i.hidden AND i.topic_id = $1 AND i.day >= $2
                   AND jsonb_typeof(i.image) = 'object'
                 ORDER BY m.place_id, m.likes DESC NULLS LAST, i.importance DESC, i.first_seen_at DESC
               ) best
               ORDER BY likes DESC NULLS LAST, importance DESC, first_seen_at DESC LIMIT $3) chosen",
    )
    .bind(TOPIC_ID)
    .bind(since)
    .bind(FEATURED)
    .fetch_one(&state.pool)
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(json!({ "places": places, "featured": featured }).to_string()),
        "application/json",
    ))
}

/// Destination pages with a guide, in both languages.
async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let rows = if listed(&state) {
        sqlx::query_as::<_, (String, DateTime<Utc>)>(
            "SELECT slug, guide_at FROM travel_places WHERE guide_at IS NOT NULL ORDER BY slug LIMIT 25000",
        )
        .fetch_all(&state.pool)
        .await?
    } else {
        Vec::new()
    };
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
pub(crate) fn listed(state: &AppState) -> bool {
    Reader::Public.check(state, TOPIC_ID).is_ok()
}
