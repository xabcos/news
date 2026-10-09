//! 𝑳𝑳𝑴: mainstream model prices and specs, kept from each provider's own documentation:
//! language models, and the image, video, audio, embedding and rerank models of the same
//! providers. Official pages are read as text. The model only turns them into rows; a price,
//! a limit or a date is published only when the page states it verbatim, and a provider
//! whose pages have not changed is not read again.
use crate::{error::Result, fetch::Usage, state::AppState, topics::Topic};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinSet, time::Instant};

mod admin;
mod extract;
mod lifecycle;
mod pages;
mod source;
mod store;
mod verify;

#[cfg(test)]
mod tests;

pub(crate) const TOPIC_ID: &str = "llm";
/// Providers read at the same time within one run, each in two requests.
const VENDOR_CONCURRENCY: usize = 6;
/// Run time kept back for recording the run.
const RUN_MARGIN: Duration = Duration::from_secs(90);

pub(crate) fn is_catalog(topic: &Topic) -> bool {
    topic.id == TOPIC_ID && topic.owner_user_id.is_none()
}

pub(crate) fn router() -> Router<AppState> {
    pages::router().merge(admin::router())
}

/// Removes history past its use; see `store::prune`.
pub(crate) async fn prune(state: &AppState) -> Result<u64> {
    store::prune(state).await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub(crate) enum Currency {
    Usd,
    Cny,
}

impl Currency {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "USD" => Some(Self::Usd),
            "CNY" => Some(Self::Cny),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Usd => "USD",
            Self::Cny => "CNY",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Self::Usd => "$",
            Self::Cny => "¥",
        }
    }
}

/// What a model is for. Each is listed apart and priced in its own units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Category {
    Text,
    Image,
    Video,
    Audio,
    /// Embedding and rerank models.
    Embedding,
}

impl Category {
    /// The order the catalogue lists them in.
    pub(crate) const ALL: [Self; 5] = [
        Self::Text,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Embedding,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Embedding => "embedding",
        }
    }

    pub(crate) fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
            .unwrap_or(Self::Text)
    }

    /// Image, video and audio models, priced by size, resolution or duration rather than
    /// by a standard token rate.
    pub(crate) fn media(self) -> bool {
        matches!(self, Self::Image | Self::Video | Self::Audio)
    }

    /// The units a row of this category is shown by, the usual one first.
    fn units(self) -> &'static [Unit] {
        match self {
            Self::Text => &[Unit::Tokens],
            Self::Image => &[Unit::Image, Unit::Tokens, Unit::Request],
            Self::Video => &[Unit::Second, Unit::Request, Unit::Tokens],
            Self::Audio => &[
                Unit::Minute,
                Unit::Hour,
                Unit::Second,
                Unit::Characters,
                Unit::Tokens,
                Unit::Request,
            ],
            Self::Embedding => &[Unit::Tokens, Unit::Request],
        }
    }
}

/// What a price is for: 1M tokens or characters, or one image, second, minute, hour or
/// request (a call or one generated video).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Unit {
    Tokens,
    Characters,
    Image,
    Second,
    Minute,
    Hour,
    Request,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tier {
    Standard,
    Batch,
    Flex,
    Priority,
}

/// Context-length band of a price; `All` when the provider does not split by length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Band {
    All,
    Short,
    Long,
}

/// Time-of-day band, as with peak and off-peak rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Period {
    All,
    Peak,
    OffPeak,
}

/// Separate rates for thinking and non-thinking output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    All,
    Thinking,
    NonThinking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Preview,
    Active,
    Legacy,
    Deprecated,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Active => "active",
            Self::Legacy => "legacy",
            Self::Deprecated => "deprecated",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "preview" => Self::Preview,
            "legacy" => Self::Legacy,
            "deprecated" => Self::Deprecated,
            _ => Self::Active,
        }
    }
}

/// One verified row of a provider's price table in its own currency and unit. `input` is
/// what is sent (text, audio, characters to speak, a request) and `output` what comes back
/// (text, an image, seconds of video).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Price {
    pub(crate) currency: Currency,
    pub(crate) unit: Unit,
    pub(crate) tier: Tier,
    /// The provider's own name for the tier, such as "Fast" or "Ultrafast".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) label: String,
    pub(crate) context: Band,
    /// Token count where the long-context rate begins, when the page states it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context_threshold: Option<i64>,
    pub(crate) period: Period,
    pub(crate) mode: Mode,
    pub(crate) input: Option<f64>,
    pub(crate) cached_input: Option<f64>,
    /// Cache write, or its 5-minute rate where the provider sells two durations.
    pub(crate) cache_write: Option<f64>,
    pub(crate) cache_write_1h: Option<f64>,
    pub(crate) output: Option<f64>,
}

/// What tells two rows of one model apart; several long-context tiers differ by threshold.
type PriceKey<'a> = (
    Currency,
    Unit,
    Tier,
    &'a str,
    Band,
    Option<i64>,
    Period,
    Mode,
);

impl Price {
    pub(crate) fn key(&self) -> PriceKey<'_> {
        (
            self.currency,
            self.unit,
            self.tier,
            self.label.as_str(),
            self.context,
            self.context_threshold,
            self.period,
            self.mode,
        )
    }

    /// What one unit costs where a row states a single rate, as media prices do.
    pub(crate) fn amount(&self) -> f64 {
        self.output.or(self.input).unwrap_or(f64::MAX)
    }

    /// The row a table shows first: standard tier, shortest context, full-price hours.
    fn rank(&self) -> (u8, u8, i64, u8, u8) {
        (
            match self.tier {
                Tier::Standard => 0,
                Tier::Flex => 1,
                Tier::Batch => 2,
                Tier::Priority => 3,
            },
            match self.context {
                Band::All | Band::Short => 0,
                Band::Long => 1,
            },
            self.context_threshold.unwrap_or(0),
            match self.period {
                Period::All | Period::Peak => 0,
                Period::OffPeak => 1,
            },
            match self.mode {
                Mode::All => 0,
                Mode::NonThinking => 1,
                Mode::Thinking => 2,
            },
        )
    }
}

pub(crate) fn same_value(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0),
        (None, None) => true,
        _ => false,
    }
}

/// Rows keyed by a tier as two reads of the same page can both describe it: a lone mode or
/// period and a context band without a long tier are what one read writes out and another
/// leaves as `All`.
fn smoothed(prices: &[Price]) -> Vec<(PriceKey<'_>, &Price)> {
    prices
        .iter()
        .map(|price| {
            let tier: Vec<&Price> = prices
                .iter()
                .filter(|other| {
                    other.currency == price.currency
                        && other.unit == price.unit
                        && other.tier == price.tier
                        && other.label == price.label
                })
                .collect();
            let long = tier.iter().any(|other| other.context == Band::Long);
            let (context, threshold) = match price.context {
                Band::Long => (Band::Long, price.context_threshold),
                Band::Short if long => (Band::Short, None),
                _ => (Band::All, None),
            };
            let periods: std::collections::HashSet<Period> = tier
                .iter()
                .filter(|other| other.context == price.context && other.mode == price.mode)
                .map(|other| other.period)
                .collect();
            let modes: std::collections::HashSet<Mode> = tier
                .iter()
                .filter(|other| other.context == price.context && other.period == price.period)
                .map(|other| other.mode)
                .collect();
            let period = if periods.len() == 1 {
                Period::All
            } else {
                price.period
            };
            let mode = if modes.len() == 1 {
                Mode::All
            } else {
                price.mode
            };
            (
                (
                    price.currency,
                    price.unit,
                    price.tier,
                    price.label.as_str(),
                    context,
                    threshold,
                    period,
                    mode,
                ),
                price,
            )
        })
        .collect()
}

/// Rows of two reads that describe the same tier. A tier only one read lists, or one that
/// matches several rows, is left out: reads differ in coverage, and that is not a change.
pub(crate) fn paired<'a>(old: &'a [Price], new: &'a [Price]) -> Vec<(&'a Price, &'a Price)> {
    let (old, new) = (smoothed(old), smoothed(new));
    let only = |rows: &[(PriceKey<'a>, &'a Price)], key: &PriceKey<'_>| {
        let mut found = rows.iter().filter(|(other, _)| other == key);
        match (found.next(), found.next()) {
            (Some((_, price)), None) => Some(*price),
            _ => None,
        }
    };
    old.iter()
        .filter_map(|(key, _)| Some((only(&old, key)?, only(&new, key)?)))
        .collect()
}

/// Whether a value both rows state has moved.
pub(crate) fn row_moved(old: &Price, new: &Price) -> bool {
    [
        (old.input, new.input),
        (old.cached_input, new.cached_input),
        (old.cache_write, new.cache_write),
        (old.cache_write_1h, new.cache_write_1h),
        (old.output, new.output),
    ]
    .into_iter()
    .any(|(a, b)| a.is_some() && b.is_some() && !same_value(a, b))
}

/// The row shown in the catalogue for one currency, in the first of the category's units
/// the model is priced in: the standard token rate of a language or embedding model, or
/// else the cheapest standard size, resolution or request by what it makes. A row pricing
/// only what is sent, such as an image to edit, leads only when no row prices the output.
pub(crate) fn headline(prices: &[Price], currency: Currency, category: Category) -> Option<&Price> {
    let unit = category.units().iter().find(|unit| {
        prices
            .iter()
            .any(|price| price.currency == currency && price.unit == **unit)
    })?;
    let rows = prices
        .iter()
        .filter(|price| price.currency == currency && price.unit == *unit);
    if category.media() || *unit != Unit::Tokens {
        rows.filter(|price| price.tier == Tier::Standard)
            .min_by(|a, b| {
                a.output
                    .is_none()
                    .cmp(&b.output.is_none())
                    .then(a.amount().total_cmp(&b.amount()))
            })
    } else {
        rows.min_by_key(|price| price.rank())
    }
}

/// A model's line and version as its API id writes them: `qwen3.8-max` and `qwen3.7-max`
/// are versions 3.8 and 3.7 of `qwen#-max`, `claude-opus-5-5` is 5.5 of `claude-opus-#`.
/// Dated snapshots, context-size variants and preview builds belong to their line.
#[derive(Debug)]
pub(crate) struct Lineage {
    pub(crate) family: String,
    pub(crate) version: Vec<u32>,
    pub(crate) preview: bool,
    pub(crate) snapshot: bool,
}

pub(crate) fn lineage(model_id: &str) -> Lineage {
    let id = model_id.trim().to_lowercase();
    let mut tokens: Vec<&str> = id
        .split(['-', '_'])
        .filter(|token| !token.is_empty())
        .collect();
    let digits = |token: &str| !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit());
    // Context sizes such as 32k or 128k name a variant, not a line.
    tokens.retain(|token| {
        !(token.len() > 1 && token.ends_with(['k', 'm']) && digits(&token[..token.len() - 1]))
    });
    let mut snapshot = false;
    // Trailing dates: 20260402, 0902 or 2026-05-20.
    if tokens.len() > 3
        && let [.., year, month, day] = tokens[..]
        && year.len() == 4
        && digits(year)
        && month.len() == 2
        && digits(month)
        && day.len() == 2
        && digits(day)
    {
        tokens.truncate(tokens.len() - 3);
        snapshot = true;
    }
    if tokens.len() > 1
        && let Some(last) = tokens.last()
        && matches!(last.len(), 4 | 6 | 8)
        && digits(last)
    {
        tokens.pop();
        snapshot = true;
    }
    let mut preview = false;
    tokens.retain(|token| match *token {
        "preview" | "exp" | "experimental" | "beta" => {
            preview = true;
            false
        }
        "latest" => false,
        _ => true,
    });
    let mut version = Vec::new();
    let mut family = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        if !version.is_empty() || !token.bytes().any(|b| b.is_ascii_digit()) {
            family.push(token.to_owned());
            index += 1;
            continue;
        }
        // The first number is the version: `3.8` in qwen3.8, `4` in v4, `5-5` in claude-opus-5-5.
        let start = token.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
        let end = token[start..]
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .map_or(token.len(), |offset| start + offset);
        version.extend(
            token[start..end]
                .split('.')
                .filter_map(|part| part.parse::<u32>().ok()),
        );
        family.push(format!("{}#{}", &token[..start], &token[end..]));
        index += 1;
        if end == token.len() && start == 0 {
            while index < tokens.len() && digits(tokens[index]) && tokens[index].len() <= 2 {
                version.extend(tokens[index].parse::<u32>().ok());
                index += 1;
            }
        }
    }
    Lineage {
        family: family.join("-"),
        version,
        preview,
        snapshot,
    }
}

/// Newer version first, then a release over a preview, a named model over a dated
/// snapshot, and the shorter id.
type Precedence = (Vec<u32>, bool, bool, std::cmp::Reverse<usize>);

/// Which catalogue models readers see: models on sale, with an official price, and only
/// the newest version of each line, a release ahead of a preview or snapshot of it.
/// `describe` gives a model's vendor, id, status and whether it has prices.
pub(crate) fn current<'a, T>(
    models: &'a [T],
    describe: impl Fn(&'a T) -> (&'a str, &'a str, &'a str, bool),
) -> std::collections::HashSet<(&'a str, &'a str)> {
    let mut best: std::collections::HashMap<(&str, String), (&str, Precedence)> =
        std::collections::HashMap::new();
    for model in models {
        let (vendor, model_id, status, priced) = describe(model);
        if !priced || !matches!(Status::parse(status), Status::Active | Status::Preview) {
            continue;
        }
        let line = lineage(model_id);
        let rank = (
            line.version,
            !line.preview && Status::parse(status) != Status::Preview,
            !line.snapshot,
            std::cmp::Reverse(model_id.len()),
        );
        let entry = best.entry((vendor, line.family));
        match entry {
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if rank > slot.get().1 {
                    slot.insert((model_id, rank));
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert((model_id, rank));
            }
        }
    }
    best.into_iter()
        .map(|((vendor, _), (model_id, _))| (vendor, model_id))
        .collect()
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Links {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) docs: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) announcement: Option<String>,
}

/// A model as verified from one read of its provider's pages.
#[derive(Debug)]
pub(crate) struct Model {
    pub(crate) model_id: String,
    pub(crate) category: Category,
    pub(crate) name: String,
    pub(crate) summary: String,
    pub(crate) summary_en: String,
    pub(crate) status: Status,
    pub(crate) knowledge_cutoff: String,
    pub(crate) context_tokens: Option<i32>,
    pub(crate) max_output_tokens: Option<i32>,
    pub(crate) input_modalities: Vec<String>,
    pub(crate) output_modalities: Vec<String>,
    pub(crate) features: Vec<String>,
    /// `None` when this read could not verify the prices; stored prices then stay.
    pub(crate) prices: Option<Vec<Price>>,
    pub(crate) links: Links,
    pub(crate) evidence: Value,
}

/// Run counters in the terms of the shared run record.
pub(crate) struct Outcome {
    pub(crate) usage: Usage,
    pub(crate) payload: Value,
    /// Providers whose pages were read by the model.
    pub(crate) read: i32,
    pub(crate) changes: i32,
    /// Providers whose pages had not changed.
    pub(crate) unchanged: i32,
    /// Models or price lists that failed verification, plus failed providers.
    pub(crate) rejected: i32,
}

enum Report {
    Unchanged,
    Read {
        models: usize,
        changes: usize,
        rejected: Vec<verify::Rejection>,
    },
    Failed(String),
}

/// Reads every listed provider, then lets providers join and leave. A provider that fails
/// keeps its published catalogue and is retried by the next run; the run itself fails only
/// when no provider could be reached.
pub(crate) async fn execute(state: &AppState, topic: &Topic) -> Result<Outcome> {
    let deadline = Instant::now() + state.config.fetch_timeout.saturating_sub(RUN_MARGIN);
    let adopted = lifecycle::adopt(state, topic)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "failed to register LLM pages from the brief");
            Vec::new()
        });
    let vendors = store::vendors(state, store::Selection::Listed).await?;
    let slots = Arc::new(Semaphore::new(VENDOR_CONCURRENCY));
    let mut jobs = JoinSet::new();
    for vendor in vendors {
        let (state, topic, slots) = (state.clone(), topic.clone(), Arc::clone(&slots));
        jobs.spawn(async move {
            let _slot = slots.acquire_owned().await;
            let mut usage = Usage::default();
            // A provider whose turn comes too late, or whose read outlasts the run, is read
            // by the next run.
            let read = tokio::time::timeout_at(
                deadline,
                refresh(&state, &topic, &vendor, deadline, &mut usage),
            )
            .await
            .unwrap_or_else(|_| Err(crate::error::AppError::Internal("本轮时间不足".into())));
            let report = match read {
                Ok(report) => report,
                Err(error) => {
                    let message = error.to_string();
                    if let Err(error) = store::vendor_failed(&state, &vendor.id, &message).await {
                        tracing::warn!(%error, vendor = %vendor.id, "failed to record LLM provider error");
                    }
                    Report::Failed(message)
                }
            };
            (vendor.id, report, usage)
        });
    }
    let mut outcome = Outcome {
        usage: Usage::default(),
        payload: json!({}),
        read: 0,
        changes: 0,
        unchanged: 0,
        rejected: 0,
    };
    let mut vendors = serde_json::Map::new();
    let mut failures = Vec::new();
    while let Some(joined) = jobs.join_next().await {
        let (id, report, usage) = joined.map_err(|error| {
            crate::error::AppError::Internal(format!("LLM 目录任务异常: {error}").into())
        })?;
        outcome.usage += usage;
        let entry = match report {
            Report::Unchanged => {
                outcome.unchanged += 1;
                json!({"status": "unchanged"})
            }
            Report::Read {
                models,
                changes,
                rejected,
            } => {
                outcome.read += 1;
                outcome.changes += changes as i32;
                outcome.rejected += rejected.len() as i32;
                json!({"status": "read", "models": models, "changes": changes, "rejected": rejected})
            }
            Report::Failed(error) => {
                outcome.rejected += 1;
                failures.push(format!("{id}: {error}"));
                json!({"status": "failed", "error": error})
            }
        };
        vendors.insert(id, entry);
    }
    let total = vendors.len();
    if total > 0 && failures.len() == total {
        return Err(crate::error::AppError::Internal(
            format!("所有服务商都未能核验：{}", failures.join("；")).into(),
        ));
    }
    let pass = lifecycle::run(state, topic, deadline).await;
    outcome.usage += pass.usage;
    outcome.changes += pass.changes;
    outcome.payload = json!({ "vendors": vendors, "adopted": adopted, "lifecycle": pass.payload });
    Ok(outcome)
}

async fn refresh(
    state: &AppState,
    topic: &Topic,
    vendor: &store::Vendor,
    deadline: Instant,
    usage: &mut Usage,
) -> Result<Report> {
    let mut pages = Vec::with_capacity(vendor.sources.len());
    for source in &vendor.sources {
        match source::fetch(state, source, &vendor.domains).await {
            Ok(page) => pages.push(page),
            Err(error) => {
                store::source_failed(state, source.id, &error.to_string()).await?;
                return Err(error);
            }
        }
    }
    let unchanged = vendor.listed > 0
        && vendor
            .sources
            .iter()
            .zip(&pages)
            .all(|(source, page)| source.content_hash.as_deref() == Some(page.hash.as_slice()));
    if unchanged {
        store::confirm_unchanged(state, vendor).await?;
        return Ok(Report::Unchanged);
    }
    let known = store::listed_ids(state, &vendor.id).await?;
    let links = source::merged_links(&pages);
    let extract::Extraction {
        docs_link,
        models: drafts,
    } = extract::read(
        state, topic, vendor, &pages, &links, &known, deadline, usage,
    )
    .await?;
    // Links are official pages by construction; the one picked is the provider's API docs.
    let docs = docs_link
        .and_then(|index| links.get(index))
        .map(|link| link.url.as_str());
    // Official pages always list models; none means the read went wrong, not a withdrawal.
    if drafts.is_empty() {
        return Err(crate::error::AppError::Internal(
            "官方页面没有读到任何模型".into(),
        ));
    }
    let verified = verify::models(drafts, &pages, &links);
    if verified.models.is_empty() {
        return Err(crate::error::AppError::Internal(
            "读到的模型都未通过核验".into(),
        ));
    }
    // A dropped extra tier is not worth reading the pages again; a set-aside model is.
    let complete = verified.rejected.iter().all(|rejection| !rejection.fatal);
    let models = verified.models.len();
    let changes = store::apply(state, vendor, &pages, verified.models, complete, docs).await?;
    Ok(Report::Read {
        models,
        changes,
        rejected: verified.rejected,
    })
}
