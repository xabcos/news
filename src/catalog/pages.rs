//! Each section's pages: `/{section}/{topic}` lists a topic's items, `/{section}/{topic}/{slug}`
//! shows an entry with its guide and items, and `more.json` pages the list. 𝑮𝒂𝒅𝒈𝒆𝒕𝒔 adds a
//! shelf of recently active products, filters by kind and keeps reviews and shares apart.
use super::{Guide, ITEM_JOIN, Section, culture, gadgets, item_json};
use crate::{
    digest::{PublicItem, Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{Chrome, absolute, choose, escape, localized},
    state::AppState,
};
use axum::{
    Extension, Router,
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
/// Products on the shelf of a 𝑮𝒂𝒅𝒈𝒆𝒕𝒔 topic.
const SHELF: i64 = 12;
const CONTENT: &str = "t.enabled AND t.owner_user_id IS NULL";
/// The groups of an entry's items, in page order.
const ROLES: [&str; 3] = ["review", "share", "news"];

#[derive(Default, Deserialize)]
struct Params {
    lang: Option<String>,
    before: Option<String>,
    kind: Option<String>,
}
impl Params {
    fn en(&self) -> bool {
        self.lang.as_deref() == Some("en")
    }
}

pub(crate) fn router() -> Router<AppState> {
    Section::ALL
        .into_iter()
        .fold(Router::new(), |router, section| {
            let base = format!("/{}", section.path());
            router.merge(
                reading::pages(
                    Router::new()
                        .route(&base, get(index))
                        .route(&format!("{base}/{{topic}}"), get(collection))
                        .route(&format!("{base}/{{topic}}/more.json"), get(more))
                        .route(&format!("{base}/{{topic}}/{{slug}}"), get(detail)),
                )
                .route(&format!("{base}/sitemap.xml"), get(sitemap))
                .layer(Extension(section)),
            )
        })
        .layer(tower_http::compression::CompressionLayer::new())
}

async fn catalogue(
    state: &AppState,
    section: Section,
    reader: Reader,
) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as("SELECT id, name FROM topics t WHERE id = ANY($1) AND t.enabled AND (t.visible OR $2) AND t.owner_user_id IS NULL ORDER BY array_position($1, id)")
        .bind(section.topics()).bind(reader.is_admin()).fetch_all(&state.pool).await?)
}
/// The sections with a public topic, for the sitemap index.
pub(crate) async fn listed(state: &AppState) -> Result<Vec<Section>> {
    let mut sections = Vec::new();
    for section in Section::ALL {
        if !catalogue(state, section, Reader::Public).await?.is_empty() {
            sections.push(section);
        }
    }
    Ok(sections)
}
fn missing() -> AppError {
    AppError::NotFound("栏目或条目不存在".into())
}
async fn check(
    state: &AppState,
    section: Section,
    topic: &str,
    reader: Reader,
) -> Result<Vec<(String, String)>> {
    let topics = catalogue(state, section, reader).await?;
    if !topics.iter().any(|(id, _)| id == topic) {
        return Err(missing());
    }
    Ok(topics)
}
async fn index(
    Extension(section): Extension<Section>,
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
) -> Result<Response> {
    let topics = catalogue(&state, section, reader).await?;
    let (topic, _) = topics.first().ok_or_else(missing)?;
    Ok(
        Redirect::temporary(
            &reader.localized(&format!("/{}/{topic}", section.path()), params.en()),
        )
        .into_response(),
    )
}
fn tabs(
    section: Section,
    topics: &[(String, String)],
    current: &str,
    en: bool,
    reader: Reader,
) -> String {
    let mut html = format!(
        "<nav class=catalog-tabs aria-label=\"{}\">",
        match section {
            Section::Culture => choose(en, "文娱栏目", "Culture collections"),
            Section::Gadgets => choose(en, "数码栏目", "Gadgets collections"),
        }
    );
    for (id, name) in topics {
        let _ = write!(
            html,
            "<a href=\"{}\"{}>{}</a>",
            escape(&reader.localized(&format!("/{}/{id}", section.path()), en)),
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
fn description(section: Section, topic: &str, en: bool) -> &'static str {
    match section {
        Section::Culture => culture::description(topic, en),
        Section::Gadgets => gadgets::description(topic, en),
    }
}
#[allow(clippy::too_many_arguments)]
async fn layout(
    state: &AppState,
    reader: Reader,
    section: Section,
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
            section: &format!("catalog {} catalog-{topic}", section.path()),
            stylesheet: "/catalog.css?v=1",
            script: "/catalog.js?v=1",
            feed: (section.label(), &format!("/feed.xml?topic={topic}")),
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
/// One page of a topic's items, newest first: all of them, those of one kind of entry, or
/// those of one entry in one role.
async fn items(
    state: &AppState,
    section: Section,
    topic: &str,
    filter: Filter<'_>,
    before: Option<(DateTime<Utc>, Uuid)>,
) -> Result<(Vec<PublicItem>, Option<String>)> {
    let mut query = sqlx::QueryBuilder::new("SELECT items.id, items.title, items.title_en, items.summary, items.summary_en, items.url, items.source,
        items.published_at, items.first_seen_at, items.tags, items.tags_en, items.importance, items.pinned,
        items.image - 'origin_url' - 'expires_at' - 'content_hash' AS image, ");
    query
        .push(item_json(section))
        .push(" AS catalog FROM items JOIN topics t ON t.id = items.topic_id ")
        .push(ITEM_JOIN)
        .push(" WHERE ")
        .push(CONTENT)
        .push(" AND NOT items.hidden AND items.topic_id = ")
        .push_bind(topic);
    match filter {
        Filter::All => {}
        Filter::Kind(kind) => {
            query.push(" AND ce.kind = ").push_bind(kind);
        }
        Filter::Entry(entry, role) => {
            query
                .push(" AND ci.entry_id = ")
                .push_bind(entry)
                .push(" AND ci.role = ")
                .push_bind(role);
        }
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
#[derive(Clone, Copy)]
enum Filter<'a> {
    All,
    Kind(&'a str),
    Entry(Uuid, &'a str),
}
fn card(item: &PublicItem, topic: &str, en: bool, reader: Reader) -> String {
    let title = choose(en, &item.title, &item.title_en);
    let mut html = format!("<article class=\"image-news-card catalog-card catalog-{topic}\">");
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
    let entry = item.catalog.as_ref();
    let role = match entry.and_then(|entry| entry["role"].as_str()) {
        Some("review") => choose(en, "评测", "Review"),
        Some("share") => choose(en, "分享", "Shared"),
        _ => "",
    };
    let _ = write!(
        html,
        "<div class=catalog-card-body><p class=item-meta>{}{} <span>·</span> <time datetime=\"{}\">{}</time></p><h2 class=item-title><a href=\"{}\" target=_blank rel=\"noopener noreferrer\">{}</a></h2><p class=item-summary>{}</p>",
        if role.is_empty() {
            String::new()
        } else {
            format!("<span class=\"badge catalog-role\">{role}</span> ")
        },
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
    if let Some(entry) = entry
        && let Some(path) = entry["path"].as_str()
    {
        let name = entry[choose(en, "name", "name_en")].as_str().unwrap_or("");
        let _ = write!(
            html,
            "<a class=catalog-work-link href=\"{}\">{}{} <span aria-hidden=true>→</span></a>",
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
/// The path of a topic's list, with its kind filter.
fn list_path(section: Section, topic: &str, kind: Option<&str>) -> String {
    let path = format!("/{}/{topic}", section.path());
    match kind {
        Some(kind) => format!("{path}?kind={kind}"),
        None => path,
    }
}
fn pager(
    section: Section,
    topic: &str,
    kind: Option<&str>,
    next: Option<&str>,
    en: bool,
    reader: Reader,
) -> String {
    next.map(|next| {
        let separator = if kind.is_some() { '&' } else { '?' };
        format!("<a class=\"button button-secondary catalog-more\" href=\"{}\" data-before=\"{}\" data-source=\"{}\">{}</a>",
            escape(&reader.localized(&format!("{}{separator}before={next}", list_path(section, topic, kind)), en)),
            escape(next),
            escape(&reader.path(&format!("/{}/{topic}/more.json{}", section.path(), kind.map(|kind| format!("?kind={kind}")).unwrap_or_default()))),
            choose(en, "更多动态", "More stories"))
    })
    .unwrap_or_default()
}
/// A kind the topic knows, from the query.
fn kind<'a>(section: Section, topic: &str, params: &'a Params) -> Option<&'a str> {
    params
        .kind
        .as_deref()
        .filter(|kind| section == Section::Gadgets && section.kinds(topic).contains(kind))
}
/// Kind filters and the shelf of recently active products of a 𝑮𝒂𝒅𝒈𝒆𝒕𝒔 topic.
async fn shelf(
    state: &AppState,
    topic: &str,
    kind: Option<&str>,
    en: bool,
    reader: Reader,
) -> Result<String> {
    let mut html = String::from("<nav class=catalog-kinds>");
    let filters = std::iter::once((None, choose(en, "全部", "All")))
        .chain(gadgets::labels(topic, en).map(|(id, label)| (Some(id), label)));
    for (id, label) in filters {
        let _ = write!(
            html,
            "<a href=\"{}\"{}>{}</a>",
            escape(&reader.localized(&list_path(Section::Gadgets, topic, id), en)),
            if id == kind {
                " aria-current=\"true\""
            } else {
                ""
            },
            escape(label)
        );
    }
    html.push_str("</nav>");
    let products: Vec<Product> = sqlx::query_as(
        "SELECT e.slug, e.name, e.name_en, e.kind, e.image, e.guide FROM catalog_entries e
         WHERE e.topic_id = $1 AND e.guide IS NOT NULL AND ($2::text IS NULL OR e.kind = $2)
         ORDER BY (SELECT MAX(i.first_seen_at) FROM catalog_items c JOIN items i ON i.id = c.item_id
                   WHERE c.entry_id = e.id AND NOT i.hidden) DESC NULLS LAST, e.guide_at DESC
         LIMIT $3",
    )
    .bind(topic)
    .bind(kind)
    .bind(SHELF)
    .fetch_all(&state.pool)
    .await?;
    if products.is_empty() {
        return Ok(html);
    }
    let _ = write!(
        html,
        "<section class=catalog-shelf><h2>{}</h2><div class=catalog-products>",
        choose(en, "产品", "Products")
    );
    for Product {
        slug,
        name,
        name_en,
        kind,
        image,
        guide,
    } in products
    {
        let specs = guide
            .0
            .facts
            .iter()
            .take(3)
            .map(|fact| escape(choose(en, &fact.text, &fact.text_en)))
            .collect::<Vec<_>>()
            .join(" · ");
        let _ = write!(
            html,
            "<a class=catalog-product href=\"{}\">{}<span class=catalog-product-kind>{}</span><strong>{}</strong><span class=catalog-product-specs>{specs}</span></a>",
            escape(&reader.localized(&format!("/gadgets/{topic}/{slug}"), en)),
            image
                .as_ref()
                .and_then(|image| image["url"].as_str())
                .map(|url| format!("<img src=\"{}\" alt=\"\" loading=lazy decoding=async referrerpolicy=no-referrer>", escape(url)))
                .unwrap_or_default(),
            gadgets::label(topic, &kind, en),
            escape(choose(en, &name, &name_en)),
        );
    }
    let _ = write!(
        html,
        "</div></section><h2 class=catalog-latest>{}</h2>",
        choose(en, "最新内容", "Latest")
    );
    Ok(html)
}
#[derive(sqlx::FromRow)]
struct Product {
    slug: String,
    name: String,
    name_en: String,
    kind: String,
    image: Option<Value>,
    guide: sqlx::types::Json<Guide>,
}
async fn collection(
    Extension(section): Extension<Section>,
    State(state): State<AppState>,
    reader: Reader,
    Path(topic): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let topics = check(&state, section, &topic, reader).await?;
    let en = params.en();
    let kind = kind(section, &topic, &params);
    let filter = kind.map_or(Filter::All, Filter::Kind);
    let (rows, next) = items(
        &state,
        section,
        &topic,
        filter,
        cursor(params.before.as_deref())?,
    )
    .await?;
    let name = topics
        .iter()
        .find(|(id, _)| id == &topic)
        .map(|(_, name)| name.as_str())
        .ok_or_else(missing)?;
    let mut content = tabs(section, &topics, &topic, en, reader);
    if section == Section::Gadgets {
        content.push_str(&shelf(&state, &topic, kind, en, reader).await?);
    }
    let _ = write!(
        content,
        "<div class=catalog-grid>{}</div>",
        cards(&rows, &topic, en, reader)
    );
    if rows.is_empty() {
        let _ = write!(
            content,
            "<p class=empty>{}</p>",
            choose(en, "新故事正在路上。", "New stories are on their way.")
        );
    }
    content.push_str(&pager(section, &topic, kind, next.as_deref(), en, reader));
    let path = list_path(section, &topic, kind);
    let html = layout(
        &state,
        reader,
        section,
        &topic,
        en,
        &path,
        &format!("{name} · {} · News", section.label()),
        description(section, &topic, en),
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
    Extension(section): Extension<Section>,
    State(state): State<AppState>,
    reader: Reader,
    Path(topic): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    check(&state, section, &topic, reader).await?;
    let filter = kind(section, &topic, &params).map_or(Filter::All, Filter::Kind);
    let (rows, next) = items(
        &state,
        section,
        &topic,
        filter,
        cursor(params.before.as_deref())?,
    )
    .await?;
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
    creator: String,
    kind: String,
    image: Option<Value>,
    images: sqlx::types::Json<Vec<Value>>,
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
    Extension(section): Extension<Section>,
    State(state): State<AppState>,
    reader: Reader,
    Path((topic, slug)): Path<(String, String)>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let topics = check(&state, section, &topic, reader).await?;
    let en = params.en();
    let work: Work = sqlx::query_as("SELECT id, name, name_en, creator, kind, image, images, guide, guide_at FROM catalog_entries WHERE topic_id = $1 AND slug = $2 AND guide IS NOT NULL")
        .bind(&topic).bind(&slug).fetch_optional(&state.pool).await?.ok_or_else(missing)?;
    let name = choose(en, &work.name, &work.name_en);
    let guide = &work.guide.0;
    let intro = choose(en, &guide.intro, &guide.intro_en);
    let path = format!("/{}/{topic}/{slug}", section.path());
    let eyebrow = match section {
        Section::Culture => work.kind.clone(),
        Section::Gadgets => format!(
            "{} · {}",
            work.creator,
            gadgets::label(&topic, &work.kind, en)
        ),
    };
    let mut content = tabs(section, &topics, &topic, en, reader);
    content.push_str("<article class=catalog-work><header class=catalog-work-heading>");
    if let Some(url) = work.image.as_ref().and_then(|image| image["url"].as_str()) {
        let _ = write!(
            content,
            "<img class=catalog-cover src=\"{}\" alt=\"{}\" fetchpriority=high decoding=async referrerpolicy=no-referrer>",
            escape(url),
            escape(name)
        );
    }
    let _ = write!(
        content,
        "<div><p class=catalog-eyebrow>{}</p><h1>{}</h1><p class=catalog-original>{}</p><div class=catalog-intro>{}</div></div></header>",
        escape(&eyebrow),
        escape(name),
        escape(choose(en, &work.name_en, &work.name)),
        paragraphs(intro)
    );
    // The official pictures beyond the cover.
    let gallery: Vec<&str> = work
        .images
        .0
        .iter()
        .filter_map(|image| image["url"].as_str())
        .skip(1)
        .collect();
    if !gallery.is_empty() {
        content.push_str("<div class=catalog-gallery>");
        for url in gallery {
            let _ = write!(
                content,
                "<img src=\"{}\" alt=\"{}\" loading=lazy decoding=async referrerpolicy=no-referrer>",
                escape(url),
                escape(name)
            );
        }
        content.push_str("</div>");
    }
    if !guide.facts.is_empty() {
        let _ = write!(
            content,
            "<dl class=\"catalog-facts{}\">",
            if section == Section::Gadgets {
                " catalog-specs"
            } else {
                ""
            }
        );
        for entry in &guide.facts {
            let _ = write!(
                content,
                "<div><dt>{}</dt><dd>{} <a class=catalog-citation href=\"#source-{}\">[{}]</a></dd></div>",
                escape(choose(en, &entry.label, &entry.label_en)),
                escape(choose(en, &entry.text, &entry.text_en)),
                entry.source,
                entry.source + 1
            );
        }
        content.push_str("</dl>");
    }
    if !guide.links.is_empty() {
        content.push_str("<div class=catalog-links>");
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
    content.push_str("<div class=catalog-reading>");
    for entry in &guide.sections {
        let _ = write!(
            content,
            "<section><h2>{}</h2>{}<a class=catalog-citation href=\"#source-{}\">[{}]</a></section>",
            escape(choose(en, &entry.label, &entry.label_en)),
            paragraphs(choose(en, &entry.text, &entry.text_en)),
            entry.source,
            entry.source + 1
        );
    }
    let _ = write!(
        content,
        "<footer class=catalog-sources><p>{} · {}</p><ol>",
        match section {
            Section::Culture => choose(en, "资料导读", "About this work"),
            Section::Gadgets => choose(en, "官方资料", "Official sources"),
        },
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
    let roles: &[&str] = match section {
        Section::Culture => &["news"],
        Section::Gadgets => &ROLES,
    };
    for &role in roles {
        let (rows, _) = items(&state, section, &topic, Filter::Entry(work.id, role), None).await?;
        if rows.is_empty() {
            continue;
        }
        let heading = match (section, role) {
            (Section::Gadgets, "review") => choose(en, "真实测评", "Reviews"),
            (Section::Gadgets, "share") => choose(en, "用户分享", "Shared on X"),
            _ => choose(en, "最新动态", "Latest stories"),
        };
        let _ = write!(
            content,
            "<section class=catalog-related><h2>{heading}</h2><div class=catalog-grid>{}</div></section>",
            cards(&rows, &topic, en, reader)
        );
    }
    let _ = write!(
        content,
        "<p class=catalog-back><a href=\"{}\">← {}</a></p>",
        escape(&reader.localized(&format!("/{}/{topic}", section.path()), en)),
        choose(en, "回到栏目", "Back to collection")
    );
    let url = absolute(&state, &localized(&path, en));
    let schema = match section {
        Section::Culture => {
            json!({"@context":"https://schema.org","@type":match topic.as_str(){"book"=>"Book","game"=>"VideoGame","music"=>"CreativeWork",_=>if work.kind=="series"{"TVSeries"}else{"Movie"}},"name":name,"description":intro,"url":url,"dateModified":work.guide_at.to_rfc3339()})
        }
        Section::Gadgets => {
            json!({"@context":"https://schema.org","@type":"Product","name":name,"brand":{"@type":"Brand","name":work.creator},"description":intro,"url":url,"image":work.image.as_ref().and_then(|image| image["url"].as_str())})
        }
    };
    let html = layout(
        &state,
        reader,
        section,
        &topic,
        en,
        &path,
        &format!("{name} · {} · News", section.label()),
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
async fn sitemap(
    Extension(section): Extension<Section>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response> {
    let rows: Vec<(String,String,DateTime<Utc>)> = sqlx::query_as("SELECT e.topic_id, e.slug, e.guide_at FROM catalog_entries e JOIN topics t ON t.id = e.topic_id WHERE t.id = ANY($1) AND t.enabled AND t.visible AND t.owner_user_id IS NULL AND e.guide IS NOT NULL ORDER BY e.topic_id, e.slug LIMIT 25000")
        .bind(section.topics()).fetch_all(&state.pool).await?;
    let mut xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">".to_owned();
    for (topic, slug, at) in rows {
        for en in [false, true] {
            let _ = write!(
                xml,
                "<url><loc>{}</loc><lastmod>{}</lastmod></url>",
                escape(&absolute(
                    &state,
                    &localized(&format!("/{}/{topic}/{slug}", section.path()), en)
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
