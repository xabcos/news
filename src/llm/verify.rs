//! Nothing the model reports is published on its word. A price or a limit must sit in a
//! verbatim excerpt of the provider's page, in that model's section, under the nearest line
//! that names it, and in its own column where a table lists models as columns. A price's
//! unit must be the one the page states, and a model's token rates must keep the orderings
//! every price table follows.
use super::{
    Band, Currency, Links, Model, Period, Price, Tier, Unit,
    extract::{Draft, DraftPrice, Fact, Stated},
    source::{Kind, Link, Page, collapse},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;

const MAX_PRICE: f64 = 100_000.0;
const MAX_QUOTE: usize = 400;
const MAX_LABEL: usize = 80;
/// How far above a price row the model's name may appear, as in a section heading.
const CONTEXT_LINES: usize = 60;
/// How pages state the token count prices are for; `1M` alone is left out, as it is also
/// how pages write context windows.
const PER_MILLION: [&str; 9] = [
    "百万",
    "million",
    "mtok",
    "1m tokens",
    "1m token",
    "m tokens",
    "1,000,000 tokens",
    "per 1m",
    "/1m",
];
const PER_THOUSAND: [&str; 10] = [
    "千tokens",
    "千 tokens",
    "千token",
    "千 token",
    "每千",
    "thousand",
    "1k tokens",
    "k tokens",
    "per 1k",
    "/1k",
];
/// How pages name each unit, lowercased and without a space after a slash. A Chinese unit
/// counts only as a rate (`/张`, `每张`, `按张`), never in words like 首次 or 条件.
fn marks(stated: Stated) -> &'static [&'static str] {
    match stated {
        Stated::MillionTokens => &PER_MILLION,
        Stated::ThousandTokens => &PER_THOUSAND,
        Stated::MillionCharacters => &[
            "百万字符",
            "million characters",
            "1m characters",
            "m characters",
            "1,000,000 characters",
            "1m chars",
            "m chars",
        ],
        Stated::TenThousandCharacters => &["万字符", "10k characters", "10,000 characters"],
        Stated::ThousandCharacters => &[
            "千字符",
            "thousand characters",
            "1k characters",
            "k characters",
            "1,000 characters",
            "1k chars",
            "k chars",
        ],
        Stated::Image => &[
            "/image",
            "/img",
            "per image",
            "per img",
            "each image",
            "/张",
            "每张",
            "按张",
        ],
        Stated::Second => &["/second", "per second", "/sec", "/s", "/秒", "每秒", "按秒"],
        Stated::Minute => &["/minute", "/min", "per minute", "/分钟", "每分钟", "按分钟"],
        Stated::Hour => &[
            "/hour",
            "per hour",
            "/hr",
            "/h",
            "/小时",
            "每小时",
            "按小时",
        ],
        Stated::Request => &[
            "/request",
            "per request",
            "per call",
            "/call",
            "per video",
            "/video",
            "per generation",
            "/次",
            "每次",
            "按次",
            "/条",
            "每条",
        ],
        Stated::ThousandRequests => &[
            "thousand requests",
            "1k requests",
            "1,000 requests",
            "1k searches",
            "1,000 searches",
            "千次",
        ],
    }
}

/// The units whose names hold this one's: 百万字符 holds 百万 and 万字符, and 千次, 千字符
/// and 1K requests hold 每千 and per 1K.
fn holders(stated: Stated) -> &'static [Stated] {
    match stated {
        Stated::MillionTokens | Stated::TenThousandCharacters => &[Stated::MillionCharacters],
        Stated::ThousandTokens => &[Stated::ThousandCharacters, Stated::ThousandRequests],
        _ => &[],
    }
}

/// Whether a text names this unit whole: not as part of a larger unit, and not as part of
/// a longer word, as `/s` is of `/speed` and `1k tokens` of `21k tokens`.
fn states(text: &str, stated: Stated) -> bool {
    let text = holders(stated)
        .iter()
        .flat_map(|holder| marks(*holder))
        .fold(text.to_owned(), |text, mark| text.replace(mark, " "));
    let word = |c: char| c.is_ascii_alphanumeric();
    marks(stated).iter().any(|mark| {
        text.match_indices(mark).any(|(at, _)| {
            (!mark.starts_with(word) || !text[..at].ends_with(word))
                && (!mark.ends_with(word) || !text[at + mark.len()..].starts_with(word))
        })
    })
}

/// Lowercased, with `$0.02 / img` written as `$0.02/img`.
fn normal(text: &str) -> String {
    text.to_lowercase().replace("/ ", "/")
}

const MODALITIES: [&str; 5] = ["text", "image", "audio", "video", "pdf"];
const FEATURES: [&str; 5] = [
    "reasoning",
    "tools",
    "structured_output",
    "web_search",
    "open_weights",
];

#[derive(Debug, Serialize)]
pub(crate) struct Rejection {
    pub(crate) model_id: String,
    pub(crate) reason: &'static str,
    /// The excerpt that failed, for the run record.
    pub(crate) quote: String,
    /// Whether the model's prices were set aside as a whole; a dropped extra tier is not.
    pub(crate) fatal: bool,
}

pub(crate) struct Verified {
    pub(crate) models: Vec<Model>,
    pub(crate) rejected: Vec<Rejection>,
}

/// A page flattened for excerpt search, keeping where each line begins.
struct Text<'a> {
    page: &'a Page,
    flat: String,
    /// `flat` lowercased once, for name lookups.
    lower: String,
    starts: Vec<usize>,
    /// Whether the page prices per million and per thousand tokens.
    tokens: (bool, bool),
}

impl<'a> Text<'a> {
    fn new(page: &'a Page) -> Self {
        let mut flat = String::new();
        let mut starts = Vec::with_capacity(page.lines.len());
        for line in &page.lines {
            if !flat.is_empty() {
                flat.push(' ');
            }
            starts.push(flat.len());
            flat.push_str(line);
        }
        let lower = flat.to_lowercase();
        let page_text = normal(&lower);
        Self {
            page,
            flat,
            lower,
            starts,
            tokens: (
                states(&page_text, Stated::MillionTokens),
                states(&page_text, Stated::ThousandTokens),
            ),
        }
    }

    /// The lines of every place the page contains the excerpt verbatim. Models that share
    /// a price share the excerpt, so each place is a candidate.
    fn find_all(&self, quote: &str) -> Vec<(usize, usize)> {
        let quote = collapse(quote);
        if quote.is_empty() {
            return Vec::new();
        }
        let line = |offset: usize| self.starts.partition_point(|&begin| begin <= offset) - 1;
        self.flat
            .match_indices(quote.as_str())
            .map(|(start, found)| (line(start), line(start + found.len() - 1)))
            .collect()
    }

    /// The first place the page shows the excerpt as this model's: the section around it
    /// names the model, and so does the model's own row within the excerpt or, failing
    /// that, the nearest line above it that names any model. Models that share a price
    /// share the excerpt, so every place is tried.
    fn place(&self, quote: &str, names: &[String], all: &[String]) -> Result<Place, &'static str> {
        let places = self.find_all(quote);
        if places.is_empty() {
            return Err("摘录不在官方页面中");
        }
        let names_model = |index: usize| naming(&self.page.lines[index], names, all).this;
        for (first, last) in places {
            let context = self.context(first, last);
            if !names.iter().any(|name| mentions(&context, name)) {
                continue;
            }
            // An excerpt that spans several models' rows speaks only for this model's row,
            // and for the rows under it until the next model is named.
            if let Some(own) = (first..=last).find(|&index| names_model(index)) {
                let column = column(&self.page.lines[own], names, all).filter(|_| own < last);
                let (first, last) = if column.is_some() {
                    (own + 1, last)
                } else {
                    let end = (own + 1..=last)
                        .find(|&index| {
                            let lower = self.page.lines[index].to_lowercase();
                            all.iter().any(|name| mentions_lower(&lower, name))
                        })
                        .map_or(last, |next| next - 1);
                    (own, end)
                };
                return Ok(Place {
                    first,
                    last,
                    column,
                    context,
                });
            }
            if let Some(named) = self.named_line(first, names, all) {
                // Named above the quoted rows, in a column other than the row labels: the
                // table lists models as columns.
                return Ok(Place {
                    first,
                    last,
                    column: column(&self.page.lines[named], names, all),
                    context,
                });
            }
        }
        Err("摘录附近没有该模型")
    }

    /// The nearest line at or above `first`, within its section, that names this model, as
    /// long as no line between names another model the way a model owns what follows it: as
    /// a heading, a row label or a line of its own. Running text that mentions another model
    /// in passing, as "priced the same as 2.5 Flash" does, and rows that name models across
    /// columns, as a version row under a column table's header does, are passed over.
    fn named_line(&self, first: usize, names: &[String], all: &[String]) -> Option<usize> {
        let lines = &self.page.lines;
        let floor = first.saturating_sub(CONTEXT_LINES);
        let mut index = first;
        loop {
            let line = &lines[index];
            let named = naming(line, names, all);
            if named.this {
                return Some(index);
            }
            let owner = if line.starts_with('|') {
                cells(line)
                    .first()
                    .is_some_and(|label| naming(label, names, all).others)
            } else {
                // A heading, or a line of nothing but names: `gemini-live-b`, `- gemini-live-b:`.
                named.others
                    && (line.starts_with('#') || !named.rest.chars().any(char::is_alphanumeric))
            };
            if owner {
                return None;
            }
            if index == floor || (index < first && section(line)) {
                return None;
            }
            index -= 1;
        }
    }

    /// What an excerpt says about one model: its own rows, and where a table lists models
    /// as columns, only that model's cells of them.
    fn own_text(&self, place: &Place) -> String {
        let Some(column) = place.column else {
            return self.page.lines[place.first..=place.last].join(" ");
        };
        (place.first..=place.last)
            .map(|index| {
                let line = &self.page.lines[index];
                if !line.starts_with('|') {
                    return line.clone();
                }
                let cells = cells(line);
                let count = cells.len();
                // A row too short to reach the column has one merged value for every model.
                if count > column + 1 {
                    cells[count - 1 - column].to_owned()
                } else {
                    cells[count - 1].to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The table around the lines and its section: back to the nearest `#` or `##`
    /// heading, as a model's section often splits its tiers into `###` subsections.
    fn context(&self, first: usize, last: usize) -> String {
        let lines = &self.page.lines;
        let row = |index: usize| lines[index].starts_with('|');
        let section = |index: usize| section(&lines[index]);
        let (mut start, mut end) = (first, last);
        if row(first) {
            while start > 0 && row(start - 1) {
                start -= 1;
            }
            while end + 1 < lines.len() && row(end + 1) {
                end += 1;
            }
        }
        let floor = start.saturating_sub(CONTEXT_LINES);
        while start > floor && !section(start) {
            start -= 1;
        }
        lines[start..=end].join(" ")
    }
}

/// Where an excerpt sits, once it is known to be this model's.
struct Place {
    first: usize,
    last: usize,
    /// This model's column counted from the right, for a table that lists models as
    /// columns. Row labels on the left may span rows, so values align on the right.
    column: Option<usize>,
    /// The table and section around the excerpt.
    context: String,
}

fn section(line: &str) -> bool {
    line.starts_with("# ") || line.starts_with("## ")
}

fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split(" | ")
        .map(str::trim)
        .collect()
}

/// An API model name as pages write one, like `deepseek-flash` or `GLM-5.3`, even for a
/// model this read did not list.
fn id_like(token: &str) -> bool {
    token.len() >= 5
        && token.contains('-')
        && token.starts_with(|c: char| c.is_ascii_alphabetic())
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// This model's column counted from the right, when the row is a header that names
/// several models across its cells. A row naming one model, even after a family or
/// category column, is that model's own row.
fn column(line: &str, names: &[String], all: &[String]) -> Option<usize> {
    if !line.starts_with('|') {
        return None;
    }
    let cells = cells(line);
    // The first cell labels the row, and may name a model family, as Baidu's tables do.
    let models = cells
        .iter()
        .skip(1)
        .filter(|cell| {
            all.iter().any(|name| mentions(cell, name))
                || cell
                    .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '，' | '/'))
                    .any(id_like)
        })
        .count();
    if models < 2 {
        return None;
    }
    let index = cells
        .iter()
        .position(|cell| naming(cell, names, all).this)?;
    (index > 0).then(|| cells.len() - 1 - index)
}

/// What a row writes at one place an amount appears: the currency beside it, as in `$1` or
/// `1 元`, and the unit right after it, up to the next separator, as `元/百万字符` after `20`
/// or `/img` after `$0.04` in `$0.04/img, $5/1m tokens`.
struct Written {
    currency: Option<Currency>,
    unit: String,
}

/// Every place a lowercased row writes this amount.
fn written_at(row: &str, amount: f64) -> Vec<Written> {
    let mut places = Vec::new();
    let mut rest = row;
    while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
        let before = row[..row.len() - rest.len() + start].trim_end();
        let number = &rest[start..];
        rest = number.trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, '.' | ','));
        let digits: String = number[..number.len() - rest.len()]
            .chars()
            .filter(|c| *c != ',')
            .collect();
        let same = digits
            .trim_end_matches('.')
            .parse::<f64>()
            .is_ok_and(|digits| (digits - amount).abs() <= 1e-9 * amount.abs().max(1.0));
        if !same {
            continue;
        }
        let after = rest.trim_start();
        let (mut currency, after) = [
            ("美元", Currency::Usd),
            ("usd", Currency::Usd),
            ("元", Currency::Cny),
            ("cny", Currency::Cny),
            ("rmb", Currency::Cny),
        ]
        .into_iter()
        .find_map(|(word, currency)| Some((Some(currency), after.strip_prefix(word)?)))
        .unwrap_or((None, after));
        // A code before the amount counts as a whole word: `usd 0.5`, not `abcusd 0.5`.
        let code = |word: &str| {
            before
                .strip_suffix(word)
                .is_some_and(|rest| !rest.ends_with(|c: char| c.is_ascii_alphanumeric()))
        };
        if before.ends_with('$') || code("usd") {
            currency = Some(Currency::Usd);
        } else if before.ends_with(['¥', '￥']) || code("cny") || code("rmb") {
            currency = Some(Currency::Cny);
        }
        let after = after.trim_start();
        let mut unit = String::new();
        if after.starts_with('/') || after.starts_with("per ") || after.starts_with('每') {
            // A comma inside a number, as in 1,000 characters, separates nothing.
            let inside = |at: usize| {
                after[..at].ends_with(|c: char| c.is_ascii_digit())
                    && after[at + 1..].starts_with(|c: char| c.is_ascii_digit())
            };
            let end = after
                .char_indices()
                .find(|&(at, c)| (c == ',' && !inside(at)) || "|，;；、$¥￥()（）".contains(c))
                .map_or(after.len(), |(at, _)| at);
            unit = after[..end].to_owned();
        }
        places.push(Written { currency, unit });
    }
    places
}

/// Whether a price is in the unit stated nearest to it. The units written right after its
/// amounts decide alone, each place on its own; then the cells of its own rows that only
/// name a unit, as `| per image |` or `| per 1k characters |` do; then the table and
/// section, once they name any unit. A token rate stated nowhere around takes its page's
/// unit.
fn unit_stated(
    places: &[Vec<Written>],
    own: &str,
    context: &str,
    page: (bool, bool),
    stated: Stated,
) -> bool {
    let mut open = false;
    for amount in places {
        let mut settled = false;
        for place in amount {
            if Stated::ALL
                .into_iter()
                .any(|unit| unit != stated && states(&place.unit, unit))
            {
                return false;
            }
            settled |= states(&place.unit, stated);
        }
        open |= !settled;
    }
    if !open {
        return true;
    }
    for text in [unit_cells(own), normal(context)] {
        if states(&text, stated) {
            return true;
        }
        if named(&text) {
            return false;
        }
    }
    // Per million is how nearly every page prices; only an explicit per-thousand page rules
    // it out.
    let (million, thousand) = page;
    match stated {
        Stated::MillionTokens => million || !thousand,
        Stated::ThousandTokens => thousand,
        _ => false,
    }
}

/// The cells of a lowercased row that only name a rate, as `per 1k characters` or `元/张`
/// do: nothing but words is left once the units' names are taken out. A limit such as
/// `1m tokens` names no rate.
fn unit_cells(own: &str) -> String {
    own.split('|')
        .filter(|cell| {
            let rest = Stated::ALL
                .into_iter()
                .flat_map(marks)
                .fold((*cell).to_owned(), |rest, mark| rest.replace(mark, " "));
            let rate = ["/", "per ", "每", "按"]
                .iter()
                .any(|word| cell.contains(word));
            rate && rest.len() < cell.len() && !rest.contains(|c: char| c.is_ascii_digit())
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Whether a text names any unit.
fn named(text: &str) -> bool {
    Stated::ALL.into_iter().any(|unit| states(text, unit))
}

/// Which models a text names. Longer names are read first, so `Codestral Embed` names that
/// model and not `Codestral`; `rest` is the lowercased text with every name taken out.
struct Naming {
    this: bool,
    others: bool,
    rest: String,
}

/// `all` holds every model's names, longest first; `names` are this model's.
fn naming(text: &str, names: &[String], all: &[String]) -> Naming {
    let mut naming = Naming {
        this: false,
        others: false,
        rest: text.to_lowercase(),
    };
    for name in all {
        if mentions_lower(&naming.rest, name) {
            if names.iter().any(|own| own.eq_ignore_ascii_case(name)) {
                naming.this = true;
            } else {
                naming.others = true;
            }
            naming.rest = remove_mentions(&naming.rest, name);
        }
    }
    naming
}

/// Whether `ident` occurs in `text` as a whole name: `gpt-6` is not found in `gpt-6-astra`
/// or `gpt-6.1`, and `glm-5.3` is found in `GLM-5.3 costs`.
pub(crate) fn mentions(text: &str, ident: &str) -> bool {
    mentions_lower(&text.to_lowercase(), ident)
}

fn mentions_lower(hay: &str, ident: &str) -> bool {
    let needle = ident.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    let joined = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut from = 0;
    while let Some(found) = hay[from..].find(&needle) {
        let start = from + found;
        let end = start + needle.len();
        let before = hay[..start].chars().next_back();
        let mut after = hay[end..].chars();
        let next = after.next();
        let joined_after = next.is_some_and(joined)
            || (next == Some('.') && after.next().is_some_and(|c| c.is_ascii_alphanumeric()));
        if !before.is_some_and(joined) && !joined_after {
            return true;
        }
        from = end;
    }
    false
}

/// Names a draft may appear under in a page: its API id, its display name and, for
/// "Claude Opus 5.5"-style names, the name without the family word.
fn identities(draft: &Draft) -> Vec<String> {
    let mut names = vec![draft.model_id.trim().to_owned()];
    let name = collapse(&draft.name);
    if name.chars().count() >= 3 {
        if let Some((_, rest)) = name.split_once(' ')
            && rest.chars().any(|c| c.is_ascii_digit())
            && rest.chars().count() >= 3
        {
            names.push(rest.to_owned());
        }
        names.push(name);
    }
    names
}

/// Prices written in an excerpt. Numbers joined to letters are left out, so the `6` of
/// `gpt-6` or the `1` of `1M` never confirms a price; the model's names are removed first.
pub(super) fn prices_in(quote: &str, names: &[String]) -> Vec<f64> {
    let mut text = collapse(quote);
    for name in names {
        text = remove_mentions(&text, name);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut values = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let attached = index > 0 && {
            let previous = chars[index - 1];
            previous.is_ascii_alphanumeric() || matches!(previous, '-' | '_' | '.')
        };
        if !c.is_ascii_digit() || attached {
            index += 1;
            continue;
        }
        let begin = index;
        while index < chars.len()
            && (chars[index].is_ascii_digit()
                || (chars[index] == ','
                    && chars[index + 1..].iter().take(3).all(char::is_ascii_digit)
                    && chars[index + 1..].len() >= 3)
                || (chars[index] == '.' && chars.get(index + 1).is_some_and(char::is_ascii_digit)))
        {
            index += 1;
        }
        if chars.get(index).is_some_and(char::is_ascii_alphabetic) {
            continue;
        }
        let token: String = chars[begin..index].iter().filter(|c| **c != ',').collect();
        if let Ok(value) = token.parse::<f64>() {
            values.push(value);
        }
    }
    let lower = text.to_lowercase();
    if lower.contains("free") || text.contains("免费") {
        values.push(0.0);
    }
    values
}

fn remove_mentions(text: &str, ident: &str) -> String {
    let needle = ident.trim().to_lowercase();
    if needle.is_empty() || !mentions(text, ident) {
        return text.to_owned();
    }
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(found) = lower[from..].find(&needle) {
        let start = from + found;
        out.push_str(&text[from..start]);
        out.push(' ');
        from = start + needle.len();
    }
    out.push_str(&text[from..]);
    out
}

/// Token counts written in an excerpt, reading `128K`, `1M`, `1.05M` and `1,050,000`.
/// Binary readings are included, as pages write 131072 tokens as 128K.
pub(super) fn token_counts(text: &str) -> Vec<i64> {
    let chars: Vec<char> = collapse(text).chars().collect();
    let mut counts = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_ascii_digit()
            || (index > 0 && (chars[index - 1].is_ascii_alphanumeric() || chars[index - 1] == '.'))
        {
            index += 1;
            continue;
        }
        let begin = index;
        while index < chars.len()
            && (chars[index].is_ascii_digit()
                || (matches!(chars[index], ',' | '.')
                    && chars.get(index + 1).is_some_and(char::is_ascii_digit)))
        {
            index += 1;
        }
        let token: String = chars[begin..index].iter().filter(|c| **c != ',').collect();
        let Ok(value) = token.parse::<f64>() else {
            continue;
        };
        let mut suffix = index;
        while chars.get(suffix) == Some(&' ') {
            suffix += 1;
        }
        let unit = chars.get(suffix).copied();
        let unit_ends = !chars
            .get(suffix + 1)
            .is_some_and(|c| c.is_ascii_alphabetic() && !matches!(c, 't' | 'T'));
        let scaled = |factor: f64| (value * factor).round() as i64;
        match unit {
            Some('k' | 'K') if unit_ends => {
                counts.push(scaled(1000.0));
                counts.push(scaled(1024.0));
            }
            Some('m' | 'M') if unit_ends => {
                counts.push(scaled(1_000_000.0));
                counts.push(scaled(1_048_576.0));
            }
            Some('万') => counts.push(scaled(10_000.0)),
            _ if value >= 1000.0 && value.fract() == 0.0 => counts.push(value as i64),
            _ => {}
        }
    }
    counts
}

fn clip(value: &str, max: usize) -> String {
    collapse(value).chars().take(max).collect()
}

pub(crate) fn models(drafts: Vec<Draft>, pages: &[Page], links: &[Link]) -> Verified {
    let texts: Vec<Text> = pages.iter().map(Text::new).collect();
    let mut all_names: Vec<String> = drafts.iter().flat_map(identities).collect();
    all_names.sort_by_key(|name| std::cmp::Reverse(name.chars().count()));
    let mut seen = HashSet::new();
    let mut verified = Verified {
        models: Vec::new(),
        rejected: Vec::new(),
    };
    for draft in drafts {
        let model_id = draft.model_id.trim().to_owned();
        let reject = |reason| Rejection {
            model_id: model_id.clone(),
            reason,
            quote: String::new(),
            fatal: true,
        };
        if model_id.is_empty() || model_id.chars().count() > 100 {
            verified.rejected.push(reject("模型 ID 无效"));
            continue;
        }
        if !seen.insert(model_id.to_lowercase()) {
            continue;
        }
        if !texts
            .iter()
            .any(|text| mentions_lower(&text.lower, &model_id))
        {
            verified.rejected.push(reject("模型 ID 不在官方页面中"));
            continue;
        }
        let names = identities(&draft);
        let check = verify_prices(&draft.prices, &texts, &names, &all_names);
        verified
            .rejected
            .extend(
                check
                    .rejected
                    .into_iter()
                    .map(|(reason, quote, fatal)| Rejection {
                        model_id: model_id.clone(),
                        reason,
                        quote,
                        fatal,
                    }),
            );
        let prices = check.prices;
        let facts = verified_facts(&draft.facts, &texts, &names, &all_names);
        verified
            .models
            .push(model(draft, model_id, prices, facts, links));
    }
    verified
}

struct Evidence {
    url: String,
    quote: String,
}

/// Verified price rows with the excerpt behind each.
type PriceRead = (Vec<Price>, Vec<Evidence>);

/// `Ok(None)` when the draft lists no prices; stored prices then stay.
/// A model's verified price rows, and the rows set aside. A tier other than the standard
/// token rate that fails is dropped on its own; a failed standard token rate, or rows that
/// break the table's orderings, set every price of the model aside so the stored ones stay.
struct PriceCheck {
    prices: Option<PriceRead>,
    rejected: Vec<(&'static str, String, bool)>,
}

fn verify_prices(
    drafts: &[DraftPrice],
    texts: &[Text],
    names: &[String],
    all: &[String],
) -> PriceCheck {
    let mut check = PriceCheck {
        prices: None,
        rejected: Vec::new(),
    };
    if drafts.is_empty() {
        return check;
    }
    let mut prices = Vec::with_capacity(drafts.len());
    let mut evidence = Vec::with_capacity(drafts.len());
    let mut fatal = false;
    for draft in drafts {
        match verify_row(draft, texts, names, all) {
            Ok((price, found)) => {
                prices.push(price);
                evidence.push(found);
            }
            Err(reason) => {
                // The standard token rate leads a language model's row; another unit's rows
                // differ by size or resolution, and one that fails is dropped alone.
                let standard =
                    draft.tier == Tier::Standard && draft.unit.stored().0 == Unit::Tokens;
                fatal |= standard;
                check
                    .rejected
                    .push((reason, clip(&draft.quote, 160), standard));
            }
        }
    }
    if fatal || prices.is_empty() {
        for rejection in &mut check.rejected {
            rejection.2 = true;
        }
        return check;
    }
    match merged(prices).and_then(|prices| consistent(&prices).map(|()| prices)) {
        Ok(prices) => check.prices = Some((prices, evidence)),
        Err(reason) => check.rejected.push((reason, String::new(), true)),
    }
    check
}

fn verify_row(
    draft: &DraftPrice,
    texts: &[Text],
    names: &[String],
    all: &[String],
) -> Result<(Price, Evidence), &'static str> {
    let text = texts.get(draft.source).ok_or("价格来源编号无效")?;
    let currency = text
        .page
        .currency
        .filter(|_| text.page.kind == Kind::Pricing)
        .ok_or("价格只能取自定价页")?;
    let quoted = naming(&collapse(&draft.quote), names, all);
    if !quoted.this && quoted.others {
        return Err("价格摘录属于其他模型");
    }
    let place = text.place(&draft.quote, names, all)?;
    let (unit, factor) = draft.unit.stored();
    let own = normal(&text.own_text(&place));
    let mut values = [
        draft.input,
        draft.cached_input,
        draft.cache_write,
        draft.cache_write_1h,
        draft.output,
    ];
    let written = prices_in(&own, names);
    for slot in &mut values {
        let Some(value) = *slot else {
            continue;
        };
        let on_page = written
            .iter()
            .any(|found| (found - value).abs() <= 1e-9 * value.abs().max(1.0));
        // A rate the row does not charge, as input a model bills only on output, is not
        // listed; only a row that says 免费 makes a rate free.
        if value == 0.0 && !on_page {
            *slot = None;
        } else if !value.is_finite() || value < 0.0 || !on_page {
            return Err("价格数字不在摘录中");
        }
    }
    if values.iter().all(Option::is_none) {
        return Err("价格行没有数值");
    }
    let places: Vec<Vec<Written>> = values
        .iter()
        .flatten()
        .map(|amount| written_at(&own, *amount))
        .collect();
    // A page listing yuan and dollars, as one for China and abroad does, prices each amount
    // in the currency written beside it; an amount written bare takes its page's.
    let currencies: HashSet<Currency> = places
        .iter()
        .flatten()
        .filter_map(|place| place.currency)
        .collect();
    let currency = match currencies.len() {
        0 => currency,
        1 => currencies.into_iter().next().unwrap_or(currency),
        _ => return Err("价格币种不一致"),
    };
    if !unit_stated(&places, &own, &place.context, text.tokens, draft.unit) {
        return Err("价格单位与页面不符");
    }
    for value in values.iter_mut().flatten() {
        *value = (*value * factor * 1e9).round() / 1e9;
        if *value > MAX_PRICE {
            return Err("价格超出合理范围");
        }
    }
    // A label may join what a table writes apart, as "720p · 输入不含视频"; each part must be
    // the page's own words.
    let label = collapse(&draft.label);
    let stated = label.chars().count() <= MAX_LABEL
        && label
            .split(['·', '，', ',', '/', '；', ';'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .all(|part| mentions(&place.context, part));
    let label = match (stated, unit) {
        (true, _) => label,
        (false, Unit::Tokens) => String::new(),
        // A size or resolution tells a media model's rows apart; one the page does not state
        // cannot.
        (false, _) => return Err("规格不在页面上"),
    };
    // A table giving input lengths in thousands of tokens writes the 32K tier as 32.
    let in_thousands = states(&normal(&place.context), Stated::ThousandTokens);
    let threshold = draft.context_threshold.filter(|threshold| {
        token_counts(&place.context).contains(threshold)
            || (in_thousands
                && written
                    .iter()
                    .any(|count| (count * 1000.0).round() as i64 == *threshold))
    });
    let [input, cached_input, cache_write, cache_write_1h, output] = values;
    Ok((
        Price {
            currency,
            unit,
            tier: draft.tier,
            label,
            context: draft.context,
            context_threshold: threshold,
            period: draft.period,
            mode: draft.mode,
            input,
            cached_input,
            cache_write,
            cache_write_1h,
            output,
        },
        Evidence {
            url: text.page.url.clone(),
            quote: clip(&draft.quote, MAX_QUOTE),
        },
    ))
}

/// A read may give one tier's rates as several rows, one per rate. Rows of the same tier
/// combine when they do not disagree; two different values for one rate are a conflict.
fn merged(prices: Vec<Price>) -> Result<Vec<Price>, &'static str> {
    let mut out: Vec<Price> = Vec::with_capacity(prices.len());
    for price in prices {
        let Some(row) = out.iter_mut().find(|row| row.key() == price.key()) else {
            out.push(price);
            continue;
        };
        for (slot, value) in [
            (&mut row.input, price.input),
            (&mut row.cached_input, price.cached_input),
            (&mut row.cache_write, price.cache_write),
            (&mut row.cache_write_1h, price.cache_write_1h),
            (&mut row.output, price.output),
        ] {
            match (*slot, value) {
                (Some(a), Some(b)) if !super::same_value(Some(a), Some(b)) => {
                    return Err("价格行重复");
                }
                (None, Some(b)) => *slot = Some(b),
                _ => {}
            }
        }
    }
    Ok(out)
}

fn at_most(lower: Option<f64>, upper: Option<f64>) -> bool {
    match (lower, upper) {
        (Some(lower), Some(upper)) => lower <= upper + 1e-9,
        _ => true,
    }
}

/// Orderings every provider's token table follows; a column read in the wrong place breaks
/// one. Rows in other units differ by size or resolution, which orders nothing.
pub(crate) fn consistent(prices: &[Price]) -> Result<(), &'static str> {
    let mut keys = HashSet::new();
    for price in prices {
        if !keys.insert(price.key()) {
            return Err("价格行重复");
        }
    }
    let tokens: Vec<&Price> = prices
        .iter()
        .filter(|price| price.unit == Unit::Tokens)
        .collect();
    for price in &tokens {
        if !at_most(price.cached_input, price.input)
            || !at_most(price.cache_write, price.cache_write_1h)
        {
            return Err("缓存价格与输入价格不一致");
        }
    }
    for price in &tokens {
        for other in &tokens {
            let same_currency = price.currency == other.currency;
            // A discounted tier never costs more than the standard one; a faster tier never less.
            if same_currency
                && other.tier == Tier::Standard
                && price.label == other.label
                && price.context == other.context
                && price.context_threshold == other.context_threshold
                && price.period == other.period
                && price.mode == other.mode
            {
                let ordered = match price.tier {
                    Tier::Batch | Tier::Flex => {
                        at_most(price.input, other.input) && at_most(price.output, other.output)
                    }
                    Tier::Priority => {
                        at_most(other.input, price.input) && at_most(other.output, price.output)
                    }
                    Tier::Standard => true,
                };
                if !ordered {
                    return Err("价格档位顺序不一致");
                }
            }
            let same_row = same_currency
                && price.tier == other.tier
                && price.label == other.label
                && price.mode == other.mode;
            // Longer prompts never cost less: long over short, and a later tier over an earlier one.
            let longer = price.context == Band::Long
                && (other.context == Band::Short
                    || (other.context == Band::Long
                        && price.context_threshold > other.context_threshold));
            if same_row
                && price.period == other.period
                && longer
                && !(at_most(other.input, price.input) && at_most(other.output, price.output))
            {
                return Err("长上下文价格低于短上下文");
            }
            if same_row
                && price.context == other.context
                && price.context_threshold == other.context_threshold
                && price.period == Period::Peak
                && other.period == Period::OffPeak
                && !(at_most(other.input, price.input) && at_most(other.output, price.output))
            {
                return Err("高峰价格低于空闲价格");
            }
        }
    }
    Ok(())
}

/// Excerpts behind a model's limits, dates and capabilities, each with what it says about
/// this model; one placed under another model is dropped.
fn verified_facts(
    facts: &[Fact],
    texts: &[Text],
    names: &[String],
    all: &[String],
) -> Vec<(Evidence, String)> {
    facts
        .iter()
        .filter_map(|fact| {
            let text = texts.get(fact.source)?;
            let quoted = naming(&collapse(&fact.quote), names, all);
            if !quoted.this && quoted.others {
                return None;
            }
            let place = text.place(&fact.quote, names, all).ok()?;
            Some((
                Evidence {
                    url: text.page.url.clone(),
                    quote: clip(&fact.quote, MAX_QUOTE),
                },
                text.own_text(&place),
            ))
        })
        .collect()
}

fn listed(values: &[String], allowed: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if allowed.contains(&value.as_str()) && !out.contains(value) {
            out.push(value.clone());
        }
    }
    out
}

fn model(
    draft: Draft,
    model_id: String,
    prices: Option<PriceRead>,
    facts: Vec<(Evidence, String)>,
    links: &[Link],
) -> Model {
    let fact_text = facts
        .iter()
        .map(|(_, own)| own.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let facts: Vec<Evidence> = facts.into_iter().map(|(evidence, _)| evidence).collect();
    let count = |value: Option<i64>| {
        value
            .filter(|value| *value > 0 && token_counts(&fact_text).contains(value))
            .and_then(|value| i32::try_from(value).ok())
    };
    let cutoff = clip(&draft.knowledge_cutoff, 40);
    let knowledge_cutoff = if !cutoff.is_empty()
        && collapse(&fact_text)
            .to_lowercase()
            .contains(&cutoff.to_lowercase())
    {
        cutoff
    } else {
        String::new()
    };
    // Capabilities are only listed for a model with at least one verified excerpt.
    let evidenced = !facts.is_empty();
    let link = |index: Option<usize>| {
        index
            .and_then(|index| links.get(index))
            .map(|link| link.url.clone())
    };
    let (prices, price_evidence) = match prices {
        Some((prices, evidence)) => (Some(prices), evidence),
        None => (None, Vec::new()),
    };
    let quotes = |items: &[Evidence]| -> Vec<Value> {
        items
            .iter()
            .map(|item| json!({"url": item.url, "quote": item.quote}))
            .collect()
    };
    Model {
        name: {
            let name = clip(&draft.name, 80);
            if name.is_empty() {
                model_id.clone()
            } else {
                name
            }
        },
        model_id,
        category: draft.category,
        summary: clip(&draft.summary, 160),
        summary_en: clip(&draft.summary_en, 240),
        status: draft.status,
        knowledge_cutoff,
        context_tokens: count(draft.context_tokens),
        max_output_tokens: count(draft.max_output_tokens),
        input_modalities: if evidenced {
            listed(&draft.input_modalities, &MODALITIES)
        } else {
            Vec::new()
        },
        output_modalities: if evidenced {
            listed(&draft.output_modalities, &MODALITIES[..4])
        } else {
            Vec::new()
        },
        features: if evidenced {
            listed(&draft.features, &FEATURES)
        } else {
            Vec::new()
        },
        prices,
        links: Links {
            docs: link(draft.docs_link),
            announcement: link(draft.announcement_link),
        },
        evidence: json!({"prices": quotes(&price_evidence), "facts": quotes(&facts)}),
    }
}
