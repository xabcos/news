//! Official pages as the model reads them: the main content as lines, one line per table
//! row, and the links on the provider's own domains.
use super::Currency;
use crate::{
    error::{AppError, Result},
    media::is_public_origin,
    state::AppState,
};
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
use scraper::{ElementRef, Html, Node, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, time::Duration};
use url::Url;

pub(crate) mod markdown;

const MAX_BYTES: u64 = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_DELAY: Duration = Duration::from_secs(3);
/// Pricing tables of the largest providers stay well inside this.
const PAGE_CHARS: usize = 150_000;
/// Changelogs list the newest entries first; only the recent part matters.
const CHANGELOG_CHARS: usize = 16_000;
/// Below this a page is an error, consent or region notice, not documentation.
const MIN_CHARS: usize = 500;
pub(crate) const MAX_LINKS: usize = 400;
const MAX_DEPTH: usize = 128;
const AGENT: &str = "XABCNews-LLM/1.0 (+https://xabcnews.com/llm)";

/// Not page content, or struck-through text such as the old price beside a discount.
const SKIPPED: [&str; 11] = [
    "script", "style", "noscript", "svg", "template", "iframe", "nav", "aside", "del", "s",
    "strike",
];
const BLOCKS: [&str; 30] = [
    "address",
    "article",
    "blockquote",
    "caption",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "li",
    "main",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tr",
    "ul",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Pricing,
    Models,
    Changelog,
}

impl Kind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "pricing" => Some(Self::Pricing),
            "models" => Some(Self::Models),
            "changelog" => Some(Self::Changelog),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Pricing => "pricing",
            Self::Models => "models",
            Self::Changelog => "changelog",
        }
    }

    fn limit(self) -> usize {
        match self {
            Self::Changelog => CHANGELOG_CHARS,
            Self::Pricing | Self::Models => PAGE_CHARS,
        }
    }
}

/// A registered official page.
#[derive(Clone, Debug)]
pub(crate) struct Source {
    pub(crate) id: i64,
    pub(crate) kind: Kind,
    pub(crate) url: String,
    pub(crate) currency: Option<Currency>,
    pub(crate) content_hash: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Link {
    pub(crate) text: String,
    pub(crate) url: String,
}

/// One official page as read in this run.
#[derive(Clone, Debug)]
pub(crate) struct Page {
    pub(crate) source_id: i64,
    pub(crate) kind: Kind,
    pub(crate) currency: Option<Currency>,
    /// The page the registered URL finally resolved to.
    pub(crate) url: String,
    pub(crate) lines: Vec<String>,
    pub(crate) links: Vec<Link>,
    pub(crate) hash: Vec<u8>,
}

impl Page {
    pub(crate) fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// A host counts as official when it is one of the provider's domains or below one.
pub(crate) fn official(url: &Url, domains: &[String]) -> bool {
    url.host_str().is_some_and(|host| {
        domains.iter().any(|domain| {
            host == domain
                || host
                    .strip_suffix(domain.as_str())
                    .is_some_and(|prefix| prefix.ends_with('.'))
        })
    })
}

pub(crate) async fn fetch(state: &AppState, source: &Source, domains: &[String]) -> Result<Page> {
    let english = source.currency != Some(Currency::Cny);
    let limit = source.kind.limit();
    let (url, lines, links) = match Url::parse(&source.url)
        .ok()
        .and_then(|url| volcengine_doc(&url))
    {
        Some(api) => {
            let (_, body) =
                official_body(state, api.as_str(), domains, english, Body::Json).await?;
            let document: Document = serde_json::from_str(&body)
                .map_err(|_| AppError::BadRequest("官方文档接口的内容无法识别".into()))?;
            let base = Url::parse(&source.url)
                .map_err(|_| AppError::BadRequest("官方页面地址无效".into()))?;
            let text = format!("# {}\n{}", document.result.title, document.result.content);
            let (lines, links) = markdown::read(&text, &base, domains, limit);
            (base, lines, links)
        }
        None => {
            let (url, html) =
                official_body(state, &source.url, domains, english, Body::Html).await?;
            let (mut lines, mut links) = read(&html, &url, domains, limit);
            // A price list without table rows is drawn in the browser; the Markdown version
            // the page links for machine readers has the tables.
            if source.kind == Kind::Pricing
                && !lines.iter().any(|line| line.starts_with('|'))
                && let Some(alternate) = markdown_alternate(&html, &url)
                && let Ok((_, text)) =
                    official_body(state, alternate.as_str(), domains, english, Body::Markdown).await
            {
                (lines, links) = markdown::read(&text, &url, domains, limit);
            }
            (url, lines, links)
        }
    };
    if lines.iter().map(|line| line.chars().count()).sum::<usize>() < MIN_CHARS {
        return Err(AppError::BadRequest("官方页面没有可读取的正文".into()));
    }
    let hash = Sha256::digest(lines.join("\n").as_bytes()).to_vec();
    Ok(Page {
        source_id: source.id,
        kind: source.kind,
        currency: source.currency,
        url: url.to_string(),
        lines,
        links,
        hash,
    })
}

/// Every official link on a page, navigation and footer included: where a provider's
/// homepage points for its developer platform, documentation and prices.
pub(crate) async fn links(
    state: &AppState,
    url: &str,
    domains: &[String],
    english: bool,
) -> Result<Vec<Link>> {
    let (url, html) = official_body(state, url, domains, english, Body::Html).await?;
    Ok(anchors(&html, &url, domains))
}

pub(crate) fn anchors(html: &str, base: &Url, domains: &[String]) -> Vec<Link> {
    let document = Html::parse_document(html);
    let mut writer = Writer::new(base, domains, 0);
    for anchor in document.select(&selector("a[href]")) {
        writer.link(anchor);
    }
    writer.links
}

/// The Markdown version a page links for machine readers.
fn markdown_alternate(html: &str, base: &Url) -> Option<Url> {
    Html::parse_document(html)
        .select(&selector(
            r#"link[rel="alternate"][type="text/markdown"][href]"#,
        ))
        .next()
        .and_then(|link| base.join(link.value().attr("href")?).ok())
}

/// Volcengine draws its documentation in the browser, and its documentation API serves the
/// same document as Markdown. Pages name the document by library and document, as codes on
/// the console and docs site or as numbers on www.volcengine.com.
pub(crate) fn volcengine_doc(url: &Url) -> Option<Url> {
    let host = url.host_str()?;
    if host != "volcengine.com" && !host.ends_with(".volcengine.com") {
        return None;
    }
    let segments: Vec<&str> = url.path_segments()?.collect();
    let at = segments.iter().position(|segment| *segment == "docs")?;
    let (library, document) = (*segments.get(at + 1)?, *segments.get(at + 2)?);
    if library.is_empty() || document.is_empty() {
        return None;
    }
    let numbered = [library, document]
        .iter()
        .all(|part| part.bytes().all(|b| b.is_ascii_digit()));
    let (library_key, document_key) = if numbered {
        ("LibraryID", "DocumentID")
    } else {
        ("LibraryCode", "DocumentCode")
    };
    let mut api = Url::parse("https://docs.volcengine.com/api/doc/getDocDetail").ok()?;
    api.query_pairs_mut()
        .append_pair(library_key, library)
        .append_pair(document_key, document)
        .append_pair("type", "online");
    Some(api)
}

#[derive(Deserialize)]
struct Document {
    #[serde(rename = "Result")]
    result: DocumentBody,
}

#[derive(Deserialize)]
struct DocumentBody {
    #[serde(rename = "Title", default)]
    title: String,
    #[serde(rename = "MDContent", default)]
    content: String,
}

/// What a download must be.
#[derive(Clone, Copy)]
enum Body {
    Html,
    Markdown,
    Json,
}

impl Body {
    fn accept(self) -> &'static str {
        match self {
            Self::Html => "text/html,application/xhtml+xml",
            Self::Markdown => "text/markdown,text/plain",
            Self::Json => "application/json",
        }
    }

    fn types(self) -> &'static [&'static str] {
        match self {
            Self::Html => &["text/html", "application/xhtml+xml"],
            Self::Markdown => &["text/markdown", "text/plain"],
            Self::Json => &["application/json"],
        }
    }
}

/// A document on the provider's own domains, retried once, that did not leave them.
async fn official_body(
    state: &AppState,
    url: &str,
    domains: &[String],
    english: bool,
    body: Body,
) -> Result<(Url, String)> {
    let url = Url::parse(url).map_err(|_| AppError::BadRequest("官方页面地址无效".into()))?;
    if !is_public_origin(&url) || !official(&url, domains) {
        return Err(AppError::BadRequest(
            "官方页面必须是服务商域名下的公开 HTTPS 地址".into(),
        ));
    }
    let (url, bytes) = match download(state, url.clone(), english, body).await {
        Ok(page) => page,
        Err(_) => {
            tokio::time::sleep(RETRY_DELAY).await;
            download(state, url, english, body).await?
        }
    };
    if !official(&url, domains) {
        return Err(AppError::BadRequest(
            "官方页面跳转到了服务商以外的域名".into(),
        ));
    }
    Ok((url, String::from_utf8_lossy(&bytes).into_owned()))
}

/// A bounded read on the hardened origin client; it sends no credentials.
async fn download(state: &AppState, url: Url, english: bool, body: Body) -> Result<(Url, Vec<u8>)> {
    let request = state
        .origin_http
        .get(url)
        .timeout(TIMEOUT)
        .header(USER_AGENT, AGENT)
        .header(ACCEPT, body.accept())
        .header(
            ACCEPT_LANGUAGE,
            if english {
                "en-US,en;q=0.9"
            } else {
                "zh-CN,zh;q=0.9"
            },
        );
    let (url, _, bytes) = crate::media::read_bounded(request, body.types(), MAX_BYTES)
        .await
        .map_err(|refused| refused.error("官方页面"))?;
    Ok((url, bytes))
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static selector")
}

/// The main content as text lines, and the official links inside it.
pub(crate) fn read(
    html: &str,
    base: &Url,
    domains: &[String],
    limit: usize,
) -> (Vec<String>, Vec<Link>) {
    let document = Html::parse_document(html);
    // Documentation sites wrap the page in <main>; sidebars and footers sit outside it.
    let mut best = (Vec::new(), Vec::new());
    for name in ["main", "article", "body"] {
        let Some(root) = document.select(&selector(name)).next() else {
            continue;
        };
        let mut writer = Writer::new(base, domains, limit);
        writer.walk(root, 0);
        writer.flush();
        let chars: usize = writer.lines.iter().map(|line| line.chars().count()).sum();
        if chars >= MIN_CHARS {
            return (writer.lines, writer.links);
        }
        if writer.lines.len() > best.0.len() {
            best = (writer.lines, writer.links);
        }
    }
    best
}

struct Writer<'a> {
    base: &'a Url,
    domains: &'a [String],
    limit: usize,
    chars: usize,
    lines: Vec<String>,
    line: String,
    links: Vec<Link>,
    seen: HashSet<String>,
}

impl<'a> Writer<'a> {
    fn new(base: &'a Url, domains: &'a [String], limit: usize) -> Self {
        Self {
            base,
            domains,
            limit,
            chars: 0,
            lines: Vec::new(),
            line: String::new(),
            links: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn full(&self) -> bool {
        self.chars >= self.limit
    }

    fn flush(&mut self) {
        let line = collapse(&self.line);
        self.line.clear();
        if !line.is_empty() && !self.full() {
            self.chars += line.chars().count() + 1;
            self.lines.push(line);
        }
    }

    fn walk(&mut self, element: ElementRef<'_>, depth: usize) {
        for child in element.children() {
            if self.full() || depth > MAX_DEPTH {
                return;
            }
            match child.value() {
                Node::Text(text) => self.line.push_str(text),
                Node::Element(_) => {
                    let Some(child) = ElementRef::wrap(child) else {
                        continue;
                    };
                    let name = child.value().name();
                    if SKIPPED.contains(&name) {
                        continue;
                    }
                    if name == "a" {
                        self.link(child);
                    }
                    match name {
                        "table" => self.table(child, depth),
                        "br" => self.flush(),
                        _ if BLOCKS.contains(&name) => {
                            self.flush();
                            match name {
                                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                                    let level = usize::from(name.as_bytes()[1] - b'0');
                                    self.line.push_str(&"#".repeat(level));
                                    self.line.push(' ');
                                }
                                "li" => self.line.push_str("- "),
                                _ => {}
                            }
                            self.walk(child, depth + 1);
                            self.flush();
                        }
                        _ => {
                            self.line.push(' ');
                            self.walk(child, depth + 1);
                            self.line.push(' ');
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Each row becomes `| cell | cell |`, so a row and its column order stay together.
    fn table(&mut self, table: ElementRef<'_>, depth: usize) {
        self.flush();
        for row in rows(table) {
            if self.full() {
                return;
            }
            let cells: Vec<String> = row
                .children()
                .filter_map(ElementRef::wrap)
                .filter(|cell| matches!(cell.value().name(), "td" | "th"))
                .map(|cell| {
                    for link in cell.select(&selector("a")) {
                        self.link(link);
                    }
                    let mut text = String::new();
                    inline(cell, &mut text, depth + 1);
                    collapse(&text)
                })
                .collect();
            if cells.iter().any(|cell| !cell.is_empty()) {
                self.line = format!("| {} |", cells.join(" | "));
                self.flush();
            }
        }
    }

    /// One finished line of text.
    fn push(&mut self, line: &str) {
        self.line = line.to_owned();
        self.flush();
    }

    fn link(&mut self, anchor: ElementRef<'_>) {
        let Some(href) = anchor.value().attr("href") else {
            return;
        };
        let mut text = String::new();
        inline(anchor, &mut text, 0);
        let mut text = collapse(&text);
        if text.is_empty() {
            text = collapse(
                anchor
                    .value()
                    .attr("aria-label")
                    .or_else(|| anchor.value().attr("title"))
                    .unwrap_or(""),
            );
        }
        self.add_link(&text, href);
    }

    /// Keeps a link on the provider's own domains, once per address.
    fn add_link(&mut self, text: &str, href: &str) {
        if self.links.len() >= MAX_LINKS || text.is_empty() {
            return;
        }
        let Some(url) = self
            .base
            .join(href)
            .ok()
            .filter(|url| is_public_origin(url) && official(url, self.domains))
        else {
            return;
        };
        let url = url.to_string();
        if self.seen.insert(url.clone()) {
            self.links.push(Link {
                text: text.chars().take(120).collect(),
                url,
            });
        }
    }
}

/// Rows that belong to this table, not to a table nested in one of its cells.
fn rows(table: ElementRef<'_>) -> Vec<ElementRef<'_>> {
    let mut rows = Vec::new();
    for child in table.children().filter_map(ElementRef::wrap) {
        match child.value().name() {
            "tr" => rows.push(child),
            "thead" | "tbody" | "tfoot" => rows.extend(
                child
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|row| row.value().name() == "tr"),
            ),
            _ => {}
        }
    }
    rows
}

/// Cell text with block boundaries as spaces, so stacked values stay apart.
fn inline(element: ElementRef<'_>, out: &mut String, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    for child in element.children() {
        match child.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(child) {
                    let name = child.value().name();
                    if SKIPPED.contains(&name) {
                        continue;
                    }
                    out.push(' ');
                    inline(child, out, depth + 1);
                    out.push(' ');
                }
            }
            _ => {}
        }
    }
}

/// Whitespace, including the non-breaking and zero-width kinds, as single spaces.
pub(crate) fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() || matches!(c, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}') {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

/// Links from all of a provider's pages, numbered once for the model.
pub(crate) fn merged_links(pages: &[Page]) -> Vec<Link> {
    let mut seen = HashSet::new();
    pages
        .iter()
        .flat_map(|page| &page.links)
        .filter(|link| seen.insert(link.url.clone()))
        .take(MAX_LINKS)
        .cloned()
        .collect()
}
