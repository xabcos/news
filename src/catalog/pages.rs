//! Each section's pages: `/{section}/{topic}` lists a topic's items under its kind filters and
//! shelves of entries, `/{section}/{topic}/{slug}` shows an entry with its guide, its reviews,
//! posts and news, then more by the same creator and more of its kind; `more.json` pages the
//! list; `/{section}/search` finds entries and stories across the section's topics, or lists all
//! of one topic's entries.
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
/// The most talked-about shelf counts the reviews and posts of this many recent days, needs this
/// many of them for an entry, and shows only when this many entries have them.
const TALKED_DAYS: i64 = 30;
const TALKED_ITEMS: i64 = 2;
const TALKED_SHELF: usize = 3;
/// Search reads this many characters of its words and shows this many entries for them.
const QUERY_CHARS: usize = 40;
const FOUND: i64 = 60;
const CONTENT: &str = "t.enabled AND t.owner_user_id IS NULL";
/// An entry has a page once a visible item belongs to it; its guide adds facts when written.
const LISTED: &str = "EXISTS (SELECT 1 FROM catalog_items c JOIN items i ON i.id = c.item_id
     WHERE c.entry_id = e.id AND NOT i.hidden)";

/// An entry's picture: its own (official, or a work's cover), otherwise the newest picture of
/// its reviews and news, and last of readers' posts where they picture the entry itself.
fn cover(section: Section) -> String {
    format!(
        "COALESCE(e.image, (SELECT i.image FROM catalog_items c JOIN items i ON i.id = c.item_id
             WHERE c.entry_id = e.id AND NOT i.hidden AND jsonb_typeof(i.image) = 'object'{}
             ORDER BY c.role = 'share', COALESCE(i.published_at, i.first_seen_at) DESC LIMIT 1))",
        if section.posts_picture_entries() {
            ""
        } else {
            " AND c.role <> 'share'"
        }
    )
}
/// The groups of an entry's items, in page order.
const ROLES: [&str; 3] = ["review", "share", "news"];

#[derive(Deserialize)]
struct Params {
    lang: Option<String>,
    before: Option<String>,
    kind: Option<String>,
}
#[derive(Deserialize)]
struct Search {
    lang: Option<String>,
    q: Option<String>,
    topic: Option<String>,
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
                        .route(&format!("{base}/search"), get(search))
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
    Ok(Redirect::temporary(&localized(
        &format!("/{}/{topic}", section.path()),
        params.en(),
    ))
    .into_response())
}
fn tabs(section: Section, topics: &[(String, String)], current: &str, en: bool) -> String {
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
            escape(&localized(&format!("/{}/{id}", section.path()), en)),
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
            stylesheet: "/catalog.css?v=6",
            script: "/catalog.js?v=4",
            feed: (section.label(en), &format!("/feed.xml?topic={topic}")),
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
        Filter::Matching(pattern) => {
            query
                .push(" AND (items.title ILIKE ")
                .push_bind(pattern)
                .push(" OR items.title_en ILIKE ")
                .push_bind(pattern)
                .push(")");
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
    /// Titles matching an ILIKE pattern.
    Matching(&'a str),
}
fn card(item: &PublicItem, topic: &str, en: bool) -> String {
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
            escape(&localized(path, en)),
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
fn cards(rows: &[PublicItem], topic: &str, en: bool) -> String {
    rows.iter().map(|item| card(item, topic, en)).collect()
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
) -> String {
    next.map(|next| {
        let separator = if kind.is_some() { '&' } else { '?' };
        format!("<a class=\"button button-secondary catalog-more\" href=\"{}\" data-before=\"{}\" data-source=\"{}\">{}</a>",
            escape(&localized(&format!("{}{separator}before={next}", list_path(section, topic, kind)), en)),
            escape(next),
            escape(&format!("/{}/{topic}/more.json{}", section.path(), kind.map(|kind| format!("?kind={kind}")).unwrap_or_default())),
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
/// Which entries a shelf shows, those with a page, the most recently active first: by when their
/// latest story was published, the date its card shows. Collection time would tie every entry a
/// run touched and lift an entry whenever an old story is found late.
#[derive(Clone, Copy)]
enum Shelf<'a> {
    /// A topic's, of one kind or all.
    Topic(&'a str, Option<&'a str>),
    /// Likewise, but those with the most reviews and posts lately first, and only those with a few.
    Talked(&'a str, Option<&'a str>),
    /// In the given topics, ordered as the talked-about shelf; with an ILIKE pattern, those whose
    /// name, English name or creator matches, or the title of one of their stories, which finds
    /// HUAWEI Mate 90 for 华为.
    Found(&'a [String], Option<&'a str>),
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
    guide: Option<sqlx::types::Json<Guide>>,
    /// Reviews and posts of the last `TALKED_DAYS` days.
    talked: i64,
}
async fn shelved(
    state: &AppState,
    section: Section,
    shelf: Shelf<'_>,
    limit: i64,
) -> Result<Vec<Shelved>> {
    let talked = format!(
        "(SELECT COUNT(*) FROM catalog_items c JOIN items i ON i.id = c.item_id
          WHERE c.entry_id = e.id AND NOT i.hidden AND c.role IN ('review', 'share')
            AND COALESCE(i.published_at, i.first_seen_at) >= NOW() - INTERVAL '{TALKED_DAYS} days')"
    );
    let mut query = sqlx::QueryBuilder::new(format!(
        "SELECT e.id, e.topic_id, e.slug, e.name, e.name_en, e.kind, {} AS image, e.guide,
                {talked} AS talked
         FROM catalog_entries e WHERE (e.guide IS NOT NULL OR {LISTED}) AND ",
        cover(section)
    ));
    match shelf {
        Shelf::Topic(topic, kind) | Shelf::Talked(topic, kind) => {
            query.push("e.topic_id = ").push_bind(topic);
            if let Some(kind) = kind {
                query.push(" AND e.kind = ").push_bind(kind);
            }
        }
        Shelf::Found(topics, pattern) => {
            query.push("e.topic_id = ANY(").push_bind(topics).push(")");
            if let Some(pattern) = pattern {
                query
                    .push(" AND (e.name ILIKE ")
                    .push_bind(pattern)
                    .push(" OR e.name_en ILIKE ")
                    .push_bind(pattern)
                    .push(" OR e.creator ILIKE ")
                    .push_bind(pattern)
                    .push(
                        " OR EXISTS (SELECT 1 FROM catalog_items c JOIN items i ON i.id = c.item_id
                           WHERE c.entry_id = e.id AND NOT i.hidden AND (i.title ILIKE ",
                    )
                    .push_bind(pattern)
                    .push(" OR i.title_en ILIKE ")
                    .push_bind(pattern)
                    .push(")))");
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
    let recent = "(SELECT MAX(COALESCE(i.published_at, i.first_seen_at)) FROM catalog_items c
         JOIN items i ON i.id = c.item_id WHERE c.entry_id = e.id AND NOT i.hidden) DESC NULLS LAST";
    if let Shelf::Talked(..) = shelf {
        query.push(format!(
            " AND {talked} >= {TALKED_ITEMS} ORDER BY {talked} DESC, {recent}, e.guide_at DESC NULLS LAST LIMIT "
        ));
    } else if let Shelf::Found(..) = shelf {
        query.push(format!(
            " ORDER BY {talked} DESC, {recent}, e.guide_at DESC NULLS LAST LIMIT "
        ));
    } else {
        query.push(format!(
            " ORDER BY {recent}, e.guide_at DESC NULLS LAST LIMIT "
        ));
    }
    query.push_bind(limit);
    Ok(query.build_query_as().fetch_all(&state.pool).await?)
}
/// Entries as shelf cards: picture, kind, name and first facts.
fn shelf_cards(section: Section, entries: &[Shelved], talked: bool, en: bool) -> String {
    let mut html = String::from("<div class=catalog-products>");
    for entry in entries {
        let buzz = if talked && entry.talked > 0 {
            let count = entry.talked;
            format!(
                "<span class=catalog-product-buzz>{}</span>",
                match (section, en) {
                    (_, true) => format!("{count} reviews and posts"),
                    (Section::Culture, false) => format!("{count} 条评论与分享"),
                    (Section::Gadgets, false) => format!("{count} 条评测与分享"),
                }
            )
        } else {
            String::new()
        };
        let facts = entry
            .guide
            .iter()
            .flat_map(|guide| &guide.0.facts)
            .take(3)
            .map(|fact| escape(choose(en, &fact.text, &fact.text_en)))
            .collect::<Vec<_>>()
            .join(" · ");
        let _ = write!(
            html,
            "<a class=catalog-product href=\"{}\">{}<span class=catalog-product-kind>{}</span><strong>{}</strong>{buzz}<span class=catalog-product-specs>{facts}</span></a>",
            escape(&localized(
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
/// A topic's kind filters, when it has more than one kind, then its shelves: the most talked-about
/// entries, when there are a few, and the recently active ones.
async fn shelf(
    state: &AppState,
    section: Section,
    topic: &str,
    kind: Option<&str>,
    en: bool,
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
                escape(&localized(&list_path(section, topic, id), en)),
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
    let talked = shelved(state, section, Shelf::Talked(topic, kind), SHELF).await?;
    if talked.len() >= TALKED_SHELF {
        let _ = write!(
            html,
            "<section class=catalog-shelf><h2>{}</h2>{}</section>",
            choose(en, "最受关注", "Most talked about"),
            shelf_cards(section, &talked, true, en)
        );
    }
    let entries = shelved(state, section, Shelf::Topic(topic, kind), SHELF).await?;
    if entries.is_empty() {
        return Ok(html);
    }
    let _ = write!(
        html,
        "<section class=catalog-shelf><h2>{} <a class=catalog-all href=\"{}\">{} →</a></h2>{}</section><h2 class=catalog-latest>{}</h2>",
        choose(en, "近期有动态", "Recently active"),
        escape(&localized(
            &format!("/{}/search?topic={topic}", section.path()),
            en
        )),
        match section {
            Section::Culture => choose(en, "全部作品", "All works"),
            Section::Gadgets => choose(en, "全部产品", "All products"),
        },
        shelf_cards(section, &entries, false, en),
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
) -> Result<String> {
    let visible = topics.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>();
    let creator = work.creator.trim();
    let by_creator = if creator.is_empty() {
        Vec::new()
    } else {
        shelved(
            state,
            section,
            Shelf::Creator(&visible, creator, work.id),
            ONWARD,
        )
        .await?
    };
    let shown = by_creator
        .iter()
        .map(|entry| entry.id)
        .chain([work.id])
        .collect::<Vec<_>>();
    let alike = shelved(
        state,
        section,
        Shelf::Kind(topic, &work.kind, &shown),
        ONWARD,
    )
    .await?;
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
            shelf_cards(section, &by_creator, false, en)
        );
    }
    if !alike.is_empty() {
        let label = section.kind_label(topic, &work.kind, en);
        let _ = write!(
            html,
            "<section class=catalog-onward><h2>{}{label}</h2>{}</section>",
            choose(en, "更多", "More "),
            shelf_cards(section, &alike, false, en)
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
    let mut content = tabs(section, &topics, &topic, en);
    content.push_str(&search_form(section, "", en));
    content.push_str(&shelf(&state, section, &topic, kind, en).await?);
    let _ = write!(
        content,
        "<div class=catalog-grid>{}</div>",
        cards(&rows, &topic, en)
    );
    if rows.is_empty() {
        let _ = write!(
            content,
            "<p class=empty>{}</p>",
            choose(en, "新故事正在路上。", "New stories are on their way.")
        );
    }
    content.push_str(&pager(section, &topic, kind, next.as_deref(), en));
    let path = list_path(section, &topic, kind);
    let html = layout(
        &state,
        reader,
        section,
        &topic,
        en,
        &path,
        &format!("{name} · {} · News", section.label(en)),
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
/// The search box of a section's pages; its words search every topic of the section.
fn search_form(section: Section, words: &str, en: bool) -> String {
    let hint = match section {
        Section::Culture => choose(
            en,
            "搜索作品、创作者或内容",
            "Search works, creators or stories",
        ),
        Section::Gadgets => choose(
            en,
            "搜索产品、品牌或内容",
            "Search products, brands or stories",
        ),
    };
    format!(
        "<form class=catalog-search action=\"{}\" role=search><input type=search name=q value=\"{}\" maxlength={QUERY_CHARS} placeholder=\"{hint}\" aria-label=\"{hint}\" enterkeyhint=search>{}</form>",
        escape(&format!("/{}/search", section.path())),
        escape(words),
        if en {
            "<input type=hidden name=lang value=en>"
        } else {
            ""
        }
    )
}
/// Entries whose name, English name, creator or a story's title holds the words, and stories
/// whose title does, across the section's topics; or, with a topic and no words, all of that topic's entries. The
/// most talked-about entries come first, and the newest stories. Pages for words stay out of
/// search engines; a topic's full list is meant to be found.
async fn search(
    Extension(section): Extension<Section>,
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Search>,
    headers: HeaderMap,
) -> Result<Response> {
    let topics = catalogue(&state, section, reader);
    let en = params.lang.as_deref() == Some("en");
    let words = params
        .q
        .as_deref()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(QUERY_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned();
    // A section or topic readers can't see is missing here too, so an administrator is sent to
    // its preview.
    if topics.is_empty() {
        return Err(missing());
    }
    let listed = params
        .topic
        .as_deref()
        .map(|id| {
            topics
                .iter()
                .find(|(topic, _)| topic == id)
                .ok_or_else(missing)
        })
        .transpose()?;
    if words.is_empty() && listed.is_none() {
        return Ok(
            Redirect::temporary(&localized(&format!("/{}", section.path()), en)).into_response(),
        );
    }
    let ids = listed.map_or_else(
        || topics.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
        |(id, _)| vec![id.clone()],
    );
    // ILIKE's own wildcards in the words are matched literally.
    let pattern = (!words.is_empty()).then(|| {
        format!(
            "%{}%",
            words
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        )
    });
    let entries = shelved(
        &state,
        section,
        Shelf::Found(&ids, pattern.as_deref()),
        if pattern.is_some() { FOUND } else { i64::MAX },
    )
    .await?;
    let mut stories = Vec::new();
    if let Some(pattern) = &pattern {
        for id in &ids {
            let (rows, _) = items(&state, section, id, Filter::Matching(pattern), None).await?;
            stories.extend(rows.into_iter().map(|item| (id.as_str(), item)));
        }
        stories.sort_by_key(|(_, item)| std::cmp::Reverse((item.first_seen_at, item.id)));
        stories.truncate(LIMIT);
    }
    let noun = match section {
        Section::Culture => choose(en, "作品", "Works"),
        Section::Gadgets => choose(en, "产品", "Products"),
    };
    let current = listed.map_or_else(|| section.topics()[0], |(id, _)| id.as_str());
    let mut content = tabs(
        section,
        &topics,
        listed.map_or("", |(id, _)| id.as_str()),
        en,
    );
    content.push_str(&search_form(section, &words, en));
    let title = match listed {
        Some((_, name)) if words.is_empty() => format!(
            "{name} · {}",
            match section {
                Section::Culture => choose(en, "全部作品", "All works"),
                Section::Gadgets => choose(en, "全部产品", "All products"),
            }
        ),
        _ => format!("{} · {words}", choose(en, "搜索", "Search")),
    };
    if !entries.is_empty() {
        let _ = write!(
            content,
            "<section class=\"catalog-shelf catalog-found\"><h2>{noun} <span>{}</span></h2>{}</section>",
            entries.len(),
            shelf_cards(section, &entries, true, en)
        );
    }
    if !stories.is_empty() {
        let _ = write!(
            content,
            "<h2 class=catalog-latest>{}</h2><div class=catalog-grid>{}</div>",
            choose(en, "相关内容", "Stories"),
            stories
                .iter()
                .map(|(topic, item)| card(item, topic, en))
                .collect::<String>()
        );
    }
    if entries.is_empty() && stories.is_empty() {
        let _ = write!(
            content,
            "<p class=empty>{}</p>",
            choose(
                en,
                "没有找到相关的内容，换个词试试。",
                "Nothing found; try other words."
            )
        );
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if !words.is_empty() {
        query.append_pair("q", &words);
    }
    if let Some((id, _)) = listed {
        query.append_pair("topic", id);
    }
    let path = format!("/{}/search?{}", section.path(), query.finish());
    let html = layout(
        &state,
        reader,
        section,
        current,
        en,
        &path,
        &format!("{title} · {} · News", section.label(en)),
        &title,
        &content,
        None,
    );
    let mut response = conditional(&headers, &Rendered::new(html), "text/html; charset=utf-8");
    if !words.is_empty() {
        response.headers_mut().insert(
            axum::http::header::HeaderName::from_static("x-robots-tag"),
            axum::http::HeaderValue::from_static("noindex"),
        );
    }
    Ok(response)
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
        &Rendered::new(json!({"html":cards(&rows, &topic, params.en()),"next":next}).to_string()),
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
    guide: Option<sqlx::types::Json<Guide>>,
    guide_at: Option<DateTime<Utc>>,
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
    let work: Work = sqlx::QueryBuilder::new(format!(
        "SELECT e.id, e.name, e.name_en, e.creator, e.kind, {} AS image, e.images, e.guide, e.guide_at
         FROM catalog_entries e WHERE (e.guide IS NOT NULL OR {LISTED}) AND e.topic_id = ",
        cover(section)
    ))
    .push_bind(&topic)
    .push(" AND e.slug = ")
    .push_bind(&slug)
    .build_query_as()
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(missing)?;
    let name = choose(en, &work.name, &work.name_en);
    let guide = work.guide.as_ref().map(|guide| &guide.0);
    let intro = guide.map_or("", |guide| choose(en, &guide.intro, &guide.intro_en));
    let path = format!("/{}/{topic}/{slug}", section.path());
    let label = section.kind_label(&topic, &work.kind, en);
    let eyebrow = match section {
        Section::Culture => label.to_owned(),
        Section::Gadgets => format!("{} · {label}", work.creator),
    };
    let mut content = tabs(section, &topics, &topic, en);
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
    if let (Some(guide), Some(at)) = (guide, work.guide_at) {
        content.push_str(&guide_html(section, guide, at, en));
    }
    content.push_str("</article>");
    for role in ROLES {
        let (rows, _) = items(&state, section, &topic, Filter::Entry(work.id, role), None).await?;
        if rows.is_empty() {
            continue;
        }
        let heading = match (section, role) {
            (Section::Culture, "review") => choose(en, "评论", "Reviews"),
            (Section::Culture, "share") => choose(en, "大家说", "On X"),
            (Section::Gadgets, "review") => choose(en, "真实测评", "Reviews"),
            (Section::Gadgets, "share") => choose(en, "用户分享", "Shared by owners"),
            _ => choose(en, "最新动态", "Latest stories"),
        };
        let _ = write!(
            content,
            "<section class=catalog-related><h2>{heading}</h2><div class=catalog-grid>{}</div></section>",
            cards(&rows, &topic, en)
        );
    }
    content.push_str(&onward(&state, section, &topics, &topic, &work, en).await?);
    let _ = write!(
        content,
        "<p class=catalog-back><a href=\"{}\">← {}</a></p>",
        escape(&localized(&format!("/{}/{topic}", section.path()), en)),
        choose(en, "回到栏目", "Back to collection")
    );
    let url = absolute(&state, &localized(&path, en));
    // Before its guide, an entry is described by what the page gathers.
    let description = if intro.is_empty() {
        if en {
            format!("{name}: reviews, owners' posts and news")
        } else {
            format!("{name}：评测、用户分享与最新动态")
        }
    } else {
        intro.to_owned()
    };
    let schema = match section {
        Section::Culture => {
            json!({"@context":"https://schema.org","@type":match topic.as_str(){"book"=>"Book","game"=>"VideoGame","music"=>"CreativeWork",_=>if work.kind=="series"{"TVSeries"}else{"Movie"}},"name":name,"description":description,"url":url,"dateModified":work.guide_at.map(|at| at.to_rfc3339())})
        }
        Section::Gadgets => {
            json!({"@context":"https://schema.org","@type":"Product","name":name,"brand":{"@type":"Brand","name":work.creator},"description":description,"url":url,"image":work.image.as_ref().and_then(|image| image["url"].as_str())})
        }
    };
    let html = layout(
        &state,
        reader,
        section,
        &topic,
        en,
        &path,
        &format!("{name} · {} · News", section.label(en)),
        &description,
        &content,
        Some(schema),
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}
/// The guide below an entry's heading: its facts, links and sections, then the sources they cite.
fn guide_html(section: Section, guide: &Guide, at: DateTime<Utc>, en: bool) -> String {
    let mut html = String::new();
    if !guide.facts.is_empty() {
        let _ = write!(
            html,
            "<dl class=\"catalog-facts{}\">",
            if section == Section::Gadgets {
                " catalog-specs"
            } else {
                ""
            }
        );
        for entry in &guide.facts {
            let _ = write!(
                html,
                "<div><dt>{}</dt><dd>{} <a class=catalog-citation href=\"#source-{}\">[{}]</a></dd></div>",
                escape(choose(en, &entry.label, &entry.label_en)),
                escape(choose(en, &entry.text, &entry.text_en)),
                entry.source,
                entry.source + 1
            );
        }
        html.push_str("</dl>");
    }
    if !guide.links.is_empty() {
        html.push_str("<div class=catalog-links>");
        for link in &guide.links {
            let _ = write!(
                html,
                "<a class=\"button button-secondary\" href=\"{}\" target=_blank rel=\"noopener noreferrer\">{} ↗</a>",
                escape(&link.url),
                escape(choose(en, &link.label, &link.label_en))
            );
        }
        html.push_str("</div>");
    }
    html.push_str("<div class=catalog-reading>");
    for entry in &guide.sections {
        let _ = write!(
            html,
            "<section><h2>{}</h2>{}<a class=catalog-citation href=\"#source-{}\">[{}]</a></section>",
            escape(choose(en, &entry.label, &entry.label_en)),
            paragraphs(choose(en, &entry.text, &entry.text_en)),
            entry.source,
            entry.source + 1
        );
    }
    let _ = write!(
        html,
        "<footer class=catalog-sources><p>{} · {}</p><ol>",
        match section {
            Section::Culture => choose(en, "资料导读", "About this work"),
            Section::Gadgets => choose(en, "官方资料", "Official sources"),
        },
        at.format("%Y-%m-%d")
    );
    let mut wiki = false;
    for (i, source) in guide.sources.iter().enumerate() {
        let _ = write!(
            html,
            "<li id=source-{i}><a href=\"{}\" target=_blank rel=\"noopener noreferrer\">{}</a></li>",
            escape(&source.url),
            escape(&source.title)
        );
        wiki |= url::Url::parse(&source.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .is_some_and(|h| h.ends_with(".wikipedia.org"));
    }
    html.push_str("</ol>");
    if wiki {
        html.push_str("<p>Wikipedia · <a href=\"https://creativecommons.org/licenses/by-sa/4.0/\" rel=license>CC BY-SA 4.0</a></p>");
    }
    html.push_str("</footer></div>");
    html
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
