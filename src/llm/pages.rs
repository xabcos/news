//! `/llm`: recent changes first, then each provider's models as dense price lists, one per
//! category. `/llm.json` and `/llm/feed.xml` publish the same catalogue for other readers.
use super::{Band, Category, Currency, Links, Mode, Period, Price, Status, Tier, Unit, headline};
use crate::{
    digest::{Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{Chrome, absolute, choose, escape, localized},
    state::AppState,
};
use axum::{
    Router,
    extract::{Query, State},
    http::HeaderMap,
    middleware,
    response::Response,
    routing::get,
};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{FromRow, types::Json};
use std::{collections::HashSet, fmt::Write};

const CHANGES_SHOWN: i64 = 20;
const FEED_SIZE: i64 = 50;
const CHROME: Chrome<'static> = Chrome {
    current: "llm",
    section: "llm",
    stylesheet: "/llm.css?v=5",
    script: "/llm.js?v=5",
    feed: ("𝑳𝑳𝑴", "/llm/feed.xml"),
};

pub(crate) fn router() -> Router<AppState> {
    let data = Router::new().route("/llm.json", get(data));
    let public = reading::public(data.clone())
        .route("/llm/feed.xml", get(feed))
        .layer(middleware::from_fn(crate::digest::public_cors));
    reading::pages(Router::new().route("/llm", get(index)))
        .merge(public)
        .merge(reading::admin(data))
        .layer(tower_http::compression::CompressionLayer::new())
}

#[derive(Deserialize)]
struct Params {
    lang: Option<String>,
}

#[derive(Clone)]
struct VendorRow {
    id: String,
    name: String,
    name_en: String,
    region: String,
    verified_at: Option<DateTime<Utc>>,
    pricing: Vec<String>,
    docs: Option<String>,
}

#[derive(FromRow)]
struct ModelRow {
    vendor_id: String,
    model_id: String,
    category: String,
    name: String,
    summary: String,
    summary_en: String,
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
    first_seen_at: DateTime<Utc>,
    verified_at: DateTime<Utc>,
    changed_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct ChangeRow {
    id: i64,
    vendor_id: String,
    /// `None` for a provider joining or leaving the catalogue.
    model_id: Option<String>,
    name: Option<String>,
    category: Option<String>,
    kind: String,
    before: Option<Json<Value>>,
    after: Option<Json<Value>>,
    source_url: String,
    detected_at: DateTime<Utc>,
}

struct Catalog {
    /// Providers currently listed, in display order.
    vendors: Vec<VendorRow>,
    /// Every enabled provider, including those that left, for naming changes.
    known: Vec<VendorRow>,
    models: Vec<ModelRow>,
    changes: Vec<ChangeRow>,
}

impl Catalog {
    fn vendor(&self, id: &str) -> Option<&VendorRow> {
        self.known.iter().find(|vendor| vendor.id == id)
    }

    fn verified_at(&self) -> Option<DateTime<Utc>> {
        self.vendors
            .iter()
            .filter_map(|vendor| vendor.verified_at)
            .max()
    }
}

/// Every kept change (they last a day); changes to models readers no longer see are
/// dropped after the catalogue has chosen what it lists.
const RECENT_CHANGES: &str = "SELECT c.id, c.vendor_id, c.model_id, m.name, m.category, c.kind, c.before, c.after, c.source_url, c.detected_at
     FROM llm_changes c JOIN llm_vendors v ON v.id = c.vendor_id
     LEFT JOIN llm_models m ON m.vendor_id = c.vendor_id AND m.model_id = c.model_id
     WHERE v.enabled AND (c.model_id IS NULL OR NOT m.hidden)
     ORDER BY c.detected_at DESC, c.id DESC";

/// The published catalogue, or `None` while the topic is switched off or hidden.
async fn load(state: &AppState, changes: i64, reader: Reader) -> Result<Option<Catalog>> {
    if reader.check(state, super::TOPIC_ID).is_err() {
        return Ok(None);
    }
    let known: Vec<(VendorRow, String)> = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            Option<DateTime<Utc>>,
            Vec<String>,
            Option<String>,
            String,
        ),
    >(
        "SELECT v.id, v.name, v.name_en, v.region, v.verified_at,
                COALESCE(array_agg(s.url ORDER BY s.sort_weight, s.id)
                    FILTER (WHERE s.kind = 'pricing'), '{}') AS pricing, v.docs, v.state
         FROM llm_vendors v LEFT JOIN llm_sources s ON s.vendor_id = v.id
         WHERE v.enabled GROUP BY v.id ORDER BY v.sort_weight, v.id",
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(
        |(id, name, name_en, region, verified_at, pricing, docs, state)| {
            (
                VendorRow {
                    id,
                    name,
                    name_en,
                    region,
                    verified_at,
                    pricing,
                    docs,
                },
                state,
            )
        },
    )
    .collect();
    let mut models: Vec<ModelRow> = sqlx::query_as(
        "SELECT m.vendor_id, m.model_id, m.category, m.name, m.summary, m.summary_en, m.status,
                m.knowledge_cutoff, m.context_tokens, m.max_output_tokens, m.input_modalities,
                m.output_modalities, m.features, m.prices, m.links, m.evidence, m.first_seen_at,
                m.verified_at, m.changed_at
         FROM llm_models m JOIN llm_vendors v ON v.id = m.vendor_id
         WHERE v.enabled AND v.state = 'active' AND NOT m.hidden AND m.removed_at IS NULL
         ORDER BY v.sort_weight, v.id, m.position, m.model_id",
    )
    .fetch_all(&state.pool)
    .await?;
    let shown: HashSet<(String, String)> = super::current(&models, |model| {
        (
            model.vendor_id.as_str(),
            model.model_id.as_str(),
            model.status.as_str(),
            !model.prices.0.is_empty(),
        )
    })
    .into_iter()
    .map(|(vendor, model)| (vendor.to_owned(), model.to_owned()))
    .collect();
    models.retain(|model| shown.contains(&(model.vendor_id.clone(), model.model_id.clone())));
    let mut recent: Vec<ChangeRow> = sqlx::query_as(RECENT_CHANGES)
        .fetch_all(&state.pool)
        .await?;
    // Lines readers see, for telling a line that ended from an old version being retired.
    let lines: HashSet<(String, String)> = models
        .iter()
        .map(|model| {
            (
                model.vendor_id.clone(),
                super::lineage(&model.model_id).family,
            )
        })
        .collect();
    recent.retain(|change| {
        let Some(model) = &change.model_id else {
            return true;
        };
        let listed = shown.contains(&(change.vendor_id.clone(), model.clone()));
        let ended = !lines.contains(&(change.vendor_id.clone(), super::lineage(model).family));
        match change.kind.as_str() {
            "removed" => ended,
            "status" => listed || (ended && deprecated(change)),
            _ => listed,
        }
    });
    recent.truncate(usize::try_from(changes).unwrap_or(usize::MAX));
    let vendors = known
        .iter()
        .filter(|(vendor, state)| {
            state == "active" && models.iter().any(|model| model.vendor_id == vendor.id)
        })
        .map(|(vendor, _)| vendor.clone())
        .collect();
    Ok(Some(Catalog {
        vendors,
        known: known.into_iter().map(|(vendor, _)| vendor).collect(),
        models,
        changes: recent,
    }))
}

fn deprecated(change: &ChangeRow) -> bool {
    change
        .after
        .as_ref()
        .is_some_and(|after| after.0["status"] == "deprecated")
}

fn not_found() -> AppError {
    AppError::NotFound("主题不存在".into())
}

/// The currency a provider is listed in: renminbi for mainland providers when published.
pub(super) fn primary(region: &str, prices: &[Price]) -> Currency {
    let has = |currency| prices.iter().any(|price| price.currency == currency);
    match (region == "cn", has(Currency::Cny), has(Currency::Usd)) {
        (true, true, _) | (false, true, false) => Currency::Cny,
        _ => Currency::Usd,
    }
}

fn trimmed(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// Prices keep at least two decimals and at most six, as official tables write them.
pub(super) fn money(currency: Currency, value: f64, en: bool) -> String {
    if value == 0.0 {
        return choose(en, "免费", "Free").to_owned();
    }
    let mut text = format!("{value:.6}");
    while text.ends_with('0') && text.len() - text.find('.').unwrap_or(text.len()) > 3 {
        text.pop();
    }
    format!("{}{text}", currency.symbol())
}

fn price_cell(currency: Currency, value: Option<f64>, en: bool) -> String {
    value.map_or_else(|| "—".to_owned(), |value| money(currency, value, en))
}

/// What a price in this unit is for, as a table caption says it.
fn per(unit: Unit, en: bool) -> &'static str {
    match unit {
        Unit::Tokens => choose(en, "每百万 tokens", "per 1M tokens"),
        Unit::Characters => choose(en, "每百万字符", "per 1M characters"),
        Unit::Image => choose(en, "每张", "per image"),
        Unit::Second => choose(en, "每秒", "per second"),
        Unit::Minute => choose(en, "每分钟", "per minute"),
        Unit::Hour => choose(en, "每小时", "per hour"),
        Unit::Request => choose(en, "每次", "per request"),
    }
}

/// A price with its unit.
fn priced(currency: Currency, value: f64, unit: Unit, en: bool) -> String {
    let unit = match unit {
        Unit::Tokens => choose(en, "百万 tokens", "1M tokens"),
        Unit::Characters => choose(en, "百万字符", "1M chars"),
        Unit::Image => choose(en, "张", "image"),
        Unit::Second => choose(en, "秒", "s"),
        Unit::Minute => choose(en, "分钟", "min"),
        Unit::Hour => choose(en, "小时", "h"),
        Unit::Request => choose(en, "次", "request"),
    };
    format!("{}/{unit}", money(currency, value, en))
}

/// A price as the lists show it: a language or embedding model's token rate bare, as the
/// page says once that those are per 1M tokens, and anything else with its unit.
fn shown(currency: Currency, value: f64, unit: Unit, category: Category, en: bool) -> String {
    if unit == Unit::Tokens && !category.media() {
        money(currency, value, en)
    } else {
        priced(currency, value, unit, en)
    }
}

/// A model's headline price in its unit, marked as a starting price when it is the cheapest
/// standard size, resolution or rate and another costs more.
pub(super) fn starting(
    prices: &[Price],
    currency: Currency,
    category: Category,
    en: bool,
) -> String {
    let Some(row) = headline(prices, currency, category) else {
        return "—".to_owned();
    };
    let price = shown(currency, row.amount(), row.unit, category, en);
    let others: Vec<f64> = prices
        .iter()
        .filter(|other| {
            other.currency == currency
                && other.unit == row.unit
                && other.tier == Tier::Standard
                && other.output.is_some() == row.output.is_some()
                && other.makes(category) == row.makes(category)
        })
        .map(Price::amount)
        .collect();
    let more = others.iter().any(|other| *other > row.amount() + 1e-9)
        && others.iter().all(|other| *other >= row.amount() - 1e-9);
    match (more, en) {
        (false, _) => price,
        (true, false) => format!("{price}起"),
        (true, true) => format!("from {price}"),
    }
}

fn category_label(category: Category, en: bool) -> &'static str {
    match category {
        Category::Text => choose(en, "语言模型", "Language"),
        Category::Image => choose(en, "图像", "Image"),
        Category::Video => choose(en, "视频", "Video"),
        Category::Audio => choose(en, "语音", "Audio"),
        Category::Embedding => choose(en, "嵌入与重排", "Embedding and rerank"),
    }
}

pub(super) fn tokens(count: i32) -> String {
    let count = i64::from(count);
    if count >= 1_000_000 {
        if count % 1_048_576 == 0 {
            format!("{}M", count / 1_048_576)
        } else {
            format!("{}M", trimmed(count as f64 / 1e6, 2))
        }
    } else if count >= 1000 {
        if count % 1024 == 0 && count % 1000 != 0 {
            format!("{}K", count / 1024)
        } else {
            format!("{}K", trimmed(count as f64 / 1000.0, 1))
        }
    } else {
        count.to_string()
    }
}

fn day(at: DateTime<Utc>, timezone: Tz) -> String {
    at.with_timezone(&timezone).format("%m-%d").to_string()
}

fn moment(at: DateTime<Utc>, timezone: Tz) -> String {
    at.with_timezone(&timezone)
        .format("%m-%d %H:%M")
        .to_string()
}

fn status_label(status: Status, en: bool) -> Option<&'static str> {
    match status {
        Status::Active => None,
        Status::Preview => Some(choose(en, "预览", "Preview")),
        Status::Legacy => Some(choose(en, "旧版", "Legacy")),
        Status::Deprecated => Some(choose(en, "弃用", "Deprecated")),
    }
}

/// Where a short-context row ends: the first long tier of the same row.
fn short_limit(price: &Price, rows: &[&Price]) -> Option<i64> {
    rows.iter()
        .filter(|other| {
            other.context == Band::Long
                && other.currency == price.currency
                && other.tier == price.tier
                && other.label == price.label
                && other.period == price.period
                && other.mode == price.mode
        })
        .filter_map(|other| other.context_threshold)
        .min()
}

fn threshold(tokens_at: i64) -> String {
    tokens(i32::try_from(tokens_at).unwrap_or(i32::MAX))
}

fn row_label(price: &Price, rows: &[&Price], en: bool) -> String {
    let mut parts = Vec::new();
    parts.push(if price.label.is_empty() {
        match price.tier {
            Tier::Standard => choose(en, "标准", "Standard").to_owned(),
            Tier::Batch => "Batch".to_owned(),
            Tier::Flex => "Flex".to_owned(),
            Tier::Priority => choose(en, "优先", "Priority").to_owned(),
        }
    } else {
        price.label.clone()
    });
    match price.context {
        Band::All => {}
        Band::Short => parts.push(match short_limit(price, rows) {
            Some(limit) => format!("≤{}", threshold(limit)),
            None => choose(en, "短上下文", "Short context").to_owned(),
        }),
        Band::Long => parts.push(match price.context_threshold {
            Some(start) => format!(">{}", threshold(start)),
            None => choose(en, "长上下文", "Long context").to_owned(),
        }),
    }
    match price.period {
        Period::All => {}
        Period::Peak => parts.push(choose(en, "高峰", "Peak").to_owned()),
        Period::OffPeak => parts.push(choose(en, "空闲", "Off-peak").to_owned()),
    }
    match price.mode {
        Mode::All => {}
        Mode::Thinking => parts.push(choose(en, "思考", "Thinking").to_owned()),
        Mode::NonThinking => parts.push(choose(en, "非思考", "Non-thinking").to_owned()),
    }
    parts.join(" · ")
}

/// Pricing structure worth knowing before opening a row.
fn flags(prices: &[Price], en: bool) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if prices.iter().any(|price| price.context == Band::Long) {
        flags.push(choose(en, "长上下文另计", "Long-context rate"));
    }
    if prices.iter().any(|price| price.period == Period::OffPeak) {
        flags.push(choose(en, "峰谷计价", "Peak/off-peak"));
    }
    if prices.iter().any(|price| price.mode == Mode::Thinking) {
        flags.push(choose(en, "思考另计", "Thinking rate"));
    }
    flags
}

fn words(values: &[String], en: bool) -> String {
    values
        .iter()
        .map(|value| match value.as_str() {
            "text" => choose(en, "文本", "Text"),
            "image" => choose(en, "图像", "Image"),
            "audio" => choose(en, "音频", "Audio"),
            "video" => choose(en, "视频", "Video"),
            "pdf" => "PDF",
            "reasoning" => choose(en, "推理", "Reasoning"),
            "tools" => choose(en, "工具调用", "Tool use"),
            "structured_output" => choose(en, "结构化输出", "Structured output"),
            "web_search" => choose(en, "联网搜索", "Web search"),
            "open_weights" => choose(en, "开放权重", "Open weights"),
            _ => "",
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(choose(en, "、", ", "))
}

/// A model's id on the page. An id holds no spaces, so a model listed by its name writes
/// them percent-encoded, as it does `%` itself; API ids are unchanged.
pub(super) fn anchor(vendor: &str, model: &str) -> String {
    format!("{vendor}:{}", model.replace('%', "%25").replace(' ', "%20"))
}

/// The address fragment that opens a model's row: its id, percent-encoded once more.
pub(super) fn fragment(vendor: &str, model: &str) -> String {
    anchor(vendor, model).replace('%', "%25")
}

fn percent(before: f64, after: f64) -> String {
    if before == 0.0 {
        return String::new();
    }
    let change = (after - before) / before * 100.0;
    format!(
        "（{}{}%）",
        if change > 0.0 { "+" } else { "" },
        trimmed(change, 0)
    )
}

fn prices_of(value: Option<&Json<Value>>) -> Vec<Price> {
    value
        .and_then(|value| serde_json::from_value(value.0["prices"].clone()).ok())
        .unwrap_or_default()
}

fn count_of(value: Option<&Json<Value>>, key: &str) -> Option<i32> {
    value
        .and_then(|value| value.0[key].as_i64())
        .and_then(|value| i32::try_from(value).ok())
}

fn summary_line(
    prices: &[Price],
    currency: Currency,
    category: Category,
    context: Option<i32>,
    en: bool,
) -> String {
    let mut parts = Vec::new();
    match headline(prices, currency, category) {
        Some(row) if row.unit == Unit::Tokens => {
            if let Some(input) = row.input {
                parts.push(format!(
                    "{} {}",
                    choose(en, "输入", "Input"),
                    priced(currency, input, row.unit, en)
                ));
            }
            if let Some(output) = row.output {
                parts.push(format!(
                    "{} {}",
                    choose(en, "输出", "Output"),
                    priced(currency, output, row.unit, en)
                ));
            }
        }
        Some(_) => parts.push(starting(prices, currency, category, en)),
        None => {}
    }
    if let Some(context) = context {
        parts.push(tokens(context));
    }
    parts.join(" · ")
}

/// A change in one line: what moved, from what, to what. Prices carry their unit, as the
/// feed has no caption to state it.
fn describe(change: &ChangeRow, region: &str, en: bool) -> (&'static str, String) {
    let before = prices_of(change.before.as_ref());
    let after = prices_of(change.after.as_ref());
    let category = Category::parse(change.category.as_deref().unwrap_or(""));
    match change.kind.as_str() {
        "added" => {
            let currency = primary(region, &after);
            (
                choose(en, "新增", "New"),
                summary_line(
                    &after,
                    currency,
                    category,
                    count_of(change.after.as_ref(), "context_tokens"),
                    en,
                ),
            )
        }
        "price" => {
            // The rate readers compare first, else the first tier that moved.
            let currency = primary(region, &after);
            let mut moved: Vec<(&Price, &Price)> = headline(&before, currency, category)
                .zip(headline(&after, currency, category))
                .filter(|(old, new)| old.key() == new.key() && super::row_moved(old, new))
                .into_iter()
                .collect();
            if moved.is_empty() {
                moved = super::paired(&before, &after)
                    .into_iter()
                    .filter(|(old, new)| super::row_moved(old, new))
                    .take(1)
                    .collect();
            }
            let rows: Vec<&Price> = after.iter().collect();
            let mut parts = Vec::new();
            for (old, new) in moved {
                let standard =
                    headline(&after, currency, category).is_some_and(|row| std::ptr::eq(row, new));
                let tier = if standard {
                    String::new()
                } else {
                    format!("{} ", row_label(new, &rows, en))
                };
                for (label, a, b) in [
                    (choose(en, "输入", "Input"), old.input, new.input),
                    (choose(en, "输出", "Output"), old.output, new.output),
                    (
                        choose(en, "缓存命中", "Cached input"),
                        old.cached_input,
                        new.cached_input,
                    ),
                ] {
                    if let (Some(a), Some(b)) = (a, b)
                        && !super::same_value(Some(a), Some(b))
                    {
                        parts.push(format!(
                            "{tier}{label} {} → {}{}",
                            money(new.currency, a, en),
                            priced(new.currency, b, new.unit, en),
                            if en { String::new() } else { percent(a, b) }
                        ));
                    }
                }
            }
            if parts.is_empty() {
                parts
                    .push(choose(en, "其他计价档位调整", "Other pricing tiers changed").to_owned());
            }
            (choose(en, "调价", "Price"), parts.join(" · "))
        }
        "spec" => {
            let mut parts = Vec::new();
            for (label, key) in [
                (choose(en, "上下文", "Context"), "context_tokens"),
                (choose(en, "最大输出", "Max output"), "max_output_tokens"),
            ] {
                let (a, b) = (
                    count_of(change.before.as_ref(), key),
                    count_of(change.after.as_ref(), key),
                );
                if a != b {
                    parts.push(format!(
                        "{label} {} → {}",
                        a.map_or_else(|| "—".to_owned(), tokens),
                        b.map_or_else(|| "—".to_owned(), tokens)
                    ));
                }
            }
            (choose(en, "规格", "Specs"), parts.join(" · "))
        }
        "status" if deprecated(change) => (
            choose(en, "弃用", "Deprecated"),
            choose(en, "官方宣布弃用", "Deprecated by the provider").to_owned(),
        ),
        "status" => {
            let label = |value: Option<&Json<Value>>| {
                let status =
                    Status::parse(value.and_then(|v| v.0["status"].as_str()).unwrap_or(""));
                status_label(status, en)
                    .unwrap_or(choose(en, "正式", "Generally available"))
                    .to_owned()
            };
            (
                choose(en, "状态", "Status"),
                format!(
                    "{} → {}",
                    label(change.before.as_ref()),
                    label(change.after.as_ref())
                ),
            )
        }
        "vendor_added" => {
            let count = change
                .after
                .as_ref()
                .and_then(|after| after.0["models"].as_u64())
                .unwrap_or(0);
            let returned = change
                .before
                .as_ref()
                .is_some_and(|before| before.0["state"] == "offline");
            (
                choose(en, "上线", "Listed"),
                match (returned, en) {
                    (false, false) => format!("新增服务商 · 收录 {count} 个模型"),
                    (true, false) => format!("恢复收录 · {count} 个模型"),
                    (false, true) => format!("New provider · {count} models"),
                    (true, true) => format!("Tracked again · {count} models"),
                },
            )
        }
        "vendor_removed" => (
            choose(en, "下线", "Retired"),
            choose(
                en,
                "官方页面连续两周无法核验，已停止收录",
                "Official pages could not be verified for two weeks; no longer tracked",
            )
            .to_owned(),
        ),
        _ => (
            choose(en, "下线", "Removed"),
            choose(en, "已从官方页面移除", "No longer listed on official pages").to_owned(),
        ),
    }
}

async fn index(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.lang.as_deref() == Some("en");
    let catalog = load(&state, CHANGES_SHOWN, reader)
        .await?
        .ok_or_else(not_found)?;
    let timezone = state.config.timezone;
    let description = choose(
        en,
        "全球主流大模型价格与更新，以各服务商官方文档为准",
        "Prices and updates of mainstream AI models, from each provider's own documentation",
    );
    let mut content = format!(
        "<header class=llm-heading><h1>{}</h1><p>{}</p></header>",
        choose(en, "大模型", "𝑳𝑳𝑴"),
        choose(
            en,
            "全球主流大模型价格与更新",
            "Mainstream model prices and updates"
        )
    );
    let mut meta =
        vec![choose(en, "以各服务商官方文档为准", "From official provider docs").to_owned()];
    if let Some(at) = catalog.verified_at() {
        meta.push(format!(
            "{} <time datetime=\"{}\">{}</time>",
            choose(en, "核验于", "Checked"),
            at.to_rfc3339(),
            moment(at, timezone)
        ));
    }
    meta.push(
        choose(
            en,
            "语言与嵌入模型的 token 价格为每百万 tokens",
            "Language and embedding token prices per 1M tokens",
        )
        .to_owned(),
    );
    let _ = write!(content, "<p class=llm-meta>{}</p>", meta.join(" · "));

    // Changes are kept for a day; without any the page starts with the catalogue.
    if !catalog.changes.is_empty() {
        content.push_str("<section class=llm-changes aria-labelledby=llm-changes-title>");
        let _ = write!(
            content,
            "<h2 id=llm-changes-title>{}</h2>",
            choose(en, "最近变化", "Recent changes")
        );
        content.push_str("<ol class=llm-change-list>");
        for change in &catalog.changes {
            let Some(vendor) = catalog.vendor(&change.vendor_id) else {
                continue;
            };
            let (kind, detail) = describe(change, &vendor.region, en);
            let model = change.model_id.as_deref();
            let listed = model.is_some_and(|model| {
                catalog
                    .models
                    .iter()
                    .any(|row| row.vendor_id == change.vendor_id && row.model_id == model)
            });
            let target = match (listed, model) {
                (true, Some(model)) => format!(
                    "<a class=llm-change-model href=\"#{}\">",
                    escape(&fragment(&change.vendor_id, model))
                ),
                _ => "<span class=llm-change-model>".to_owned(),
            };
            let _ = write!(
                content,
                "<li class=\"llm-change llm-change-{kind_class}\"><time datetime=\"{at}\">{day}</time><span class=llm-change-kind>{kind}</span>{target}<span>{vendor}</span>{model}{close}<span class=llm-change-detail>{detail}</span>{source}</li>",
                kind_class = escape(&change.kind),
                at = change.detected_at.to_rfc3339(),
                day = day(change.detected_at, timezone),
                vendor = escape(choose(en, &vendor.name, &vendor.name_en)),
                model = model.map_or_else(String::new, |model| format!(
                    " <code>{}</code>",
                    escape(model)
                )),
                close = if listed { "</a>" } else { "</span>" },
                detail = escape(&detail),
                source = if change.source_url.is_empty() {
                    String::new()
                } else {
                    format!(
                        "<a class=llm-source href=\"{}\" rel=\"noopener noreferrer\">{} ↗</a>",
                        escape(&change.source_url),
                        choose(en, "来源", "Source")
                    )
                },
            );
        }
        content.push_str("</ol></section>");
    }

    let button = |kind: &str, value: &str, label: &str, pressed: bool| {
        format!(
            "<button type=button class=\"button button-quiet button-small\" data-{kind}={value} aria-pressed={pressed}>{label}</button>"
        )
    };
    let regions = [
        ("all", choose(en, "全部", "All")),
        ("global", choose(en, "海外", "Global")),
        ("cn", choose(en, "国内", "China")),
    ]
    .map(|(value, label)| button("region", value, label, value == "all"))
    .concat();
    let listed: Vec<Category> = Category::ALL
        .into_iter()
        .filter(|category| {
            catalog
                .models
                .iter()
                .any(|model| model.category == category.as_str())
        })
        .collect();
    // A choice of categories only where there is more than one to choose from.
    let categories = if listed.len() > 1 {
        let buttons: String =
            std::iter::once(button("category", "all", choose(en, "全部", "All"), true))
                .chain(listed.iter().map(|category| {
                    button(
                        "category",
                        category.as_str(),
                        category_label(*category, en),
                        false,
                    )
                }))
                .collect();
        format!(
            "<div class=llm-filter role=group aria-label=\"{}\">{buttons}</div>",
            choose(en, "类别", "Category")
        )
    } else {
        String::new()
    };
    let _ = write!(
        content,
        "<div class=llm-toolbar hidden><div class=llm-filters><div class=llm-filter role=group aria-label=\"{}\">{regions}</div>{categories}</div><input class=llm-search type=search autocomplete=off spellcheck=false placeholder=\"{}\" aria-label=\"{}\"></div>",
        choose(en, "地区", "Region"),
        choose(en, "搜索模型", "Search models"),
        choose(en, "搜索模型", "Search models"),
    );
    if catalog.models.is_empty() {
        let _ = write!(
            content,
            "<p class=llm-empty>{}</p>",
            choose(
                en,
                "正在从各服务商官方文档读取首批模型与价格，完成后显示在这里。",
                "The first models and prices are being read from official provider docs and will appear here."
            )
        );
    }
    content.push_str("<div class=llm-catalog>");
    for vendor in &catalog.vendors {
        let models: Vec<&ModelRow> = catalog
            .models
            .iter()
            .filter(|model| model.vendor_id == vendor.id)
            .collect();
        if models.is_empty() {
            continue;
        }
        let link = |url: &str, label: &str| {
            format!(
                " · <a href=\"{}\" rel=\"noopener noreferrer\">{label} ↗</a>",
                escape(url)
            )
        };
        let mut meta = String::from(if vendor.region == "cn" {
            choose(en, "国内", "China")
        } else {
            choose(en, "海外", "Global")
        });
        if let Some(url) = vendor.pricing.first() {
            meta.push_str(&link(url, choose(en, "定价页", "Pricing")));
        }
        if let Some(url) = &vendor.docs {
            meta.push_str(&link(url, choose(en, "文档", "Docs")));
        }
        let _ = write!(
            content,
            "<section class=llm-vendor id=\"{id}\" data-region=\"{region}\"><header class=llm-vendor-heading><h2>{name}</h2><span class=llm-vendor-meta>{meta}</span></header>",
            id = escape(&vendor.id),
            region = escape(&vendor.region),
            name = escape(choose(en, &vendor.name, &vendor.name_en)),
        );
        for category in Category::ALL {
            let rows: Vec<&&ModelRow> = models
                .iter()
                .filter(|model| model.category == category.as_str())
                .collect();
            if rows.is_empty() {
                continue;
            }
            let _ = write!(
                content,
                "<div class=\"llm-group llm-group-{kind}\" data-category={kind}><h3 class=llm-group-title>{label}</h3><div class=llm-columns aria-hidden=true>{columns}</div>",
                kind = category.as_str(),
                label = category_label(category, en),
                columns = columns(category, en)
                    .iter()
                    .map(|column| format!("<span>{column}</span>"))
                    .collect::<String>(),
            );
            for model in rows {
                content.push_str(&model_row(model, category, vendor, timezone, en));
            }
            content.push_str("</div>");
        }
        content.push_str("</section>");
    }
    content.push_str("</div>");
    let _ = write!(
        content,
        "<p class=llm-footnote>{} <a href=\"/llm.json\">JSON</a> · <a href=\"/llm/feed.xml\">Atom</a></p>",
        choose(
            en,
            "价格、规格与链接均取自各服务商的官方文档。",
            "Prices, specs and links come from each provider's official documentation."
        )
    );
    let structured = json!({
        "@context": "https://schema.org", "@type": "Dataset",
        "name": "𝑳𝑳𝑴", "description": description,
        "url": absolute(&state, &localized("/llm", en)),
        "isAccessibleForFree": true,
        "dateModified": catalog.verified_at().map(|at| at.to_rfc3339()),
        "distribution": [{"@type": "DataDownload", "encodingFormat": "application/json", "contentUrl": absolute(&state, "/llm.json")}],
    });
    let html = crate::site::layout(
        &state,
        reader,
        en,
        "/llm",
        &format!(
            "{} · {}",
            choose(en, "大模型", "𝑳𝑳𝑴"),
            choose(en, "模型价格与更新", "Model prices")
        ),
        description,
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

/// The headings of a category's list; a language model's rates are per 1M tokens, the
/// others carry their unit in the cell.
fn columns(category: Category, en: bool) -> Vec<&'static str> {
    let model = choose(en, "模型", "Model");
    match category {
        Category::Text => vec![
            model,
            choose(en, "上下文", "Context"),
            choose(en, "输入", "Input"),
            choose(en, "输出", "Output"),
            choose(en, "缓存命中", "Cached"),
        ],
        Category::Embedding => vec![
            model,
            choose(en, "上下文", "Context"),
            choose(en, "输入", "Input"),
        ],
        Category::Image | Category::Video | Category::Audio => {
            vec![model, choose(en, "价格", "Price")]
        }
    }
}

fn model_row(
    model: &ModelRow,
    category: Category,
    vendor: &VendorRow,
    timezone: Tz,
    en: bool,
) -> String {
    let prices = &model.prices.0;
    let currency = primary(&vendor.region, prices);
    let row = headline(prices, currency, category);
    let status = Status::parse(&model.status);
    let mut badges = String::new();
    if let Some(label) = status_label(status, en) {
        let _ = write!(badges, "<span class=llm-badge>{label}</span>");
    }
    for flag in flags(prices, en) {
        let _ = write!(badges, "<span class=llm-flag>{flag}</span>");
    }
    let search = format!(
        "{} {} {} {}",
        model.model_id, model.name, vendor.name, vendor.name_en
    )
    .to_lowercase();
    let cell = |label: &str, value: String| {
        format!("<span class=llm-num data-label=\"{label}\">{value}</span>")
    };
    let context = || {
        cell(
            choose(en, "上下文", "Context"),
            model.context_tokens.map_or_else(|| "—".to_owned(), tokens),
        )
    };
    let cells = match category {
        Category::Text => [
            context(),
            cell(
                choose(en, "输入", "Input"),
                price_cell(currency, row.and_then(|row| row.input), en),
            ),
            cell(
                choose(en, "输出", "Output"),
                price_cell(currency, row.and_then(|row| row.output), en),
            ),
            cell(
                choose(en, "缓存命中", "Cached"),
                price_cell(currency, row.and_then(|row| row.cached_input), en),
            ),
        ]
        .concat(),
        Category::Embedding => [
            context(),
            cell(
                choose(en, "输入", "Input"),
                starting(prices, currency, category, en),
            ),
        ]
        .concat(),
        Category::Image | Category::Video | Category::Audio => cell(
            choose(en, "价格", "Price"),
            starting(prices, currency, category, en),
        ),
    };
    let mut html = format!(
        "<details class=llm-model id=\"{id}\" data-search=\"{search}\"><summary><span class=llm-model-name><code>{model_id}</code>{badges}{name}</span>{cells}</summary><div class=llm-detail>",
        id = escape(&anchor(&model.vendor_id, &model.model_id)),
        search = escape(&search),
        model_id = escape(&model.model_id),
        // A model listed by its name has nothing more to show under it.
        name = if model.name.eq_ignore_ascii_case(&model.model_id) {
            String::new()
        } else {
            format!("<small>{}</small>", escape(&model.name))
        },
    );
    let summary = choose(en, &model.summary, &model.summary_en);
    if !summary.is_empty() {
        let _ = write!(html, "<p class=llm-summary>{}</p>", escape(summary));
    }
    // One table per currency and unit, the category's usual unit first.
    let mut units: Vec<Unit> = category.units().to_vec();
    for price in prices {
        if !units.contains(&price.unit) {
            units.push(price.unit);
        }
    }
    for currency in [Currency::Cny, Currency::Usd] {
        for unit in &units {
            let mut rows: Vec<&Price> = prices
                .iter()
                .filter(|price| price.currency == currency && price.unit == *unit)
                .collect();
            if rows.is_empty() {
                continue;
            }
            rows.sort_by_key(|price| price.rank());
            html.push_str(&price_table(currency, *unit, &rows, en));
        }
    }
    let mut specs = Vec::new();
    let mut spec = |label: &str, value: String| {
        if !value.is_empty() {
            specs.push(format!(
                "<div><dt>{}</dt><dd>{}</dd></div>",
                label,
                escape(&value)
            ));
        }
    };
    spec(
        choose(en, "上下文", "Context"),
        model.context_tokens.map(tokens).unwrap_or_default(),
    );
    spec(
        choose(en, "最大输出", "Max output"),
        model.max_output_tokens.map(tokens).unwrap_or_default(),
    );
    spec(
        choose(en, "知识截止", "Knowledge cutoff"),
        model.knowledge_cutoff.clone(),
    );
    spec(
        choose(en, "输入类型", "Input"),
        words(&model.input_modalities, en),
    );
    spec(
        choose(en, "输出类型", "Output"),
        words(&model.output_modalities, en),
    );
    spec(
        choose(en, "能力", "Capabilities"),
        words(&model.features, en),
    );
    if !specs.is_empty() {
        let _ = write!(html, "<dl class=llm-specs>{}</dl>", specs.join(""));
    }
    let price_source = model.evidence.0["prices"][0]["url"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| vendor.pricing.first().cloned());
    let mut links = Vec::new();
    for (label, url) in [
        (choose(en, "官方文档", "Docs"), model.links.0.docs.as_ref()),
        (
            choose(en, "发布公告", "Announcement"),
            model.links.0.announcement.as_ref(),
        ),
        (choose(en, "定价页", "Pricing"), price_source.as_ref()),
    ] {
        if let Some(url) = url {
            links.push(format!(
                "<a href=\"{}\" rel=\"noopener noreferrer\">{label} ↗</a>",
                escape(url)
            ));
        }
    }
    if !links.is_empty() {
        let _ = write!(html, "<p class=llm-links>{}</p>", links.join(""));
    }
    let _ = write!(
        html,
        "<p class=llm-provenance>{} <time datetime=\"{}\">{}</time> · {} {}</p></div></details>",
        choose(en, "核验于", "Checked"),
        model.verified_at.to_rfc3339(),
        moment(model.verified_at, timezone),
        choose(en, "首次收录", "First listed"),
        model
            .first_seen_at
            .with_timezone(&timezone)
            .format("%Y-%m-%d"),
    );
    html
}

/// A price table's column: its heading and the rate it shows.
type Column = (&'static str, fn(&Price) -> Option<f64>);

/// A model's rows in one currency and unit. Token rates keep their cache columns; other
/// units state one rate a row, or what is sent and what comes back.
fn price_table(currency: Currency, unit: Unit, rows: &[&Price], en: bool) -> String {
    let caption = format!(
        "{} · {}",
        match currency {
            Currency::Cny => choose(en, "人民币", "CNY"),
            Currency::Usd => choose(en, "美元", "USD"),
        },
        per(unit, en)
    );
    let mut html = format!(
        "<div class=llm-prices-wrap><table class=llm-prices><caption>{caption}</caption><thead><tr><th scope=col>{}</th>",
        choose(en, "计价", "Tier")
    );
    let columns: Vec<Column> = if unit == Unit::Tokens {
        let mut columns: Vec<Column> = vec![
            (choose(en, "输入", "Input"), |price| price.input),
            (choose(en, "缓存命中", "Cached input"), |price| {
                price.cached_input
            }),
        ];
        if rows.iter().any(|price| price.cache_write_1h.is_some()) {
            columns.push((
                choose(en, "缓存写入 5 分钟", "Cache write 5m"),
                |price| price.cache_write,
            ));
            columns.push((
                choose(en, "缓存写入 1 小时", "Cache write 1h"),
                |price| price.cache_write_1h,
            ));
        } else {
            columns.push((choose(en, "缓存写入", "Cache write"), |price| {
                price.cache_write
            }));
        }
        columns.push((choose(en, "输出", "Output"), |price| price.output));
        columns
    } else if rows.iter().any(|price| price.input.is_some())
        && rows.iter().any(|price| price.output.is_some())
    {
        vec![
            (choose(en, "输入", "Input"), |price| price.input),
            (choose(en, "输出", "Output"), |price| price.output),
        ]
    } else {
        vec![(choose(en, "价格", "Price"), |price| {
            price.output.or(price.input)
        })]
    };
    for (label, _) in &columns {
        let _ = write!(html, "<th scope=col>{label}</th>");
    }
    html.push_str("</tr></thead><tbody>");
    for price in rows {
        let _ = write!(
            html,
            "<tr><th scope=row>{}</th>",
            escape(&row_label(price, rows, en))
        );
        for (_, value) in &columns {
            let _ = write!(html, "<td>{}</td>", price_cell(currency, value(price), en));
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table></div>");
    html
}

fn change_json(change: &ChangeRow) -> Value {
    json!({
        "id": change.id, "vendor": change.vendor_id, "model_id": change.model_id,
        "kind": change.kind, "before": change.before.as_ref().map(|value| &value.0),
        "after": change.after.as_ref().map(|value| &value.0),
        "source_url": change.source_url, "detected_at": change.detected_at,
    })
}

async fn data(
    State(state): State<AppState>,
    reader: Reader,
    headers: HeaderMap,
) -> Result<Response> {
    let catalog = load(&state, FEED_SIZE, reader)
        .await?
        .ok_or_else(not_found)?;
    let vendors: Vec<Value> = catalog
        .vendors
        .iter()
        .map(|vendor| {
            json!({
                "id": vendor.id, "name": vendor.name, "name_en": vendor.name_en,
                "region": vendor.region, "verified_at": vendor.verified_at, "pricing_urls": vendor.pricing,
                "docs_url": vendor.docs,
            })
        })
        .collect();
    let models: Vec<Value> = catalog
        .models
        .iter()
        .map(|model| {
            json!({
                "vendor": model.vendor_id, "model_id": model.model_id, "category": model.category,
                "name": model.name,
                "summary": model.summary, "summary_en": model.summary_en, "status": model.status,
                "knowledge_cutoff": model.knowledge_cutoff,
                "context_tokens": model.context_tokens, "max_output_tokens": model.max_output_tokens,
                "input_modalities": model.input_modalities, "output_modalities": model.output_modalities,
                "features": model.features, "prices": model.prices.0, "links": model.links.0,
                "sources": model.evidence.0["prices"].as_array().map(|items| {
                    let mut urls: Vec<&str> = items.iter().filter_map(|item| item["url"].as_str()).collect();
                    urls.dedup();
                    urls
                }),
                "first_seen_at": model.first_seen_at, "verified_at": model.verified_at, "changed_at": model.changed_at,
            })
        })
        .collect();
    let changes: Vec<Value> = catalog.changes.iter().map(change_json).collect();
    let body = json!({
        "name": "𝑳𝑳𝑴", "url": absolute(&state, "/llm"),
        "description": "Mainstream model prices and specs from each provider's own documentation: language, image, video, audio, embedding and rerank models. Each price row states its currency and unit: tokens and characters per 1M, image, second, minute, hour and request per one.",
        "verified_at": catalog.verified_at(), "vendors": vendors, "models": models, "changes": changes,
    });
    Ok(conditional(
        &headers,
        &Rendered::new(body.to_string()),
        "application/json",
    ))
}

async fn feed(State(state): State<AppState>, headers: HeaderMap) -> Result<Response> {
    let catalog = load(&state, FEED_SIZE, Reader::Public)
        .await?
        .ok_or_else(not_found)?;
    let base = state.config.public_base_url.as_str();
    let updated = catalog
        .changes
        .first()
        .map(|change| change.detected_at)
        .or_else(|| catalog.verified_at())
        .unwrap_or_else(Utc::now);
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><feed xmlns="http://www.w3.org/2005/Atom" xml:lang="zh-CN"><title>𝑳𝑳𝑴 · NEWS</title><id>{base}llm/feed.xml</id><link rel="self" href="{base}llm/feed.xml"/><link rel="alternate" href="{base}llm"/><updated>{}</updated>"#,
        updated.to_rfc3339()
    );
    for change in &catalog.changes {
        let Some(vendor) = catalog.vendor(&change.vendor_id) else {
            continue;
        };
        let (kind, detail) = describe(change, &vendor.region, false);
        let _ = write!(
            xml,
            r#"<entry><id>{base}llm#change-{id}</id><title>{title}</title><link rel="alternate" href="{link}"/><updated>{at}</updated><author><name>{vendor}</name></author><category term="{kind}"/><summary>{summary}</summary></entry>"#,
            id = change.id,
            title = escape(&match &change.model_id {
                Some(model) => format!("{} {model} · {kind}", vendor.name),
                None => format!("{} · {kind}", vendor.name),
            }),
            link = escape(&match &change.model_id {
                Some(model) => format!("{base}llm#{}", fragment(&change.vendor_id, model)),
                None => format!("{base}llm"),
            }),
            at = change.detected_at.to_rfc3339(),
            vendor = escape(&vendor.name),
            summary = escape(&match &change.name {
                Some(name) => format!("{name} {detail}"),
                None => detail,
            }),
        );
    }
    xml.push_str("</feed>");
    Ok(conditional(
        &headers,
        &Rendered::new(xml),
        "application/atom+xml; charset=utf-8",
    ))
}
