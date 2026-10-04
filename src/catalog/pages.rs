//! Each section's pages: `/{section}/{topic}` lists a topic's items under its kind filters and a
//! shelf of recently active entries, `/{section}/{topic}/{slug}` shows an entry with its guide,
//! its reviews, posts and news, then more by the same creator and more of its kind; `more.json`
//! pages the list.
use super::{Guide, ITEM_JOIN, Section, culture, gadgets, item_json};
use crate::{
    digest::{PublicItem, Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{After, Chrome, absolute, choose, escape, localized, next_page, parse_after, push_page},
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
/// Entries on a topic's shelf, and in each row of an entry's onward links.
const SHELF: i64 = 12;
const ONWARD: i64 = 6;
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

/// The section's topics the reader may see, as (id, name) in the section's order.
fn catalogue(state: &AppState, section: Section, reader: Reader) -> Vec<(String, String)> {
    let topics = state.digest.topics_for(reader);
    section
        .topics()
        .iter()
        .filter_map(|id| topics.iter().find(|topic| topic.id == *id))
        .map(|topic| (topic.id.clone(), topic.name.clone()))
        .collect()
}
/// The sections with a public topic, for the sitemap index.
pub(crate) fn listed(state: &AppState) -> Vec<Section> {
    Section::ALL
        .into_iter()
        .filter(|section| !catalogue(state, *section, Reader::Public).is_empty())
        .collect()
}
fn missing() -> AppError {
    AppError::NotFound("栏目或条目不存在".into())
}
fn check(
    state: &AppState,
    section: Section,
    topic: &str,
    reader: Reader,
) -> Result<Vec<(String, String)>> {
    let topics = catalogue(state, section, reader);
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
    let topics = catalogue(&state, section, reader);
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
fn layout(
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
) -> String {
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
            stylesheet: "/catalog.css?v=2",
            script: "/catalog.js?v=3",
            feed: (section.label(), &format!("/feed.xml?topic={topic}")),
        },
    )
}
/// One page of a topic's items, newest first: all of them, those of one kind of entry, or
/// those of one entry in one role.
async fn items(
    state: &AppState,
    section: Section,
    topic: &str,
    filter: Filter<'_>,
    before: Option<After>,
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
    push_page(&mut query, before, LIMIT);
    let mut rows = query
        .build_query_as::<PublicItem>()
        .fetch_all(&state.pool)
        .await?;
    let next = next_page(&mut rows, LIMIT, |item| (item.first_seen_at, item.id));
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
    let section = Section::of(topic).unwrap_or(Section::Culture);
    let role = entry
        .and_then(|entry| entry["role"].as_str())
        .map_or("", |role| role_label(section, role, en));
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
/// A role's badge on a card.
fn role_label(section: Section, role: &str, en: bool) -> &'static str {
    match (section, role) {
        (Section::Culture, "review") => choose(en, "评论", "Review"),
        (Section::Culture, "share") => choose(en, "大家说", "On X"),
        (Section::Gadgets, "review") => choose(en, "评测", "Review"),
        (Section::Gadgets, "share") => choose(en, "分享", "Shared"),
        _ => "",
    }
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
        .filter(|kind| section.kinds(topic).contains(kind))
}
/// Which entries a shelf shows, all with a guide, the most recently active first: by when their
/// latest story was published, the date its card shows. Collection time would tie every entry a
/// run touched and lift an entry whenever an old story is found late.
#[derive(Clone, Copy)]
enum Shelf<'a> {
    /// A topic's, of one kind or all.
    Topic(&'a str, Option<&'a str>),
    /// By one creator in the given topics, but one entry.
    Creator(&'a [String], &'a str, Uuid),
    /// Of one kind in a topic, but the given entries.
    Kind(&'a str, &'a str, &'a [Uuid]),
}
#[derive(sqlx::FromRow)]
struct Shelved {
    id: Uuid,
    topic_id: String,
    slug: String,
    name: String,
    name_en: String,
    kind: String,
    image: Option<Value>,
    guide: sqlx::types::Json<Guide>,
}
async fn shelved(state: &AppState, shelf: Shelf<'_>, limit: i64) -> Result<Vec<Shelved>> {
    let mut query = sqlx::QueryBuilder::new(
        "SELECT e.id, e.topic_id, e.slug, e.name, e.name_en, e.kind, e.image, e.guide
         FROM catalog_entries e WHERE e.guide IS NOT NULL AND ",
    );
    match shelf {
        Shelf::Topic(topic, kind) => {
            query.push("e.topic_id = ").push_bind(topic);
            if let Some(kind) = kind {
                query.push(" AND e.kind = ").push_bind(kind);
            }
        }
        Shelf::Creator(topics, creator, except) => {
            query
                .push("e.topic_id = ANY(")
                .push_bind(topics)
                .push(") AND lower(e.creator) = lower(")
                .push_bind(creator)
                .push(") AND e.id <> ")
                .push_bind(except);
        }
        Shelf::Kind(topic, kind, except) => {
            query
                .push("e.topic_id = ")
                .push_bind(topic)
                .push(" AND e.kind = ")
                .push_bind(kind)
                .push(" AND e.id <> ALL(")
                .push_bind(except)
                .push(")");
        }
    }
    query
        .push(
            " ORDER BY (SELECT MAX(COALESCE(i.published_at, i.first_seen_at)) FROM catalog_items c JOIN items i ON i.id = c.item_id
                 WHERE c.entry_id = e.id AND NOT i.hidden) DESC NULLS LAST, e.guide_at DESC LIMIT ",
        )
        .push_bind(limit);
    Ok(query.build_query_as().fetch_all(&state.pool).await?)
}
/// Entries as shelf cards: picture, kind, name and first facts.
fn shelf_cards(section: Section, entries: &[Shelved], en: bool, reader: Reader) -> String {
    let mut html = String::from("<div class=catalog-products>");
    for entry in entries {
        let facts = entry
            .guide
            .0
            .facts
            .iter()
            .take(3)
            .map(|fact| escape(choose(en, &fact.text, &fact.text_en)))
            .collect::<Vec<_>>()
            .join(" · ");
        let _ = write!(
            html,
            "<a class=catalog-product href=\"{}\">{}<span class=catalog-product-kind>{}</span><strong>{}</strong><span class=catalog-product-specs>{facts}</span></a>",
            escape(&reader.localized(
                &format!("/{}/{}/{}", section.path(), entry.topic_id, entry.slug),
                en
            )),
            entry
                .image
                .as_ref()
                .and_then(|image| image["url"].as_str())
                .map(|url| format!("<img src=\"{}\" alt=\"\" loading=lazy decoding=async referrerpolicy=no-referrer>", escape(url)))
                .unwrap_or_default(),
            section.kind_label(&entry.topic_id, &entry.kind, en),
            escape(choose(en, &entry.name, &entry.name_en)),
        );
    }
    html.push_str("</div>");
    html
}
/// A topic's kind filters, when it has more than one kind, and its shelf of recently active
/// entries.
async fn shelf(
    state: &AppState,
    section: Section,
    topic: &str,
    kind: Option<&str>,
    en: bool,
    reader: Reader,
) -> Result<String> {
    let mut html = String::new();
    let labels = section.labels(topic, en);
    if labels.len() > 1 {
        html.push_str("<nav class=catalog-kinds>");
        let filters = std::iter::once((None, choose(en, "全部", "All")))
            .chain(labels.into_iter().map(|(id, label)| (Some(id), label)));
        for (id, label) in filters {
            let _ = write!(
                html,
                "<a href=\"{}\"{}>{}</a>",
                escape(&reader.localized(&list_path(section, topic, id), en)),
                if id == kind {
                    " aria-current=\"true\""
                } else {
                    ""
                },
                escape(label)
            );
        }
        html.push_str("</nav>");
    }
    let entries = shelved(state, Shelf::Topic(topic, kind), SHELF).await?;
    if entries.is_empty() {
        return Ok(html);
    }
    let _ = write!(
        html,
        "<section class=catalog-shelf><h2>{}</h2>{}</section><h2 class=catalog-latest>{}</h2>",
        match section {
            Section::Culture => choose(en, "作品", "Works"),
            Section::Gadgets => choose(en, "产品", "Products"),
        },
        shelf_cards(section, &entries, en, reader),
        choose(en, "最新内容", "Latest")
    );
    Ok(html)
}
/// Where to go from an entry: more by its creator in the section, then more of its kind in
/// its topic.
async fn onward(
    state: &AppState,
    section: Section,
    topics: &[(String, String)],
    topic: &str,
    work: &Work,
    en: bool,
    reader: Reader,
) -> Result<String> {
    let visible = topics.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
    let creator = work.creator.trim();
    let by_creator = if creator.is_empty() {
        Vec::new()
    } else {
        shelved(state, Shelf::Creator(&visible, creator, work.id), ONWARD).await?
    };
    let shown = by_creator
        .iter()
        .map(|entry| entry.id)
        .chain([work.id])
        .collect::<Vec<_>>();
    let alike = shelved(state, Shelf::Kind(topic, &work.kind, &shown), ONWARD).await?;
    let mut html = String::new();
    if !by_creator.is_empty() {
        let heading = if en {
            format!("More from {creator}")
        } else {
            format!(
                "{creator} 的更多{}",
                match section {
                    Section::Culture => "作品",
                    Section::Gadgets => "产品",
                }
            )
        };
        let _ = write!(
            html,
            "<section class=catalog-onward><h2>{}</h2>{}</section>",
            escape(&heading),
            shelf_cards(section, &by_creator, en, reader)
        );
    }
    if !alike.is_empty() {
        let label = section.kind_label(topic, &work.kind, en);
        let _ = write!(
            html,
            "<section class=catalog-onward><h2>{}{label}</h2>{}</section>",
            choose(en, "更多", "More "),
            shelf_cards(section, &alike, en, reader)
        );
    }
    Ok(html)
}
async fn collection(
    Extension(section): Extension<Section>,
    State(state): State<AppState>,
    reader: Reader,
    Path(topic): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let topics = check(&state, section, &topic, reader)?;
    let en = params.en();
    let kind = kind(section, &topic, &params);
    let filter = kind.map_or(Filter::All, Filter::Kind);
    let (rows, next) = items(
        &state,
        section,
        &topic,
        filter,
        params.before.as_deref().map(parse_after).transpose()?,
    )
    .await?;
    let name = topics
        .iter()
        .find(|(id, _)| id == &topic)
        .map(|(_, name)| name.as_str())
        .ok_or_else(missing)?;
    let mut content = tabs(section, &topics, &topic, en, reader);
    content.push_str(&shelf(&state, section, &topic, kind, en, reader).await?);
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
    );
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
    check(&state, section, &topic, reader)?;
    let filter = kind(section, &topic, &params).map_or(Filter::All, Filter::Kind);
    let (rows, next) = items(
        &state,
        section,
        &topic,
        filter,
        params.before.as_deref().map(parse_after).transpose()?,
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
    let topics = check(&state, section, &topic, reader)?;
    let en = params.en();
    let work: Work = sqlx::query_as("SELECT id, name, name_en, creator, kind, image, images, guide, guide_at FROM catalog_entries WHERE topic_id = $1 AND slug = $2 AND guide IS NOT NULL")
        .bind(&topic).bind(&slug).fetch_optional(&state.pool).await?.ok_or_else(missing)?;
    let name = choose(en, &work.name, &work.name_en);
    let guide = &work.guide.0;
    let intro = choose(en, &guide.intro, &guide.intro_en);
    let path = format!("/{}/{topic}/{slug}", section.path());
    let label = section.kind_label(&topic, &work.kind, en);
    let eyebrow = match section {
        Section::Culture => label.to_owned(),
        Section::Gadgets => format!("{} · {label}", work.creator),
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
    for role in ROLES {
        let (rows, _) = items(&state, section, &topic, Filter::Entry(work.id, role), None).await?;
        if rows.is_empty() {
            continue;
        }
        let heading = match (section, role) {
            (Section::Culture, "review") => choose(en, "评论", "Reviews"),
            (Section::Culture, "share") => choose(en, "大家说", "On X"),
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
    content.push_str(&onward(&state, section, &topics, &topic, &work, en, reader).await?);
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
    );
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
