//! The catalogue in PostgreSQL: providers and their pages, the published models and the
//! changes of the last day. One provider's read is applied in one transaction.
use super::{
    Currency, Links, Model, Price, Status, current, headline,
    pages::tokens,
    paired, row_moved,
    source::{Kind, Page, Source},
    verify::mentions,
};
use crate::{error::Result, state::AppState};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, types::Json};
use std::collections::HashSet;

/// Where a provider stands. Listed providers are read every day; the weekly pass tries
/// candidates and providers that left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Standing {
    Active,
    Candidate,
    Offline,
}

impl Standing {
    fn parse(value: &str) -> Self {
        match value {
            "candidate" => Self::Candidate,
            "offline" => Self::Offline,
            _ => Self::Active,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Candidate => "candidate",
            Self::Offline => "offline",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Vendor {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) region: String,
    pub(crate) homepage: String,
    pub(crate) note: String,
    pub(crate) domains: Vec<String>,
    pub(crate) standing: Standing,
    pub(crate) sources: Vec<Source>,
    /// Models currently published for the provider.
    pub(crate) listed: i64,
}

#[derive(FromRow)]
struct VendorRow {
    id: String,
    name: String,
    region: String,
    homepage: String,
    note: String,
    domains: Vec<String>,
    state: String,
    listed: i64,
}

/// Which providers a pass reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    /// Listed providers, read every day.
    Listed,
    /// Listed providers whose pages have not confirmed their catalogue for two weeks.
    Stale,
    /// Candidates and providers that left, not tried for about a week.
    Waiting,
}

const VENDORS: &str = "SELECT v.id, v.name, v.region, v.homepage, v.note, v.domains, v.state,
        (SELECT COUNT(*) FROM llm_models m WHERE m.vendor_id = v.id AND m.removed_at IS NULL) AS listed
     FROM llm_vendors v
     WHERE v.enabled AND CASE $1
        WHEN 'listed' THEN v.state = 'active'
        WHEN 'stale' THEN v.state = 'active'
            AND COALESCE(v.verified_at, v.state_changed_at, v.created_at) < NOW() - INTERVAL '14 days'
        ELSE v.state <> 'active'
            AND (v.checked_at IS NULL OR v.checked_at < NOW() - INTERVAL '6 days 12 hours')
     END
     ORDER BY v.sort_weight, v.id";

#[derive(FromRow)]
struct SourceRow {
    id: i64,
    vendor_id: String,
    kind: String,
    url: String,
    currency: Option<String>,
    content_hash: Option<Vec<u8>>,
}

/// Enabled providers of one selection with their enabled pages, in display order. A listed
/// provider without pages is left to the weekly pass.
pub(crate) async fn vendors(state: &AppState, selection: Selection) -> Result<Vec<Vendor>> {
    let rows: Vec<VendorRow> = sqlx::query_as(VENDORS)
        .bind(match selection {
            Selection::Listed => "listed",
            Selection::Stale => "stale",
            Selection::Waiting => "waiting",
        })
        .fetch_all(&state.pool)
        .await?;
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    let sources: Vec<SourceRow> = sqlx::query_as(
        "SELECT id, vendor_id, kind, url, currency, content_hash FROM llm_sources
         WHERE vendor_id = ANY($1) ORDER BY sort_weight, id",
    )
    .bind(&ids)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Vendor {
            sources: sources
                .iter()
                .filter(|source| source.vendor_id == row.id)
                .filter_map(|source| {
                    Some(Source {
                        id: source.id,
                        kind: Kind::parse(&source.kind)?,
                        url: source.url.clone(),
                        currency: source.currency.as_deref().and_then(Currency::parse),
                        content_hash: source.content_hash.clone(),
                    })
                })
                .collect(),
            id: row.id,
            name: row.name,
            region: row.region,
            homepage: row.homepage,
            note: row.note,
            domains: row.domains,
            standing: Standing::parse(&row.state),
            listed: row.listed,
        })
        .filter(|vendor| selection != Selection::Listed || !vendor.sources.is_empty())
        .collect())
}

pub(crate) async fn listed_ids(state: &AppState, vendor: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT model_id FROM llm_models WHERE vendor_id = $1 AND removed_at IS NULL ORDER BY model_id",
    )
    .bind(vendor)
    .fetch_all(&state.pool)
    .await?)
}

pub(crate) async fn source_failed(state: &AppState, source: i64, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE llm_sources SET checked_at = NOW(), failures = failures + 1, error = $2 WHERE id = $1",
    )
    .bind(source)
    .bind(clip(error))
    .execute(&state.pool)
    .await?;
    Ok(())
}

pub(crate) async fn vendor_failed(state: &AppState, vendor: &str, error: &str) -> Result<()> {
    sqlx::query("UPDATE llm_vendors SET checked_at = NOW(), error = $2 WHERE id = $1")
        .bind(vendor)
        .bind(clip(error))
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Unchanged pages still confirm the published catalogue as current.
pub(crate) async fn confirm_unchanged(state: &AppState, vendor: &Vendor) -> Result<()> {
    let ids: Vec<i64> = vendor.sources.iter().map(|source| source.id).collect();
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        "UPDATE llm_sources SET checked_at = NOW(), failures = 0, error = NULL WHERE id = ANY($1)",
    )
    .bind(&ids)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE llm_vendors SET checked_at = NOW(), verified_at = NOW(), error = NULL WHERE id = $1",
    )
    .bind(&vendor.id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

fn clip(error: &str) -> String {
    error.chars().take(300).collect()
}

#[derive(FromRow)]
struct Stored {
    model_id: String,
    status: String,
    knowledge_cutoff: String,
    context_tokens: Option<i32>,
    max_output_tokens: Option<i32>,
    input_modalities: Vec<String>,
    output_modalities: Vec<String>,
    features: Vec<String>,
    prices: Json<Vec<Price>>,
    links: Json<Links>,
    evidence: Json<Value>,
    removed_at: Option<DateTime<Utc>>,
}

struct Change {
    model_id: String,
    kind: &'static str,
    before: Option<Value>,
    after: Option<Value>,
}

fn snapshot(prices: &[Price], context: Option<i32>, output: Option<i32>) -> Value {
    json!({"prices": prices, "context_tokens": context, "max_output_tokens": output})
}

fn or_kept(new: Vec<String>, old: &[String]) -> Vec<String> {
    if new.is_empty() { old.to_vec() } else { new }
}

/// Two reads of the same page can label the same rows differently, list more or fewer
/// tiers, or write 1M as 1048576 tokens. A price change is therefore a moved value in a
/// tier both reads describe, or in the standard rate a currency's table leads with.
pub(crate) fn price_moved(old: &[Price], new: &[Price]) -> bool {
    let standard = [Currency::Usd, Currency::Cny].into_iter().any(|currency| {
        match (headline(old, currency), headline(new, currency)) {
            (Some(a), Some(b)) => row_moved(a, b),
            _ => false,
        }
    });
    standard || paired(old, new).into_iter().any(|(a, b)| row_moved(a, b))
}

pub(crate) fn limit_moved(old: Option<i32>, new: Option<i32>) -> bool {
    matches!((old, new), (Some(old), Some(new)) if tokens(old) != tokens(new))
}

/// Whether a model is "legacy" is a reading of the page; general availability and a
/// deprecation are events.
pub(crate) fn status_moved(old: &str, new: Status) -> bool {
    (new == Status::Deprecated && old != "deprecated")
        || (old == "preview" && new == Status::Active)
}

/// Publishes one provider's verified read and records what changed.
pub(crate) async fn apply(
    state: &AppState,
    vendor: &Vendor,
    pages: &[Page],
    models: Vec<Model>,
    complete: bool,
) -> Result<usize> {
    let mut transaction = state.pool.begin().await?;
    let changes = write(&mut transaction, vendor, pages, &models, complete).await?;
    transaction.commit().await?;
    Ok(changes)
}

/// The first read of a provider is an import, not a list of new releases. A model leaves
/// the catalogue only once none of the provider's pages mentions it, so a model the read
/// skipped stays. Page digests are kept only for a complete read, so a partial one is
/// read again.
async fn write(
    connection: &mut PgConnection,
    vendor: &Vendor,
    pages: &[Page],
    models: &[Model],
    complete: bool,
) -> Result<usize> {
    // Whether most of the catalogue has been missing from the pages for a week.
    let hold_expired: bool = sqlx::query_scalar(
        "SELECT COALESCE(held_since <= NOW() - INTERVAL '7 days', false)
         FROM llm_vendors WHERE id = $1 FOR UPDATE",
    )
    .bind(&vendor.id)
    .fetch_one(&mut *connection)
    .await?;
    let stored: Vec<Stored> = sqlx::query_as(
        "SELECT model_id, status, knowledge_cutoff, context_tokens, max_output_tokens,
                input_modalities, output_modalities, features, prices, links, evidence, removed_at
         FROM llm_models WHERE vendor_id = $1 FOR UPDATE",
    )
    .bind(&vendor.id)
    .fetch_all(&mut *connection)
    .await?;
    let import = stored.is_empty();
    // A model the previous pages already named was on the page all along, not released.
    let previous: Vec<String> = sqlx::query_scalar(
        "SELECT content FROM llm_sources WHERE vendor_id = $1 AND content IS NOT NULL",
    )
    .bind(&vendor.id)
    .fetch_all(&mut *connection)
    .await?;
    let mut changes = Vec::new();
    for (position, model) in models.iter().enumerate() {
        let old = stored.iter().find(|old| old.model_id == model.model_id);
        let prices = match (&model.prices, old) {
            (Some(prices), _) => prices.clone(),
            (None, Some(old)) => old.prices.0.clone(),
            (None, None) => Vec::new(),
        };
        let context = model
            .context_tokens
            .or(old.and_then(|old| old.context_tokens));
        let output = model
            .max_output_tokens
            .or(old.and_then(|old| old.max_output_tokens));
        let status = model.status.as_str();
        let mut changed = true;
        match old {
            None if import => {}
            None if previous.iter().any(|text| mentions(text, &model.model_id)) => {}
            None => changes.push(Change {
                model_id: model.model_id.clone(),
                kind: "added",
                before: None,
                after: Some(snapshot(&prices, context, output)),
            }),
            Some(old) if old.removed_at.is_some() => changes.push(Change {
                model_id: model.model_id.clone(),
                kind: "added",
                before: None,
                after: Some(snapshot(&prices, context, output)),
            }),
            Some(old) => {
                let before = changes.len();
                // A model's first verified prices or limits are not a change.
                if model.prices.is_some() && price_moved(&old.prices.0, &prices) {
                    changes.push(Change {
                        model_id: model.model_id.clone(),
                        kind: "price",
                        before: Some(json!({"prices": old.prices.0})),
                        after: Some(json!({"prices": prices})),
                    });
                }
                if limit_moved(old.context_tokens, context)
                    || limit_moved(old.max_output_tokens, output)
                {
                    changes.push(Change {
                        model_id: model.model_id.clone(),
                        kind: "spec",
                        before: Some(json!({"context_tokens": old.context_tokens, "max_output_tokens": old.max_output_tokens})),
                        after: Some(json!({"context_tokens": context, "max_output_tokens": output})),
                    });
                }
                if status_moved(&old.status, model.status) {
                    changes.push(Change {
                        model_id: model.model_id.clone(),
                        kind: "status",
                        before: Some(json!({"status": old.status})),
                        after: Some(json!({"status": status})),
                    });
                }
                changed = changes.len() > before;
            }
        }
        let links = Links {
            docs: model
                .links
                .docs
                .clone()
                .or_else(|| old.and_then(|old| old.links.0.docs.clone())),
            announcement: model
                .links
                .announcement
                .clone()
                .or_else(|| old.and_then(|old| old.links.0.announcement.clone())),
        };
        // Evidence for kept prices stays with those prices.
        let mut evidence = model.evidence.clone();
        if model.prices.is_none()
            && let Some(old) = old
        {
            evidence["prices"] = old.evidence.0["prices"].clone();
        }
        let empty = Vec::new();
        let kept = |pick: fn(&Stored) -> &Vec<String>| old.map_or(&empty, pick);
        sqlx::query(
            "INSERT INTO llm_models (vendor_id, model_id, name, summary, summary_en, status,
                knowledge_cutoff, context_tokens, max_output_tokens, input_modalities, output_modalities,
                features, prices, links, evidence, position)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $17)
             ON CONFLICT (vendor_id, model_id) DO UPDATE SET
                name = EXCLUDED.name, summary = EXCLUDED.summary, summary_en = EXCLUDED.summary_en,
                status = EXCLUDED.status,
                knowledge_cutoff = EXCLUDED.knowledge_cutoff, context_tokens = EXCLUDED.context_tokens,
                max_output_tokens = EXCLUDED.max_output_tokens, input_modalities = EXCLUDED.input_modalities,
                output_modalities = EXCLUDED.output_modalities, features = EXCLUDED.features,
                prices = EXCLUDED.prices, links = EXCLUDED.links, evidence = EXCLUDED.evidence,
                position = EXCLUDED.position, verified_at = NOW(), removed_at = NULL,
                changed_at = CASE WHEN $16 THEN NOW() ELSE llm_models.changed_at END",
        )
        .bind(&vendor.id)
        .bind(&model.model_id)
        .bind(&model.name)
        .bind(&model.summary)
        .bind(&model.summary_en)
        .bind(status)
        .bind(if model.knowledge_cutoff.is_empty() {
            old.map_or("", |old| old.knowledge_cutoff.as_str())
        } else {
            model.knowledge_cutoff.as_str()
        })
        .bind(context)
        .bind(output)
        .bind(or_kept(model.input_modalities.clone(), kept(|old| &old.input_modalities)))
        .bind(or_kept(model.output_modalities.clone(), kept(|old| &old.output_modalities)))
        .bind(or_kept(model.features.clone(), kept(|old| &old.features)))
        .bind(Json(&prices))
        .bind(Json(&links))
        .bind(Json(&evidence))
        .bind(changed)
        .bind(position as i32)
        .execute(&mut *connection)
        .await?;
    }
    let read: HashSet<&str> = models.iter().map(|model| model.model_id.as_str()).collect();
    let texts: Vec<String> = pages.iter().map(Page::text).collect();
    let gone: Vec<&Stored> = stored
        .iter()
        .filter(|old| {
            old.removed_at.is_none()
                && !read.contains(old.model_id.as_str())
                && !texts.iter().any(|text| mentions(text, &old.model_id))
        })
        .collect();
    let listed = stored.iter().filter(|old| old.removed_at.is_none()).count();
    // Providers retire a few models at a time. Most of a catalogue leaving at once points
    // at a changed or broken page, so those models stay until the pages have left them out
    // for a week.
    let held = gone.len() > 1 && gone.len() * 2 > listed && !hold_expired;
    for old in gone.iter().filter(|_| !held) {
        sqlx::query(
            "UPDATE llm_models SET removed_at = NOW(), changed_at = NOW() WHERE vendor_id = $1 AND model_id = $2",
        )
        .bind(&vendor.id)
        .bind(&old.model_id)
        .execute(&mut *connection)
        .await?;
        changes.push(Change {
            model_id: old.model_id.clone(),
            kind: "removed",
            before: Some(snapshot(
                &old.prices.0,
                old.context_tokens,
                old.max_output_tokens,
            )),
            after: None,
        });
    }
    let source_url = source_url(pages);
    for change in &changes {
        sqlx::query(
            "INSERT INTO llm_changes (vendor_id, model_id, kind, before, after, source_url)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&vendor.id)
        .bind(&change.model_id)
        .bind(change.kind)
        .bind(change.before.as_ref().map(Json))
        .bind(change.after.as_ref().map(Json))
        .bind(source_url)
        .execute(&mut *connection)
        .await?;
    }
    for page in pages {
        sqlx::query(
            "UPDATE llm_sources SET checked_at = NOW(), failures = 0, error = NULL,
                changed_at = CASE WHEN $3 AND content_hash IS DISTINCT FROM $2 THEN NOW() ELSE changed_at END,
                content_hash = CASE WHEN $3 THEN $2 ELSE content_hash END,
                content = CASE WHEN $3 THEN $4 ELSE content END
             WHERE id = $1",
        )
        .bind(page.source_id)
        .bind(&page.hash)
        .bind(complete && !held)
        .bind(page.text())
        .execute(&mut *connection)
        .await?;
    }
    let error = if held {
        Some("官方页面不再列出多数已收录模型，暂缓下线；一周后页面仍如此时下线")
    } else {
        (!complete).then_some("部分模型未通过核验，下次运行重新读取")
    };
    sqlx::query(
        "UPDATE llm_vendors SET checked_at = NOW(), verified_at = NOW(), error = $2,
            held_since = CASE WHEN $3 THEN COALESCE(held_since, NOW()) END
         WHERE id = $1",
    )
    .bind(&vendor.id)
    .bind(error)
    .bind(held)
    .execute(&mut *connection)
    .await?;
    Ok(changes.len())
}

/// The page a change is attributed to: the price list, else the first page read.
fn source_url(pages: &[Page]) -> &str {
    pages
        .iter()
        .find(|page| page.kind == Kind::Pricing)
        .or(pages.first())
        .map_or("", |page| page.url.as_str())
}

/// Lists a provider from pages found for it. They replace its registered pages and its
/// models are published; a provider that was not listed arrives with a record of it.
pub(crate) async fn publish(
    state: &AppState,
    vendor: &Vendor,
    mut pages: Vec<Page>,
    models: Vec<Model>,
    complete: bool,
) -> Result<usize> {
    let mut transaction = state.pool.begin().await?;
    let mut ids = Vec::with_capacity(pages.len());
    for (weight, page) in (0..).step_by(10).zip(pages.iter_mut()) {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO llm_sources (vendor_id, kind, url, currency, sort_weight)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (vendor_id, url) DO UPDATE SET kind = EXCLUDED.kind,
                currency = EXCLUDED.currency, sort_weight = EXCLUDED.sort_weight
             RETURNING id",
        )
        .bind(&vendor.id)
        .bind(page.kind.as_str())
        .bind(&page.url)
        .bind(page.currency.map(Currency::as_str))
        .bind(weight)
        .fetch_one(&mut *transaction)
        .await?;
        page.source_id = id;
        ids.push(id);
    }
    // The pages being replaced still name the models they listed, so none of those is new.
    let mut changes = write(&mut transaction, vendor, &pages, &models, complete).await?;
    sqlx::query("DELETE FROM llm_sources WHERE vendor_id = $1 AND id <> ALL($2)")
        .bind(&vendor.id)
        .bind(&ids)
        .execute(&mut *transaction)
        .await?;
    if vendor.standing != Standing::Active {
        let shown = current(&models, |model| {
            (
                vendor.id.as_str(),
                model.model_id.as_str(),
                model.status.as_str(),
                model
                    .prices
                    .as_ref()
                    .is_some_and(|prices| !prices.is_empty()),
            )
        })
        .len();
        sqlx::query(
            "UPDATE llm_vendors SET state = 'active', state_changed_at = NOW() WHERE id = $1",
        )
        .bind(&vendor.id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            "INSERT INTO llm_changes (vendor_id, kind, before, after, source_url)
             VALUES ($1, 'vendor_added', $2, $3, $4)",
        )
        .bind(&vendor.id)
        .bind(Json(json!({"state": vendor.standing.as_str()})))
        .bind(Json(json!({"models": shown})))
        .bind(source_url(&pages))
        .execute(&mut *transaction)
        .await?;
        changes += 1;
    }
    transaction.commit().await?;
    Ok(changes)
}

/// Takes a provider out of the catalogue once its pages have not confirmed it for two weeks
/// and none could be found from its homepage. Its models go with it: if it returns, its
/// pages are read as a new provider's.
pub(crate) async fn retire(state: &AppState, vendor: &Vendor, reason: &str) -> Result<()> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query("SELECT 1 FROM llm_vendors WHERE id = $1 FOR UPDATE")
        .bind(&vendor.id)
        .execute(&mut *transaction)
        .await?;
    let models = sqlx::query("DELETE FROM llm_models WHERE vendor_id = $1")
        .bind(&vendor.id)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    sqlx::query("UPDATE llm_sources SET content = NULL, content_hash = NULL WHERE vendor_id = $1")
        .bind(&vendor.id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE llm_vendors SET state = 'offline', state_changed_at = NOW(), checked_at = NOW(),
            held_since = NULL, error = $2
         WHERE id = $1",
    )
    .bind(&vendor.id)
    .bind(clip(reason))
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "INSERT INTO llm_changes (vendor_id, kind, before, source_url)
         VALUES ($1, 'vendor_removed', $2, $3)",
    )
    .bind(&vendor.id)
    .bind(Json(json!({"models": models})))
    .bind(&vendor.homepage)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// Every provider the catalogue knows, in any state: what discovery must not add again.
#[derive(Clone, Debug, FromRow)]
pub(crate) struct Known {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) name_en: String,
    pub(crate) region: String,
    pub(crate) domains: Vec<String>,
    /// Its registered pages.
    pub(crate) urls: Vec<String>,
}

pub(crate) async fn known(state: &AppState) -> Result<Vec<Known>> {
    Ok(sqlx::query_as(
        "SELECT v.id, v.name, v.name_en, v.region, v.domains,
                COALESCE(array_agg(s.url) FILTER (WHERE s.url IS NOT NULL), '{}') AS urls
         FROM llm_vendors v LEFT JOIN llm_sources s ON s.vendor_id = v.id
         GROUP BY v.id ORDER BY v.sort_weight, v.id",
    )
    .fetch_all(&state.pool)
    .await?)
}

/// Registers a price page an administrator pointed to; a provider not listed yet is tried
/// by this run.
pub(crate) async fn register(
    state: &AppState,
    vendor: &str,
    url: &str,
    currency: Currency,
) -> Result<()> {
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        "INSERT INTO llm_sources (vendor_id, kind, url, currency, sort_weight)
         SELECT $1, 'pricing', $2, $3, COALESCE(MAX(sort_weight), 0) + 10
         FROM llm_sources WHERE vendor_id = $1
         ON CONFLICT (vendor_id, url) DO NOTHING",
    )
    .bind(vendor)
    .bind(url)
    .bind(currency.as_str())
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE llm_vendors SET checked_at = NULL WHERE id = $1 AND state <> 'active'")
        .bind(vendor)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

/// A provider the news reported, to be tried before it is listed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Proposal {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) name_en: String,
    pub(crate) region: &'static str,
    pub(crate) homepage: String,
    /// Where its price list is thought to be: the first page tried.
    pub(crate) pricing: Option<String>,
    pub(crate) domains: Vec<String>,
    pub(crate) note: String,
}

pub(crate) async fn propose(state: &AppState, proposal: &Proposal) -> Result<bool> {
    let mut transaction = state.pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO llm_vendors (id, name, name_en, region, homepage, domains, note, state,
            origin, sort_weight, state_changed_at)
         SELECT $1, $2, $3, $4, $5, $6, $7, 'candidate', 'discovered',
            COALESCE(MAX(sort_weight), 0) + 10, NOW()
         FROM llm_vendors
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(&proposal.id)
    .bind(&proposal.name)
    .bind(&proposal.name_en)
    .bind(proposal.region)
    .bind(&proposal.homepage)
    .bind(&proposal.domains)
    .bind(&proposal.note)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(false);
    }
    if let Some(url) = &proposal.pricing {
        let currency = if proposal.region == "cn" {
            Currency::Cny
        } else {
            Currency::Usd
        };
        sqlx::query(
            "INSERT INTO llm_sources (vendor_id, kind, url, currency) VALUES ($1, 'pricing', $2, $3)",
        )
        .bind(&proposal.id)
        .bind(url)
        .bind(currency.as_str())
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(true)
}

/// The catalogue keeps no history nobody reads: a change is news for a day, a withdrawn
/// model leaves with its last change, and a discovered provider not listed within eight
/// weeks is forgotten, so the news can propose it afresh.
pub(crate) async fn prune(state: &AppState) -> Result<u64> {
    let mut transaction = state.pool.begin().await?;
    let mut total = 0;
    for statement in [
        "DELETE FROM llm_changes WHERE detected_at < NOW() - INTERVAL '1 day'",
        "DELETE FROM llm_models WHERE removed_at < NOW() - INTERVAL '1 day'",
        "DELETE FROM llm_vendors
         WHERE origin = 'discovered' AND state <> 'active'
           AND COALESCE(state_changed_at, created_at) < NOW() - INTERVAL '56 days'",
    ] {
        total += sqlx::query(statement)
            .execute(&mut *transaction)
            .await?
            .rows_affected();
    }
    transaction.commit().await?;
    Ok(total)
}
