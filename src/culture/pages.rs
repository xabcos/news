use super::{Guide, IDS, ITEM_JOIN, ITEM_JSON};
use crate::{
    digest::{PublicItem, Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
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
use std::fmt::Write;
use uuid::Uuid;

const LIMIT: usize = 18;
const CONTENT: &str = "t.enabled AND t.owner_user_id IS NULL";
#[derive(Default, Deserialize)]
struct Params {
    lang: Option<String>,
    before: Option<String>,
}
impl Params {
    fn en(&self) -> bool {
        self.lang.as_deref() == Some("en")
    }
}

pub(crate) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route("/culture", get(index))
            .route("/culture/{topic}", get(collection))
            .route("/culture/{topic}/more.json", get(more))
            .route("/culture/{topic}/{slug}", get(detail)),
    )
    .route("/culture/sitemap.xml", get(sitemap))
    .layer(tower_http::compression::CompressionLayer::new())
}

async fn catalogue(state: &AppState, reader: Reader) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as("SELECT id, name FROM topics t WHERE id = ANY($1) AND t.enabled AND (t.visible OR $2) AND t.owner_user_id IS NULL ORDER BY array_position($1, id)")
        .bind(IDS.as_slice()).bind(reader.is_admin()).fetch_all(&state.pool).await?)
}
pub(crate) async fn listed(state: &AppState) -> Result<bool> {
    Ok(!catalogue(state, Reader::Public).await?.is_empty())
}
fn missing() -> AppError {
    AppError::NotFound("栏目或作品不存在".into())
}
async fn check(state: &AppState, topic: &str, reader: Reader) -> Result<Vec<(String, String)>> {
    let topics = catalogue(state, reader).await?;
    if !topics.iter().any(|(id, _)| id == topic) {
        return Err(missing());
    }
    Ok(topics)
}
async fn index(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
) -> Result<Response> {
    let topics = catalogue(&state, reader).await?;
    let (topic, _) = topics.first().ok_or_else(missing)?;
    Ok(
        Redirect::temporary(&reader.localized(&format!("/culture/{topic}"), params.en()))
            .into_response(),
    )
}
fn tabs(topics: &[(String, String)], current: &str, en: bool, reader: Reader) -> String {
    let mut html = format!(
        "<nav class=culture-tabs aria-label=\"{}\">",
        choose(en, "文娱栏目", "Culture collections")
    );
    for (id, name) in topics {
        let _ = write!(
            html,
            "<a href=\"{}\"{}>{}</a>",
            escape(&reader.localized(&format!("/culture/{id}"), en)),
            if id == current {
                " aria-current=\"true\""
            } else {
                ""
            },
            escape(name)
        );
    }
    html.push_str("</nav>");
    html
}
fn description(topic: &str, en: bool) -> &'static str {
    match topic {
        "movie" => choose(
            en,
            "银幕内外，正在发生的故事。",
            "Stories unfolding, on screen and off.",
        ),
        "music" => choose(
            en,
            "新声音，新现场，音乐背后的故事。",
            "New sounds, live moments, and the stories behind the music.",
        ),
        "game" => choose(
            en,
            "新的世界，值得探索的玩法。",
            "New worlds and ideas worth playing.",
        ),
        _ => choose(
            en,
            "从一本书，走进更大的世界。",
            "A wider world, one book at a time.",
        ),
    }
}
#[allow(clippy::too_many_arguments)]
async fn layout(
    state: &AppState,
    reader: Reader,
    topic: &str,
    en: bool,
    path: &str,
    title: &str,
    description: &str,
    content: &str,
    schema: Option<Value>,
) -> Result<String> {
    crate::site::layout(
        state,
        reader,
        en,
        path,
        title,
        description,
        content,
        schema,
        &Chrome {
            current: topic,
            section: &format!("culture culture-{topic}"),
            stylesheet: "/culture.css?v=3",
            script: "/culture.js?v=2",
            feed: ("Culture", &format!("/feed.xml?topic={topic}")),
        },
    )
    .await
}
fn cursor(value: Option<&str>) -> Result<Option<(DateTime<Utc>, Uuid)>> {
    value
        .map(|value| {
            let invalid = || AppError::BadRequest("分页参数无效".into());
            let (time, id) = value.split_once('_').ok_or_else(invalid)?;
            let time = time
                .parse()
                .ok()
                .and_then(DateTime::from_timestamp_micros)
                .ok_or_else(invalid)?;
            Ok((time, Uuid::parse_str(id).map_err(|_| invalid())?))
        })
        .transpose()
}
async fn items(
    state: &AppState,
    topic: &str,
    work: Option<Uuid>,
    before: Option<(DateTime<Utc>, Uuid)>,
) -> Result<(Vec<PublicItem>, Option<String>)> {
    let mut query = sqlx::QueryBuilder::new("SELECT items.id, items.title, items.title_en, items.summary, items.summary_en, items.url, items.source,
        items.published_at, items.first_seen_at, items.tags, items.tags_en, items.importance, items.pinned,
        items.image - 'origin_url' - 'expires_at' - 'content_hash' AS image, ");
    query
        .push(ITEM_JSON)
        .push(" AS culture FROM items JOIN topics t ON t.id = items.topic_id ")
        .push(ITEM_JOIN)
        .push(" WHERE ")
        .push(CONTENT)
        .push(" AND NOT items.hidden AND items.topic_id = ")
        .push_bind(topic);
    if let Some(work) = work {
        query.push(" AND ci.work_id = ").push_bind(work);
    }
    if let Some((time, id)) = before {
        query
            .push(" AND (items.first_seen_at, items.id) < (")
            .push_bind(time)
            .push(",")
            .push_bind(id)
            .push(")");
    }
    let mut rows = query
        .push(" ORDER BY items.first_seen_at DESC, items.id DESC LIMIT ")
        .push_bind(LIMIT as i64 + 1)
        .build_query_as::<PublicItem>()
        .fetch_all(&state.pool)
        .await?;
    let next = if rows.len() > LIMIT {
        rows.truncate(LIMIT);
        rows.last()
            .map(|item| format!("{}_{}", item.first_seen_at.timestamp_micros(), item.id))
    } else {
        None
    };
    Ok((rows, next))
}
fn card(item: &PublicItem, topic: &str, en: bool, reader: Reader) -> String {
    let title = choose(en, &item.title, &item.title_en);
    let mut html = format!("<article class=\"image-news-card culture-card culture-{topic}\">");
    if let Some(url) = item.image.as_ref().and_then(|image| image["url"].as_str()) {
        let _ = write!(
            html,
            "<a class=image-news-media href=\"{}\" target=_blank rel=\"noopener noreferrer\"><img src=\"{}\" alt=\"{}\" loading=lazy decoding=async referrerpolicy=no-referrer></a>",
            escape(&item.url),
            escape(url),
            escape(title)
        );
    }
    let at = item.published_at.unwrap_or(item.first_seen_at);
    let _ = write!(
        html,
        "<div class=culture-card-body><p class=item-meta>{} <span>·</span> <time datetime=\"{}\">{}</time></p><h2 class=item-title><a href=\"{}\" target=_blank rel=\"noopener noreferrer\">{}</a></h2><p class=item-summary>{}</p>",
        escape(&item.source),
        at.to_rfc3339(),
        at.format("%m-%d"),
        escape(&item.url),
        escape(title),
        escape(choose(en, &item.summary, &item.summary_en))
    );
    let tags = if en && !item.tags_en.is_empty() {
        &item.tags_en
    } else {
        &item.tags
    };
    html.push_str("<div class=item-tags>");
    for tag in tags {
        let _ = write!(html, "<span class=tag>{}</span>", escape(tag));
    }
    html.push_str("</div>");
    if let Some(work) = &item.culture {
        let path = work["path"].as_str().unwrap_or("");
        let name = work[choose(en, "name", "name_en")].as_str().unwrap_or("");
        let _ = write!(
            html,
            "<a class=culture-work-link href=\"{}\">{}{} <span aria-hidden=true>→</span></a>",
            escape(&reader.localized(path, en)),
            choose(en, "了解 ", "Explore "),
            escape(name)
        );
    }
    html.push_str("</div></article>");
    html
}
fn cards(rows: &[PublicItem], topic: &str, en: bool, reader: Reader) -> String {
    rows.iter()
        .map(|item| card(item, topic, en, reader))
        .collect()
}
fn pager(topic: &str, next: Option<&str>, en: bool, reader: Reader) -> String {
    next.map(|next| format!("<a class=\"button button-secondary culture-more\" href=\"{}\" data-before=\"{}\" data-source=\"{}\">{}</a>",
        escape(&reader.localized(&format!("/culture/{topic}?before={next}"), en)), escape(next), escape(&reader.path(&format!("/culture/{topic}/more.json"))), choose(en,"更多动态","More stories"))).unwrap_or_default()
}
async fn collection(
    State(state): State<AppState>,
    reader: Reader,
    Path(topic): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let topics = check(&state, &topic, reader).await?;
    let en = params.en();
    let (rows, next) = items(&state, &topic, None, cursor(params.before.as_deref())?).await?;
    let name = topics
        .iter()
        .find(|(id, _)| id == &topic)
        .map(|(_, name)| name.as_str())
        .ok_or_else(missing)?;
    let mut content = tabs(&topics, &topic, en, reader);
    let _ = write!(
        content,
        "<div class=culture-grid>{}</div>",
        cards(&rows, &topic, en, reader)
    );
    if rows.is_empty() {
        let _ = write!(
            content,
            "<p class=empty>{}</p>",
            choose(en, "新故事正在路上。", "New stories are on their way.")
        );
    }
    content.push_str(&pager(&topic, next.as_deref(), en, reader));
    let path = format!("/culture/{topic}");
    let html = layout(
        &state,
        reader,
        &topic,
        en,
        &path,
        &format!("{name} · Culture · News"),
        description(&topic, en),
        &content,
        None,
    )
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}
async fn more(
    State(state): State<AppState>,
    reader: Reader,
    Path(topic): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    check(&state, &topic, reader).await?;
    let (rows, next) = items(&state, &topic, None, cursor(params.before.as_deref())?).await?;
    Ok(conditional(
        &headers,
        &Rendered::new(
            json!({"html":cards(&rows, &topic, params.en(), reader),"next":next}).to_string(),
        ),
        "application/json",
    ))
}
#[derive(sqlx::FromRow)]
struct Work {
    id: Uuid,
    name: String,
    name_en: String,
    kind: String,
    image: Option<Value>,
    guide: sqlx::types::Json<Guide>,
    guide_at: DateTime<Utc>,
}
fn paragraphs(text: &str) -> String {
    text.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("<p>{}</p>", escape(p)))
        .collect()
}
async fn detail(
    State(state): State<AppState>,
    reader: Reader,
    Path((topic, slug)): Path<(String, String)>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let topics = check(&state, &topic, reader).await?;
    let en = params.en();
    let work: Work = sqlx::query_as("SELECT id, name, name_en, kind, image, guide, guide_at FROM culture_works WHERE topic_id = $1 AND slug = $2 AND guide IS NOT NULL")
        .bind(&topic).bind(&slug).fetch_optional(&state.pool).await?.ok_or_else(missing)?;
    let name = choose(en, &work.name, &work.name_en);
    let guide = &work.guide.0;
    let intro = choose(en, &guide.intro, &guide.intro_en);
    let path = format!("/culture/{topic}/{slug}");
    let mut content = tabs(&topics, &topic, en, reader);
    content.push_str("<article class=culture-work><header class=culture-work-heading>");
    if let Some(url) = work.image.as_ref().and_then(|image| image["url"].as_str()) {
        let _ = write!(
            content,
            "<img class=culture-cover src=\"{}\" alt=\"{}\" fetchpriority=high decoding=async referrerpolicy=no-referrer>",
            escape(url),
            escape(name)
        );
    }
    let _ = write!(
        content,
        "<div><p class=culture-eyebrow>{}</p><h1>{}</h1><p class=culture-original>{}</p><div class=culture-intro>{}</div></div></header>",
        escape(&work.kind),
        escape(name),
        escape(choose(en, &work.name_en, &work.name)),
        paragraphs(intro)
    );
    if !guide.facts.is_empty() {
        content.push_str("<dl class=culture-facts>");
        for entry in &guide.facts {
            let _ = write!(
                content,
                "<div><dt>{}</dt><dd>{} <a class=culture-citation href=\"#source-{}\">[{}]</a></dd></div>",
                escape(choose(en, &entry.label, &entry.label_en)),
                escape(choose(en, &entry.text, &entry.text_en)),
                entry.source,
                entry.source + 1
            );
        }
        content.push_str("</dl>");
    }
    if !guide.links.is_empty() {
        content.push_str("<div class=culture-links>");
        for link in &guide.links {
            let _ = write!(
                content,
                "<a class=\"button button-secondary\" href=\"{}\" target=_blank rel=\"noopener noreferrer\">{} ↗</a>",
                escape(&link.url),
                escape(choose(en, &link.label, &link.label_en))
            );
        }
        content.push_str("</div>");
    }
    content.push_str("<div class=culture-reading>");
    for entry in &guide.sections {
        let _ = write!(
            content,
            "<section><h2>{}</h2>{}<a class=culture-citation href=\"#source-{}\">[{}]</a></section>",
            escape(choose(en, &entry.label, &entry.label_en)),
            paragraphs(choose(en, &entry.text, &entry.text_en)),
            entry.source,
            entry.source + 1
        );
    }
    let _ = write!(
        content,
        "<footer class=culture-sources><p>{} · {}</p><ol>",
        choose(en, "资料导读", "About this work"),
        work.guide_at.format("%Y-%m-%d")
    );
    let mut wiki = false;
    for (i, source) in guide.sources.iter().enumerate() {
        let _ = write!(
            content,
            "<li id=source-{i}><a href=\"{}\" target=_blank rel=\"noopener noreferrer\">{}</a></li>",
            escape(&source.url),
            escape(&source.title)
        );
        wiki |= url::Url::parse(&source.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .is_some_and(|h| h.ends_with(".wikipedia.org"));
    }
    content.push_str("</ol>");
    if wiki {
        content.push_str("<p>Wikipedia · <a href=\"https://creativecommons.org/licenses/by-sa/4.0/\" rel=license>CC BY-SA 4.0</a></p>");
    }
    content.push_str("</footer></div></article>");
    let (news, _) = items(&state, &topic, Some(work.id), None).await?;
    if !news.is_empty() {
        let _ = write!(
            content,
            "<section class=culture-related><h2>{}</h2><div class=culture-grid>{}</div></section>",
            choose(en, "最新动态", "Latest stories"),
            cards(&news, &topic, en, reader)
        );
    }
    let _ = write!(
        content,
        "<p class=culture-back><a href=\"{}\">← {}</a></p>",
        escape(&reader.localized(&format!("/culture/{topic}"), en)),
        choose(en, "回到栏目", "Back to collection")
    );
    let schema = json!({"@context":"https://schema.org","@type":match topic.as_str(){"book"=>"Book","game"=>"VideoGame","music"=>"CreativeWork",_=>if work.kind=="series"{"TVSeries"}else{"Movie"}},"name":name,"description":intro,"url":absolute(&state,&localized(&path,en)),"dateModified":work.guide_at.to_rfc3339()});
    let html = layout(
        &state,
        reader,
        &topic,
        en,
        &path,
        &format!("{name} · Culture · News"),
        intro,
        &content,
        Some(schema),
    )
    .await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}
async fn sitemap(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let rows: Vec<(String,String,DateTime<Utc>)> = sqlx::query_as("SELECT w.topic_id, w.slug, w.guide_at FROM culture_works w JOIN topics t ON t.id = w.topic_id WHERE t.enabled AND t.visible AND t.owner_user_id IS NULL AND w.guide IS NOT NULL ORDER BY w.topic_id,w.slug LIMIT 25000")
        .fetch_all(&state.pool).await?;
    let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
    for (topic, slug, at) in rows {
        for en in [false, true] {
            let _ = write!(
                xml,
                "<url><loc>{}</loc><lastmod>{}</lastmod></url>",
                escape(&absolute(
                    &state,
                    &localized(&format!("/culture/{topic}/{slug}"), en)
                )),
                at.to_rfc3339()
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
