//! A catalogue of works and products in two sections, Culture and 𝑮𝒂𝒅𝒈𝒆𝒕𝒔. Their topics share
//! collection and news storage; items attach to entries, and each entry gets a guide written
//! only from the sources the program read. Headlines publish first; guides are independent,
//! bounded work at the end of each run.
mod culture;
mod gadgets;
mod images;
mod pages;
mod source;
#[cfg(test)]
mod tests;

use crate::{
    error::{AppError, Result},
    fetch::{NewsImage, Usage},
    media,
    state::AppState,
    topics::Topic,
};
pub(crate) use images::prepare as prepare_images;
pub(crate) use pages::{listed, router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use source::{Source, excerpt, fold, public_url};
use std::{collections::HashMap, time::Duration};
use tokio::{
    task::JoinSet,
    time::{Instant, timeout_at},
};
use uuid::Uuid;

/// A navigation entry over its topics, each with its own catalogue pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Section {
    Culture,
    Gadgets,
}

impl Section {
    pub(crate) const ALL: [Self; 2] = [Self::Culture, Self::Gadgets];

    /// The path of its pages and the class of their body.
    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Culture => "culture",
            Self::Gadgets => "gadgets",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Culture => "𝑪𝒖𝒍𝒕𝒖𝒓𝒆",
            Self::Gadgets => "𝑮𝒂𝒅𝒈𝒆𝒕𝒔",
        }
    }

    /// Its topics in the order of its tabs.
    pub(crate) fn topics(self) -> &'static [&'static str] {
        match self {
            Self::Culture => &culture::TOPICS,
            Self::Gadgets => &gadgets::TOPICS,
        }
    }

    pub(crate) fn of(topic: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|section| section.topics().contains(&topic))
    }

    fn kinds(self, topic: &str) -> Vec<&'static str> {
        match self {
            Self::Culture => culture::kinds(topic).to_vec(),
            Self::Gadgets => gadgets::kinds(topic),
        }
    }

    /// Characters kept of each source: a product's full specifications run long.
    fn source_limit(self) -> usize {
        match self {
            Self::Culture => 18_000,
            Self::Gadgets => 40_000,
        }
    }

    fn facts_limit(self) -> usize {
        match self {
            Self::Culture => 12,
            Self::Gadgets => 24,
        }
    }
}

/// The section of a public catalogue topic.
pub(crate) fn section(topic: &Topic) -> Option<Section> {
    topic
        .owner_user_id
        .is_none()
        .then(|| Section::of(&topic.id))
        .flatten()
}

pub(crate) const ITEM_JOIN: &str = "LEFT JOIN catalog_items ci ON ci.item_id = items.id LEFT JOIN catalog_entries ce ON ce.id = ci.entry_id";

/// What a public item tells of its entry: its role, and once the guide is written, the
/// entry's name, kind and page. Drafts stay private.
pub(crate) fn item_json(section: Section) -> String {
    format!(
        "CASE WHEN ci.item_id IS NOT NULL THEN jsonb_build_object('role', ci.role) || CASE WHEN ce.guide IS NOT NULL THEN jsonb_build_object('name', ce.name, 'name_en', ce.name_en, 'kind', ce.kind, 'path', '/{}/' || ce.topic_id || '/' || ce.slug) ELSE '{{}}' END END",
        section.path()
    )
}

pub(crate) fn instructions(topic: &Topic, section: Section) -> String {
    match section {
        Section::Culture => culture::instructions(topic),
        Section::Gadgets => gadgets::instructions(topic),
    }
}

pub(crate) fn schema(mut base: Value, topic: &Topic, section: Section) -> Value {
    let item = &mut base["properties"]["items"]["items"];
    let required = item["required"].as_array_mut().expect("item schema");
    required.extend([json!("work"), json!("image"), json!("image_page_url")]);
    if section == Section::Gadgets {
        required.push(json!("role"));
        item["properties"]["role"] = json!({"type":"string", "enum":["review","share","news"]});
    }
    item["properties"]["image_page_url"] = json!({"type":["string","null"]});
    item["properties"]["published_at"] = json!({"type":"string"});
    item["properties"]["work"] = json!({
        "type": ["object", "null"], "additionalProperties": false,
        "required": ["name", "name_en", "creator", "kind", "source_urls"],
        "properties": {
            "name": {"type":"string"}, "name_en": {"type":"string"},
            "creator": {"type":"string"},
            "kind": {"type":"string", "enum": section.kinds(&topic.id)},
            "source_urls": {"type":"array", "items":{"type":"string"}, "maxItems":3}
        }
    });
    base
}

#[derive(Clone, Debug, Deserialize)]
struct Proposal {
    name: String,
    name_en: String,
    creator: String,
    kind: String,
    source_urls: Vec<String>,
}

impl Proposal {
    /// The entry the topic accepts: named, of one of its kinds, with the sources its section
    /// allows. A Culture work needs its stable page; a product keeps only official pages.
    fn clean(mut self, topic: &str, brief: &str) -> Option<Self> {
        let section = Section::of(topic)?;
        self.name = self.name.trim().chars().take(200).collect();
        self.name_en = self.name_en.trim().chars().take(200).collect();
        self.creator = self.creator.trim().chars().take(200).collect();
        self.source_urls = self
            .source_urls
            .iter()
            .filter_map(|url| public_url(url))
            .take(3)
            .collect();
        self.source_urls.dedup();
        if self.name.is_empty()
            || self.name_en.is_empty()
            || !section.kinds(topic).contains(&self.kind.as_str())
        {
            return None;
        }
        match section {
            Section::Culture => {
                let primary = url::Url::parse(self.source_urls.first()?).ok()?;
                (!primary.path().trim_matches('/').is_empty()
                    && crate::reads::x_status(&primary).is_none())
                .then_some(self)
            }
            Section::Gadgets => {
                if self.creator.is_empty() {
                    return None;
                }
                self.source_urls.retain(|url| gadgets::official(brief, url));
                Some(self)
            }
        }
    }

    /// Culture knows a work by its stable page, 𝑮𝒂𝒅𝒈𝒆𝒕𝒔 a product by brand and model.
    fn key(&self, section: Section) -> String {
        match section {
            Section::Culture => self.source_urls[0].clone(),
            Section::Gadgets => {
                let brand = fold(&self.creator);
                // Galaxy S26+ is not Galaxy S26.
                let name = fold(&self.name_en.replace('+', "plus"));
                format!("{brand}:{}", name.strip_prefix(&brand).unwrap_or(&name))
            }
        }
    }
}

fn slug(name: &str, id: Uuid) -> String {
    let stem = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let stem = stem.chars().take(65).collect::<String>();
    format!(
        "{}-{}",
        if stem.is_empty() {
            "work"
        } else {
            stem.trim_end_matches('-')
        },
        &id.simple().to_string()[..8]
    )
}

/// An item's address as stored, which may be plain HTTP.
fn canonical(value: &str) -> Option<String> {
    url::Url::parse(value.trim())
        .ok()
        .map(|url| crate::fetch::canonical_url(&url))
}

/// Attach only items actually published by this run, never rejected candidates. An entry
/// seen again gains the sources it was missing, up to four.
pub(crate) async fn attach(
    state: &AppState,
    topic: &Topic,
    section: Section,
    run: Uuid,
    payload: &Value,
) -> Result<()> {
    let proposals: HashMap<_, _> = payload["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let key = canonical(item["url"].as_str()?)?;
            let work = serde_json::from_value::<Proposal>(item["work"].clone())
                .ok()?
                .clean(&topic.id, &topic.brief)?;
            let role = item["role"]
                .as_str()
                .filter(|role| matches!(*role, "review" | "share"))
                .unwrap_or("news");
            Some((key, (work, role)))
        })
        .collect();
    let published: Vec<(Uuid, String, Option<Value>)> = sqlx::query_as(
        "SELECT id, url, image FROM items WHERE first_run_id = $1 AND topic_id = $2",
    )
    .bind(run)
    .bind(&topic.id)
    .fetch_all(&state.pool)
    .await?;
    let mut tx = state.pool.begin().await?;
    for (item, url, image) in published {
        let Some((proposal, role)) = canonical(&url).and_then(|url| proposals.get(&url)) else {
            continue;
        };
        let id = Uuid::new_v4();
        // A product's picture comes from its official pages, not from a reader's photograph.
        let cover = image.filter(|_| section == Section::Culture);
        let entry: Uuid = sqlx::query_scalar("INSERT INTO catalog_entries (id, topic_id, key, slug, name, name_en, creator, kind, source_urls, image)
            VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            ON CONFLICT (topic_id, key) DO UPDATE SET image = COALESCE(catalog_entries.image, EXCLUDED.image),
                attempts = CASE WHEN catalog_entries.source_urls @> EXCLUDED.source_urls
                    OR jsonb_array_length(catalog_entries.source_urls) >= 4 THEN catalog_entries.attempts ELSE 0 END,
                source_urls = (SELECT COALESCE(jsonb_agg(url ORDER BY first), '[]') FROM (
                    SELECT url, MIN(n) AS first FROM jsonb_array_elements_text(catalog_entries.source_urls || EXCLUDED.source_urls)
                        WITH ORDINALITY AS sources(url, n) GROUP BY url ORDER BY first LIMIT 4) merged)
            RETURNING id")
            .bind(id).bind(&topic.id).bind(proposal.key(section)).bind(slug(&proposal.name_en, id))
            .bind(&proposal.name).bind(&proposal.name_en).bind(&proposal.creator).bind(&proposal.kind)
            .bind(json!(proposal.source_urls)).bind(cover).fetch_one(&mut *tx).await?;
        sqlx::query(
            "INSERT INTO catalog_items (item_id, entry_id, role) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING",
        )
        .bind(item)
        .bind(entry)
        .bind(role)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    label: String,
    label_en: String,
    text: String,
    text_en: String,
    source: usize,
    quote: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Link {
    label: String,
    label_en: String,
    url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Reference {
    title: String,
    url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Guide {
    intro: String,
    intro_en: String,
    facts: Vec<Entry>,
    sections: Vec<Entry>,
    links: Vec<Link>,
    #[serde(default)]
    sources: Vec<Reference>,
}

impl Guide {
    fn verify(&mut self, sources: &[Source], facts: usize) -> Result<()> {
        if self.intro.trim().is_empty() || self.intro_en.trim().is_empty() {
            return Err(AppError::Internal("导读缺少简介".into()));
        }
        let cited = |entry: &Entry| {
            let quote = excerpt(&entry.quote);
            !entry.label.trim().is_empty()
                && !entry.label_en.trim().is_empty()
                && !entry.text.trim().is_empty()
                && !entry.text_en.trim().is_empty()
                && quote.chars().count() >= 12
                && sources
                    .get(entry.source)
                    .is_some_and(|source| excerpt(&source.text).contains(&quote))
        };
        // A genuine excerpt must also support the numbers shown in its fact field.
        self.facts.retain(|entry| {
            cited(entry)
                && numbers_cited(&entry.text, &entry.quote)
                && numbers_cited(&entry.text_en, &entry.quote)
        });
        self.facts.truncate(facts);
        self.sections.retain(cited);
        self.sections.truncate(6);
        self.links.retain_mut(|link| {
            let Some(url) = public_url(&link.url) else {
                return false;
            };
            link.url = url;
            sources.iter().any(|s| {
                public_url(&s.url).as_ref() == Some(&link.url)
                    || s.links.iter().any(|known| known.url == link.url)
            })
        });
        self.links.truncate(5);
        self.sources = sources
            .iter()
            .map(|s| Reference {
                title: s.title.clone(),
                url: s.url.clone(),
            })
            .collect();
        Ok(())
    }
}

/// Whether every number a field shows stands whole in its quote, so 8.6 never cites 6.8.
fn numbers_cited(text: &str, quote: &str) -> bool {
    let numbers = |s: &str| {
        s.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
            .map(|n| n.trim_matches(['.', ',']))
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let evidence = numbers(quote);
    numbers(text).iter().all(|n| evidence.contains(n))
}

#[derive(sqlx::FromRow)]
struct Pending {
    id: Uuid,
    name: String,
    name_en: String,
    creator: String,
    kind: String,
    source_urls: sqlx::types::Json<Vec<String>>,
    source_hash: Option<Vec<u8>>,
}

fn guide_request(topic: &Topic, section: Section, entry: &Pending, sources: &[Source]) -> Value {
    let string = json!({"type":"string"});
    let field = json!({"type":"object", "additionalProperties":false,
        "required":["label","label_en","text","text_en","source","quote"],
        "properties":{"label":string,"label_en":string,"text":string,"text_en":string,
            "source":{"type":"integer"},"quote":string}});
    let (instructions, effort) = match section {
        Section::Culture => culture::guide(topic),
        Section::Gadgets => gadgets::guide(&topic.id, &entry.kind),
    };
    json!({"model":topic.model,"store":false,"reasoning":{"effort":effort},
    "instructions":instructions,
    "input":json!({"name":entry.name,"name_en":entry.name_en,"creator":entry.creator,"sources":sources}).to_string(),
    "text":{"format":{"type":"json_schema","name":"catalog_guide","strict":true,"schema":{
        "type":"object","additionalProperties":false,"required":["intro","intro_en","facts","sections","links"],
        "properties":{"intro":string,"intro_en":string,
            "facts":{"type":"array","items":field},"sections":{"type":"array","items":field},
            "links":{"type":"array","items":{"type":"object","additionalProperties":false,
                "required":["label","label_en","url"],"properties":{"label":string,"label_en":string,"url":string}}}}
    }}}})
}

enum Composed {
    Written(Guide, Vec<u8>, Vec<NewsImage>),
    Unchanged,
    Unavailable,
}

async fn compose(
    state: &AppState,
    topic: &Topic,
    section: Section,
    entry: &Pending,
    usage: &mut Usage,
) -> Result<Composed> {
    let mut sources = Vec::new();
    for url in &entry.source_urls.0 {
        match source::read(state, url, section.source_limit()).await {
            // What was read must still be official: a redirect or a post's author may differ.
            Ok(source)
                if section == Section::Culture || gadgets::official(&topic.brief, &source.url) =>
            {
                sources.push(source)
            }
            Ok(source) => tracing::debug!(url = %source.url, "catalogue source is not official"),
            Err(error) => tracing::debug!(%error, %url, "catalogue source unavailable"),
        }
    }
    if sources.is_empty() {
        return Ok(Composed::Unavailable);
    }
    if !sources
        .iter()
        .any(|source| named(section, entry, &source.text))
    {
        return Err(AppError::Internal("资料未确认名称与出品方".into()));
    }
    let hash = Sha256::digest(serde_json::to_vec(&sources)?).to_vec();
    if entry.source_hash.as_ref() == Some(&hash) {
        return Ok(Composed::Unchanged);
    }
    let reply = crate::ai::request(state, guide_request(topic, section, entry, &sources)).await?;
    *usage += Usage::from_reply(&reply);
    let mut guide: Guide = serde_json::from_value(crate::ai::parse_payload(&reply)?)?;
    guide.verify(&sources, section.facts_limit())?;
    let images = match section {
        Section::Culture => Vec::new(),
        Section::Gadgets => official_images(state, entry, &sources).await,
    };
    Ok(Composed::Written(guide, hash, images))
}

/// Whether a source names the entry: a work by its name and its creator, a product by every
/// word of its model name, which official pages spread over a heading, a year or a generation.
fn named(section: Section, entry: &Pending, text: &str) -> bool {
    let text = fold(text);
    match section {
        Section::Culture => {
            (text.contains(&fold(&entry.name)) || text.contains(&fold(&entry.name_en)))
                && (entry.creator.is_empty() || text.contains(&fold(&entry.creator)))
        }
        Section::Gadgets => {
            let brand = fold(&entry.creator);
            [&entry.name, &entry.name_en].into_iter().any(|name| {
                let words: Vec<String> = name
                    .split(|c: char| !c.is_alphanumeric())
                    .map(fold)
                    .filter(|word| !word.is_empty() && *word != brand)
                    .collect();
                !words.is_empty() && words.iter().all(|word| text.contains(word.as_str()))
            })
        }
    }
}

/// Up to four pictures a product's official pages and posts show, kept on the file server;
/// eight are tried, as an official page may still serve a placeholder.
async fn official_images(state: &AppState, entry: &Pending, sources: &[Source]) -> Vec<NewsImage> {
    let mut images: Vec<Option<NewsImage>> = Vec::new();
    for source in sources {
        for url in &source.images {
            if images.len() < 8 && !images.iter().flatten().any(|known| &known.url == url) {
                images.push(Some(NewsImage {
                    url: url.clone(),
                    source_url: source.url.clone(),
                    caption: entry.name.clone(),
                    caption_en: entry.name_en.clone(),
                    credit: entry.creator.clone(),
                    origin_url: None,
                    expires_at: None,
                    content_hash: None,
                    width: None,
                    height: None,
                }));
            }
        }
    }
    media::mirror_images(state, &mut images).await;
    images.into_iter().flatten().take(4).collect()
}

/// Two guides at a time, at most eight per run; changed sources refresh older guides weekly.
pub(crate) async fn guides(
    state: &AppState,
    topic: &Topic,
    section: Section,
    usage: &mut Usage,
    deadline: Instant,
) -> Result<Vec<Value>> {
    let pending = sqlx::query_as::<_, Pending>("SELECT id, name, name_en, creator, kind, source_urls, source_hash FROM catalog_entries e
        WHERE topic_id = $1 AND attempts < 3 AND jsonb_array_length(source_urls) > 0
        AND (guide IS NULL OR checked_at < NOW() - INTERVAL '7 days')
        AND EXISTS (SELECT 1 FROM catalog_items c JOIN items i ON i.id = c.item_id WHERE c.entry_id = e.id AND NOT i.hidden)
        ORDER BY guide IS NOT NULL, checked_at NULLS FIRST, id LIMIT 8")
        .bind(&topic.id).fetch_all(&state.pool).await?;
    let mut pending = pending.into_iter();
    let mut jobs = JoinSet::new();
    let mut outcomes = Vec::new();
    let mut changed = Vec::new();
    loop {
        while jobs.len() < 2 && Instant::now() + Duration::from_secs(60) < deadline {
            let Some(entry) = pending.next() else {
                break;
            };
            let (state, topic) = (state.clone(), topic.clone());
            jobs.spawn(async move {
                let mut usage = Usage::default();
                let result = timeout_at(
                    deadline.min(Instant::now() + Duration::from_secs(420)),
                    compose(&state, &topic, section, &entry, &mut usage),
                )
                .await;
                (entry.id, result, usage)
            });
        }
        let Some(job) = jobs.join_next().await else {
            break;
        };
        let (id, result, spent) =
            job.map_err(|error| AppError::Internal(error.to_string().into()))?;
        *usage += spent;
        match result {
            Ok(Ok(Composed::Written(guide, hash, images))) => {
                sqlx::query("UPDATE catalog_entries SET guide = $2, source_hash = $3, guide_at = NOW(), checked_at = NOW(), attempts = 0,
                        images = CASE WHEN jsonb_array_length($4) > 0 THEN $4 ELSE images END,
                        image = COALESCE($4->0, image) WHERE id = $1")
                    .bind(id).bind(json!(guide)).bind(hash).bind(json!(images)).execute(&state.pool).await?;
                changed.push(id);
                outcomes.push(json!({"entry":id,"written":true}));
            }
            Ok(Ok(Composed::Unchanged)) => {
                sqlx::query(
                    "UPDATE catalog_entries SET checked_at = NOW(), attempts = 0 WHERE id = $1",
                )
                .bind(id)
                .execute(&state.pool)
                .await?;
                outcomes.push(json!({"entry":id,"unchanged":true}));
            }
            Ok(Ok(Composed::Unavailable)) => {
                // Unreadable drafts rotate behind new entries instead of occupying every slot.
                sqlx::query("UPDATE catalog_entries SET checked_at = NOW() WHERE id = $1")
                    .bind(id)
                    .execute(&state.pool)
                    .await?;
                outcomes.push(json!({"entry":id,"deferred":"资料暂不可读"}));
            }
            Err(_) if Instant::now() >= deadline => {
                outcomes.push(json!({"entry":id,"deferred":true}))
            }
            result => {
                let reason = match result {
                    Ok(Err(error)) => error.to_string(),
                    _ => "导读生成超时".into(),
                };
                sqlx::query("UPDATE catalog_entries SET checked_at = NOW(), attempts = attempts + 1 WHERE id = $1")
                    .bind(id).execute(&state.pool).await?;
                outcomes.push(json!({"entry":id,"error":reason}));
            }
        }
    }
    if !changed.is_empty() {
        sqlx::query("UPDATE topics SET feed_revision = feed_revision + 1 WHERE id = $1")
            .bind(&topic.id)
            .execute(&state.pool)
            .await?;
        let days: Vec<chrono::NaiveDate> = sqlx::query_scalar("SELECT DISTINCT i.day FROM catalog_items c JOIN items i ON i.id = c.item_id WHERE c.entry_id = ANY($1)")
            .bind(changed).fetch_all(&state.pool).await?;
        for day in days {
            state.digest.refresh(state, &topic.id, day).await?;
        }
    }
    Ok(outcomes)
}
