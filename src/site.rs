//! The shell shared by the server-rendered public pages: head tags, language alternates and
//! the topic tabs, which the news page (`home`) draws from here too.
use crate::{
    catalog::Section,
    digest::{Rendered, TopicMeta, conditional},
    error::{AppError, Result},
    reading::Reader,
    state::AppState,
};
use axum::{
    Router,
    extract::{RawQuery, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use chrono::{DateTime, Utc};
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd, TextMergeStream, html};
use sqlx::{Postgres, QueryBuilder};
use std::{fmt::Write, sync::LazyLock};
use uuid::Uuid;

/// Topics with their own server-rendered page instead of the news feed.
const PAGE_TOPICS: [&str; 2] = ["reads", "llm"];
/// Where the tabs of 调研, 分享, 时代 and 追踪 stand among the topics': after those whose sort
/// weight is at most this, so an administrator moves a topic before or after them by its weight.
pub(crate) const ARTICLES_WEIGHT: i32 = 42;

/// What every public page loads, with the versions that refresh caches.
const STYLES: &str = "/styles.css?v=margins";
const PUBLIC_SCRIPT: &str = "/public.js?v=12";
const STATS_SCRIPT: &str = "/stats.js?v=3";
/// How the news page introduces the site where it is found and shared.
const TITLE: &str = "𝑵𝒆𝒘𝒔 · AI、开发者与前沿科技的全网热点";
const DESCRIPTION: &str = "面向 AI 实践者、软硬件开发者、开源爱好者和极客，每天精选全网科技热点、开发者热议、安全与创客动态、前沿进展，提供 XAI 日报与 XABC 长文。";
/// app.js's first page of 𝐀𝐋𝐋 and of one topic, and the days it lists, which its preloads repeat.
const ALL_PAGE_LIMIT: &str = "6";
const PAGE_LIMIT: &str = "12";
const DAYS_LIMIT: &str = "366";

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(home))
        .layer(tower_http::compression::CompressionLayer::new())
}

/// The news page, whose feed app.js draws. Its bar is every page's, in the address's language,
/// so the strip is whole in the first frame and the switch opens the page in the other language.
/// Readers, signed in or not, and administrators get the same page; hidden topics have no tab,
/// and an administrator opens them by address.
async fn home(
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Result<Response> {
    let query = query.unwrap_or_default();
    // The first value of a parameter, as app.js reads it; an empty one counts as none.
    let param = |name: &str| {
        url::form_urlencoded::parse(query.as_bytes())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .filter(|value| !value.is_empty())
    };
    let en = param("lang").as_deref() == Some("en");
    let topic = param("topic").unwrap_or_default();
    let topics = state.digest.topics();
    // This address in the other language.
    let mut other = url::form_urlencoded::Serializer::new(String::new());
    other.extend_pairs(
        url::form_urlencoded::parse(query.as_bytes()).filter(|(key, _)| key != "lang"),
    );
    if !en {
        other.append_pair("lang", "en");
    }
    let other = match other.finish() {
        other if other.is_empty() => "/".to_owned(),
        other => format!("/?{other}"),
    };
    let dated = param("day").is_some();
    let html = format!(
        r##"<!doctype html>
<html lang="{lang}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="description" content="{DESCRIPTION}">
  <meta name="theme-color" media="(prefers-color-scheme: light)" content="#f6f6f4">
  <meta name="theme-color" media="(prefers-color-scheme: dark)" content="#0f1115">
  <meta name="referrer" content="no-referrer">
  <!-- How a link to the home page looks where it is shared, X first. -->
  <meta property="og:type" content="website">
  <meta property="og:site_name" content="𝑵𝒆𝒘𝒔">
  <meta property="og:title" content="{TITLE}">
  <meta property="og:description" content="{DESCRIPTION}">
  <meta property="og:url" content="{home}">
  <meta property="og:image" content="{card}">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta name="twitter:card" content="summary_large_image">
  <title>{title}</title>
  <link rel="icon" type="image/svg+xml" href="/icon.svg">
  <link rel="alternate" type="application/atom+xml" title="𝑵𝒆𝒘𝒔" href="/feed.xml">
  <link rel="stylesheet" href="{STYLES}">
  <link rel="stylesheet" href="/catalog.css?v=9">
  {preloads}
</head>
<body class="public feed">
  {topbar}
  <main class="shell">
    <div class="broadcast-strip" data-broadcast-url="https://broadcast.xairouter.com/feed_news?mode=sequence" hidden>
      <a class="broadcast-strip__value"></a>
    </div>
    <div id="status" class="status" role="status" aria-live="polite"></div>
    <section id="worth" class="worth" hidden></section>
    <div id="digest" class="digest"></div>
    <div id="loadMore" class="load-more" role="status" aria-live="polite"></div>
  </main>
  {back_to_top}
  <script src="{STATS_SCRIPT}" data-track-url="{track_url}" defer></script>
  <script src="{PUBLIC_SCRIPT}" defer></script>
  <script src="/travel.js?v=13" defer></script>
  <script src="/app.js?v=59" defer></script>
  <!-- async: a slow third-party banner must not hold back DOMContentLoaded, which starts app.js. -->
  <script src="https://broadcast.xairouter.com/embed.js" async></script>
</body>
</html>
"##,
        lang = choose(en, "zh-CN", "en"),
        title = choose(en, TITLE, "𝑵𝒆𝒘𝒔"),
        home = escape(&absolute(&state, "/")),
        card = escape(&absolute(&state, "/og.png")),
        preloads = preloads(&topics, &topic, dated),
        topbar = topbar(&topics, &topic, en, &other),
        back_to_top = back_to_top(en),
        track_url = crate::reads::TRACK_URL,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// The news page's first requests, sent off with its HTML instead of after app.js has run. They
/// must match app.js exactly (PAGE_LIMIT, ALL_PAGE_LIMIT, DAYS_LIMIT and the order `search()`
/// writes); `crossorigin` gives them fetch()'s credentials mode, which is what makes the browser
/// reuse them. Signed-in readers and administrators read elsewhere and leave them unused; a
/// dated address gets none. Topic ids are slugs, safe in a query as they are.
fn preloads(topics: &[TopicMeta], topic: &str, dated: bool) -> String {
    let link = |path: &str, first: bool| {
        format!(
            "<link rel=\"preload\" href=\"{}\" as=\"fetch\" crossorigin{}>",
            escape(path),
            if first { " fetchpriority=\"high\"" } else { "" }
        )
    };
    let feed = |query: &str, limit: &str| {
        link(&format!("/digest?{query}limit={limit}"), true)
            + &link(&format!("/days?{query}limit={DAYS_LIMIT}"), false)
    };
    match topic {
        _ if dated => String::new(),
        "" => feed("", ALL_PAGE_LIMIT),
        "insight" => link("/insight", true),
        // Pages of their own, and topics readers can't see, have nothing here.
        _ if topics.iter().any(|known| known.id == topic)
            && !PAGE_TOPICS.contains(&topic)
            && Section::of(topic).is_none() =>
        {
            feed(&format!("topic={topic}&"), PAGE_LIMIT)
        }
        _ => String::new(),
    }
}

/// What differs between the server-rendered sections.
pub(crate) struct Chrome<'a> {
    /// The topic whose tab is marked as current.
    pub(crate) current: &'a str,
    /// Extra class on `<body>` for section styles.
    pub(crate) section: &'a str,
    pub(crate) stylesheet: &'a str,
    pub(crate) script: &'a str,
    /// Title and path of the section's Atom feed.
    pub(crate) feed: (&'a str, &'a str),
}

/// Where the next page of a list starts: the last item's `(first_seen_at, id)`. Lists run
/// newest first, and a cursor travels as `{micros}_{id}`.
pub(crate) type After = (DateTime<Utc>, Uuid);

pub(crate) fn parse_after(value: &str) -> Result<After> {
    let invalid = || AppError::BadRequest("分页参数无效".into());
    let (time, id) = value.split_once('_').ok_or_else(invalid)?;
    let time = time
        .parse()
        .ok()
        .and_then(DateTime::from_timestamp_micros)
        .ok_or_else(invalid)?;
    Ok((time, Uuid::parse_str(id).map_err(|_| invalid())?))
}

/// Ends an `items` query with the page condition, the order and one row beyond `limit`.
pub(crate) fn push_page(query: &mut QueryBuilder<Postgres>, after: Option<After>, limit: usize) {
    if let Some((time, id)) = after {
        query
            .push(" AND (items.first_seen_at, items.id) < (")
            .push_bind(time)
            .push(", ")
            .push_bind(id)
            .push(")");
    }
    query
        .push(" ORDER BY items.first_seen_at DESC, items.id DESC LIMIT ")
        .push_bind(i64::try_from(limit).unwrap_or(i64::MAX) + 1);
}

/// Drops the extra row `push_page` asked for and returns the next page's cursor, if any.
pub(crate) fn next_page<T>(
    rows: &mut Vec<T>,
    limit: usize,
    key: impl Fn(&T) -> After,
) -> Option<String> {
    (rows.len() > limit).then(|| {
        rows.truncate(limit);
        let (time, id) = key(&rows[limit - 1]);
        format!("{}_{id}", time.timestamp_micros())
    })
}

pub(crate) fn choose<'a>(en: bool, zh: &'a str, english: &'a str) -> &'a str {
    if en && !english.is_empty() {
        english
    } else {
        zh
    }
}

/// A topic's name in the page's language, as the navigation shows it. Pages are served only
/// for topics the reader may see, so the id stands in only if the topic is gone.
pub(crate) fn topic_name(state: &AppState, reader: Reader, id: &str, en: bool) -> String {
    state
        .digest
        .topics_for(reader)
        .iter()
        .find(|topic| topic.id == id)
        .map_or_else(
            || id.to_owned(),
            |topic| choose(en, &topic.name, &topic.name_en).to_owned(),
        )
}

/// A search reads this many characters of its words.
pub(crate) const SEARCH_CHARS: usize = 40;

/// The words a search box sent, trimmed and cut to `SEARCH_CHARS`.
pub(crate) fn search_words(q: Option<&str>) -> String {
    q.unwrap_or_default()
        .trim()
        .chars()
        .take(SEARCH_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// An ILIKE pattern finding `words` anywhere; ILIKE's own wildcards in them match literally.
pub(crate) fn containing(words: &str) -> String {
    format!(
        "%{}%",
        words
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}

/// A search box sending its words to `action` as `q`, in the page's language.
pub(crate) fn search_box(class: &str, action: &str, words: &str, hint: &str, en: bool) -> String {
    format!(
        "<form class={class} action=\"{}\" role=search><input type=search name=q value=\"{}\" maxlength={SEARCH_CHARS} placeholder=\"{hint}\" aria-label=\"{hint}\" enterkeyhint=search>{}</form>",
        escape(action),
        escape(words),
        if en {
            "<input type=hidden name=lang value=en>"
        } else {
            ""
        }
    )
}

pub(crate) fn localized(path: &str, en: bool) -> String {
    format!(
        "{path}{}",
        if en {
            if path.contains('?') {
                "&lang=en"
            } else {
                "?lang=en"
            }
        } else {
            ""
        }
    )
}

/// The address of a list's page, one author's with `by`, a search's with `words`.
pub(crate) fn listing(path: &str, page: u32, en: bool, by: Option<i64>, words: &str) -> String {
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(by) = by {
        query.append_pair("by", &by.to_string());
    }
    if !words.is_empty() {
        query.append_pair("q", words);
    }
    if page > 1 {
        query.append_pair("page", &page.to_string());
    }
    let query = query.finish();
    localized(
        &if query.is_empty() {
            path.into()
        } else {
            format!("{path}?{query}")
        },
        en,
    )
}

/// Links to a list's neighbouring pages, the older ones under `older` in English; the page script
/// follows the next one as readers scroll.
pub(crate) fn pagination(
    path: &str,
    page: u32,
    more: bool,
    en: bool,
    by: Option<i64>,
    words: &str,
    older: &str,
) -> String {
    let mut nav = "<nav class=reads-pagination aria-label=Pagination>".to_owned();
    if page > 1 {
        let _ = write!(
            nav,
            "<a rel=prev href=\"{}\">{}</a>",
            escape(&listing(path, page - 1, en, by, words)),
            choose(en, "上一页", "Newer")
        );
    }
    if more {
        let _ = write!(
            nav,
            "<a rel=next href=\"{}\">{}</a>",
            escape(&listing(path, page + 1, en, by, words)),
            choose(en, "继续阅读", older)
        );
    }
    nav.push_str("</nav>");
    nav
}

pub(crate) fn absolute(state: &AppState, path: &str) -> String {
    format!(
        "{}{}",
        state.config.public_base_url.as_str().trim_end_matches('/'),
        path
    )
}

pub(crate) fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A page's headline, escaped. A Chinese or Japanese headline is cut after its punctuation into
/// phrases that wrap as wholes (`.phrase` in reads.css), so a line ends at "：" or "，" whenever
/// the next phrase fits instead of splitting a phrase; one without such punctuation stays as it is.
pub(crate) fn headline(title: &str) -> String {
    let pause = |c: char| matches!(c, '，' | '：' | '；' | '、' | '！' | '？' | '。');
    let mut phrases = Vec::new();
    let mut start = 0;
    let mut chars = title.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if pause(c) && !chars.peek().is_some_and(|&(_, next)| pause(next)) {
            phrases.push(&title[start..at + c.len_utf8()]);
            start = at + c.len_utf8();
        }
    }
    phrases.push(&title[start..]);
    phrases.retain(|phrase| !phrase.trim().is_empty());
    if phrases.len() < 2 {
        return escape(title);
    }
    phrases
        .iter()
        .map(|phrase| format!("<span class=phrase>{}</span>", escape(phrase)))
        .collect()
}

/// The eye before a read count.
pub(crate) const EYE: &str = r#"<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/></svg>"#;

/// Where a Markdown picture loads from, and its size when known.
pub(crate) struct Picture {
    pub(crate) src: String,
    pub(crate) size: Option<(i32, i32)>,
}

/// Where the marks that stand for formulas set apart start: a private-use plane no text uses.
/// A formula's mark is this plus its number.
const FORMULA: u32 = 0xF0000;
/// The longest formula math-core is given. It parses nested groups by recursion, a level for each
/// `{` at worst: 1,000 of them take some 1.5 MiB of a worker's 2 MiB stack.
const FORMULA_LENGTH: usize = 1000;

/// Markdown parsed with its LaTeX formulas, `\(…\)` within a line and `\[…\]` set apart, as
/// `InlineMath` and `DisplayMath` for `markdown` to lay out. Each formula outside code, links and
/// pictures becomes one mark before parsing, so neither Markdown's escapes nor a table's bars
/// reach it. `$` delimits nothing: Chinese writes prices with it, `$1.46→$2.06`.
pub(crate) fn parsed(source: &str, options: Options) -> Vec<Event<'static>> {
    // Code keeps every backslash as written, and so do links and their definitions, which escape
    // parentheses with them.
    let parser = Parser::new_ext(source, options);
    let mut kept: Vec<_> = parser
        .reference_definitions()
        .iter()
        .map(|(_, definition)| definition.span.clone())
        .collect();
    kept.extend(parser.into_offset_iter().filter_map(|(event, range)| {
        matches!(
            event,
            Event::Code(_) | Event::Start(Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. })
        )
        .then_some(range)
    }));
    kept.sort_unstable_by_key(|range| range.start);
    let mut kept = kept.into_iter().peekable();
    let bytes = source.as_bytes();
    let mut found: Vec<Option<(&str, bool)>> = Vec::new();
    let mut marked = String::with_capacity(source.len());
    let mut copied = 0;
    // The formula being read: where its TeX starts and whether it is set apart.
    let mut open: Option<(usize, bool)> = None;
    // One pass, whatever the text: an opener without its closer costs nothing later.
    let mut at = 0;
    while at < bytes.len() {
        match (bytes[at], bytes.get(at + 1), open) {
            // A backslash escaped, or a row ending inside a formula.
            (b'\\', Some(b'\\'), _) => at += 2,
            (b'\\', Some(&kind @ (b'(' | b'[')), None) => {
                while kept.next_if(|range| range.end <= at).is_some() {}
                if !kept.peek().is_some_and(|range| range.contains(&at)) {
                    open = Some((at + 2, kind == b'['));
                }
                at += 2;
            }
            (b'\\', Some(&kind @ (b')' | b']')), Some((start, block)))
                if block == (kind == b']') =>
            {
                let Some(mark) = u32::try_from(found.len())
                    .ok()
                    .and_then(|number| char::from_u32(FORMULA + number))
                else {
                    break;
                };
                marked.push_str(&source[copied..start - 2]);
                marked.push(mark);
                found.push(Some((&source[start..at], block)));
                (copied, open, at) = (at + 2, None, at + 2);
            }
            // A formula ends with its paragraph, or was none: at a line holding nothing visible.
            (b'\n', _, Some(_))
                if bytes[at + 1..]
                    .iter()
                    .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r'))
                    == Some(&b'\n') =>
            {
                (open, at) = (None, at + 1);
            }
            _ => at += 1,
        }
    }
    marked.push_str(&source[copied..]);
    let mut events = Vec::new();
    for event in Parser::new_ext(&marked, options) {
        let Event::Text(text) = event else {
            events.push(event.into_static());
            continue;
        };
        let mut words = String::new();
        for c in text.chars() {
            // Each mark stands for its formula once: one the text forges, as a character or an
            // entity, finds it taken or stays a character.
            let formula = u32::from(c)
                .checked_sub(FORMULA)
                .and_then(|number| found.get_mut(usize::try_from(number).ok()?))
                .and_then(Option::take);
            let Some((tex, block)) = formula else {
                words.push(c);
                continue;
            };
            if !words.is_empty() {
                events.push(Event::Text(std::mem::take(&mut words).into()));
            }
            let tex = CowStr::from(tex.to_owned());
            events.push(if block {
                Event::DisplayMath(tex)
            } else {
                Event::InlineMath(tex)
            });
        }
        if !words.is_empty() {
            events.push(Event::Text(words.into()));
        }
    }
    events
}

/// The commands a formula could define its own with: one that expands to itself nests groups
/// without end, so math-core isn't given a formula that names any. Its own commands that expand,
/// like `\implies`, never recurse.
const DEFINING: [&str; 6] = [
    "\\def",
    "\\gdef",
    "\\let",
    "\\newcommand",
    "\\renewcommand",
    "\\providecommand",
];

/// The elements of a formula that lays out on one line.
const FLAT: [&str; 9] = [
    "math", "mi", "mn", "mo", "mtext", "mspace", "mrow", "mpadded", "mstyle",
];

/// A formula as MathML, if math-core is given it and can read it. An operator name is set as
/// `\mathop\mathrm`: math-core writes `\operatorname`'s into the page unescaped.
fn mathml(tex: &str, block: bool) -> Option<String> {
    static CONVERTER: LazyLock<math_core::LatexToMathML> = LazyLock::new(|| {
        math_core::LatexToMathML::new(math_core::MathCoreConfig::default(), Vec::new())
            .expect("no custom macros to parse")
    });
    if tex.chars().count() > FORMULA_LENGTH || DEFINING.iter().any(|name| tex.contains(name)) {
        return None;
    }
    let display = if block {
        math_core::MathDisplay::Block
    } else {
        math_core::MathDisplay::Inline
    };
    // The longer names first, as each holds the shorter.
    let named = tex
        .replace("\\operatornamewithlimits", "\\mathop\\mathrm")
        .replace("\\operatorname*", "\\mathop\\mathrm")
        .replace("\\operatorname", "\\mathop\\mathrm");
    let converted = CONVERTER.convert_with_local_state(&named, display).ok()?;
    Some(converted.mathml)
}

/// A formula as MathML, which browsers lay out themselves, or else its TeX, as code.
fn formula(tex: &str, block: bool) -> String {
    mathml(tex, block).unwrap_or_else(|| format!("<code>{}</code>", escape(tex.trim())))
}

/// A formula as plain text, for summaries: one that lays out on a line as the characters it
/// shows, `𝐏=m(𝐂−𝐕)`, and one with scripts, fractions or anything else stacked as its TeX.
pub(crate) fn formula_text(tex: &str, block: bool) -> String {
    mathml(tex, block)
        .and_then(|mathml| {
            let mut text = String::new();
            let mut rest = mathml.as_str();
            while let Some((before, after)) = rest.split_once('<') {
                text.push_str(before);
                let (tag, after) = after.split_once('>')?;
                let name = tag.trim_start_matches('/').split([' ', '/']).next()?;
                if !FLAT.contains(&name) {
                    return None;
                }
                // A space of some width; a bare one only stops Firefox spacing a word apart.
                if name == "mspace" && tag.contains("width") {
                    text.push(' ');
                }
                rest = after;
            }
            text.push_str(rest);
            Some(
                text.replace("&InvisibleTimes;", "")
                    .replace("&ApplyFunction;", " ")
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&#39;", "'")
                    .replace("&amp;", "&"),
            )
        })
        .unwrap_or_else(|| tex.trim().to_owned())
}

/// Markdown as HTML that can't run anything: raw HTML is dropped, links keep only http(s)
/// addresses without credentials, and a picture shows only where `picture` gives it an
/// address; otherwise its description stays as text. Formulas (`parsed`) come out as MathML,
/// which math-core writes with its text escaped.
pub(crate) fn markdown<'a>(
    events: impl IntoIterator<Item = Event<'a>>,
    picture: impl Fn(&str) -> Option<Picture>,
) -> String {
    let mut image = None;
    let mut alt = String::new();
    let parser = bolded(events).into_iter().filter_map(|event| match event {
        Event::Html(_) | Event::InlineHtml(_) => None,
        Event::Start(Tag::Image { dest_url, .. }) => {
            alt.clear();
            image = Some(picture(&dest_url));
            None
        }
        Event::End(TagEnd::Image) => Some(match image.take().flatten() {
            Some(Picture { src, size }) => {
                let size = size
                    .map(|(width, height)| format!(" width={width} height={height}"))
                    .unwrap_or_default();
                Event::Html(
                    format!(
                        "<img src=\"{}\" alt=\"{}\"{size} loading=lazy decoding=async referrerpolicy=no-referrer>",
                        escape(&src),
                        escape(&alt)
                    )
                    .into(),
                )
            }
            None => Event::Text(std::mem::take(&mut alt).into()),
        }),
        Event::Text(text) | Event::Code(text) if image.is_some() => {
            alt.push_str(&text);
            None
        }
        _ if image.is_some() => None,
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe = url::Url::parse(&dest_url).is_ok_and(|url| {
                matches!(url.scheme(), "https" | "http")
                    && url.username().is_empty()
                    && url.password().is_none()
            });
            Some(Event::Start(Tag::Link {
                link_type,
                dest_url: if safe { dest_url } else { "".into() },
                title,
                id,
            }))
        }
        Event::InlineMath(tex) => Some(Event::InlineHtml(formula(&tex, false).into())),
        Event::DisplayMath(tex) => Some(Event::InlineHtml(formula(&tex, true).into())),
        other => Some(other),
    });
    let mut html = String::new();
    html::push_html(&mut html, parser);
    html
}

/// CommonMark leaves `**` as text when punctuation is just inside it and a letter just outside,
/// as in `一起玩。**把` or `看**“引号”**里`: it expects a space there, and Chinese writes none.
/// Within a run of plain text such stars are paired in order and become bold where punctuation
/// at the edge is what kept them apart; stars around letters were escaped and stay.
pub(crate) fn bolded<'a>(input: impl IntoIterator<Item = Event<'a>>) -> Vec<Event<'a>> {
    let mut events = Vec::new();
    let mut code = false;
    for event in TextMergeStream::new(input.into_iter()) {
        match &event {
            Event::Start(Tag::CodeBlock(_)) => code = true,
            Event::End(TagEnd::CodeBlock) => code = false,
            Event::Text(text) if !code && text.contains("**") => {
                let pieces: Vec<&str> = text.split("**").collect();
                events.push(Event::Text(pieces[0].to_string().into()));
                for pair in pieces[1..].chunks(2) {
                    match pair {
                        [inside, outside] if refused(inside) => events.extend([
                            Event::Start(Tag::Strong),
                            Event::Text(inside.to_string().into()),
                            Event::End(TagEnd::Strong),
                            Event::Text(outside.to_string().into()),
                        ]),
                        _ => events.push(Event::Text(format!("**{}", pair.join("**")).into())),
                    }
                }
                continue;
            }
            _ => {}
        }
        events.push(event);
    }
    events
}

/// Whether CommonMark refused bold around `inside` only for punctuation at its edge: the text
/// touches its stars, is no part of a longer run of stars, and starts or ends with punctuation.
fn refused(inside: &str) -> bool {
    let (Some(first), Some(last)) = (inside.chars().next(), inside.chars().next_back()) else {
        return false;
    };
    [first, last]
        .iter()
        .all(|edge| !edge.is_whitespace() && *edge != '*')
        && [first, last].iter().any(|edge| !edge.is_alphanumeric())
}

/// The topic tabs of every public page, the news page's included: 𝐀𝐋𝐋 and 𝐀𝐈, then the site's
/// visible topics by sort weight, each catalogue section as one tab, with 调研 and 分享 at
/// `ARTICLES_WEIGHT`. `current` is the page's topic, "" on the news page's 𝐀𝐋𝐋.
pub(crate) fn tabs(topics: &[TopicMeta], current: &str, en: bool) -> String {
    let marked = |id: &str| {
        if current == id {
            " aria-current=\"true\""
        } else {
            ""
        }
    };
    let mut tabs = format!(
        "<a class=tab href=\"{}\"{}>𝐀𝐋𝐋</a><a class=tab href=\"{}\"{}>𝐀𝐈</a>",
        localized("/", en),
        marked(""),
        escape(&localized("/?topic=insight", en)),
        marked("insight")
    );
    // 调研 and 分享 are not topics: readers ask for and write them. Their tabs stand at
    // `ARTICLES_WEIGHT`, and XABC's 时代 and 追踪 follow them.
    let mut articles_tab = Some(format!(
        "<a class=tab href=\"{}\"{}>{}</a><a class=tab href=\"{}\"{}>{}</a><a class=tab href=\"{}\"{}>{}</a><a class=tab href=\"{}\"{}>{}</a>",
        localized(crate::articles::research::PATH, en),
        marked("research"),
        crate::articles::research::label(en),
        localized("/articles", en),
        marked("articles"),
        crate::articles::label(en),
        localized(crate::articles::PEOPLE, en),
        marked("people"),
        crate::articles::people_label(en),
        localized(crate::articles::TRACKER, en),
        marked("tracker"),
        crate::articles::tracker_label(en)
    ));
    // A catalogue section's topics share one tab; swiping back into it enters the last
    // (public.js).
    let mut sections = crate::catalog::Section::ALL.map(|section| {
        let last = section
            .topics()
            .iter()
            .rev()
            .find(|topic| topics.iter().any(|known| known.id == **topic));
        (section, last)
    });
    for topic in topics.iter() {
        if topic.sort_weight > ARTICLES_WEIGHT
            && let Some(articles) = articles_tab.take()
        {
            tabs.push_str(&articles);
        }
        let id = &topic.id;
        if let Some(section) = crate::catalog::Section::of(id) {
            if let Some(last) = sections
                .iter_mut()
                .find(|(known, _)| *known == section)
                .and_then(|(_, last)| last.take())
            {
                let path = format!("/{}", section.path());
                let _ = write!(
                    tabs,
                    "<a class=tab href=\"{}\" data-last=\"{}\"{}>{}</a>",
                    escape(&localized(&path, en)),
                    escape(&localized(&format!("{path}/{last}"), en)),
                    if crate::catalog::Section::of(current) == Some(section) {
                        " aria-current=\"true\""
                    } else {
                        ""
                    },
                    section.label(en)
                );
            }
            continue;
        }
        // Codex & Claude's tab opens the handbook, which leads to the topic's feed here; there
        // app.js marks it by its topic.
        let handbook = id == crate::articles::handbook::TOPIC;
        let href = if PAGE_TOPICS.contains(&id.as_str()) {
            localized(&format!("/{id}"), en)
        } else if handbook {
            localized(crate::articles::handbook::PATH, en)
        } else {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query.append_pair("topic", id);
            if en {
                query.append_pair("lang", "en");
            }
            format!("/?{}", query.finish())
        };
        let _ = write!(
            tabs,
            "<a class=tab href=\"{}\"{}{}>{}</a>",
            escape(&href),
            if handbook {
                format!(" data-topic={id}")
            } else {
                String::new()
            },
            marked(id),
            escape(choose(en, &topic.name, &topic.name_en))
        );
    }
    if let Some(articles) = articles_tab {
        tabs.push_str(&articles);
    }
    tabs
}

/// A server-rendered page. Its tabs are the public catalogue's for every reader: a preview shows
/// an administrator the page, not the hidden topics' tabs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout(
    state: &AppState,
    en: bool,
    path: &str,
    title: &str,
    description: &str,
    content: &str,
    structured: Option<serde_json::Value>,
    chrome: &Chrome<'_>,
) -> String {
    // An article's picture in its structured data also stands for it where the page is shared.
    let card = structured
        .as_ref()
        .and_then(|value| value["image"].as_str())
        .map(|image| {
            format!(
                "<meta property=\"og:image\" content=\"{}\"><meta name=\"twitter:card\" content=\"summary_large_image\">",
                escape(image)
            )
        })
        .unwrap_or_else(|| "<meta name=\"twitter:card\" content=\"summary\">".into());
    let schema = structured
        .map(|value| {
            format!(
                "<script type=\"application/ld+json\">{}</script>",
                value
                    .to_string()
                    .replace('&', "\\u0026")
                    .replace('<', "\\u003c")
                    .replace('>', "\\u003e")
            )
        })
        .unwrap_or_default();
    format!(
        r##"<!doctype html>
<html lang="{lang}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title><meta name="description" content="{description}"><meta name="theme-color" media="(prefers-color-scheme: light)" content="#f6f6f4"><meta name="theme-color" media="(prefers-color-scheme: dark)" content="#0f1115">
<link rel="canonical" href="{canonical}"><link rel="alternate" hreflang="zh-CN" href="{zh}"><link rel="alternate" hreflang="en" href="{english}"><link rel="alternate" hreflang="x-default" href="{zh}">
<meta property="og:type" content="{og_type}"><meta property="og:title" content="{title}"><meta property="og:description" content="{description}"><meta property="og:url" content="{canonical}"><meta property="og:site_name" content="News">{card}
<link rel="icon" type="image/svg+xml" href="/icon.svg"><link rel="alternate" type="application/atom+xml" title="{feed_title}" href="{feed_href}"><link rel="stylesheet" href="{STYLES}"><link rel="stylesheet" href="{stylesheet}">{schema}</head>
<body class="public {section}">{topbar}
<main class="shell">{content}</main>
{back_to_top}
<script src="{PUBLIC_SCRIPT}" defer></script><script src="{script}" defer></script><script src="{STATS_SCRIPT}" data-track-url="{track_url}" defer></script></body></html>"##,
        lang = choose(en, "zh-CN", "en"),
        title = escape(title),
        description = escape(description),
        canonical = escape(&absolute(state, &localized(path, en))),
        zh = escape(&absolute(state, path)),
        english = escape(&absolute(state, &localized(path, true))),
        og_type = if schema.is_empty() {
            "website"
        } else {
            "article"
        },
        feed_title = escape(chrome.feed.0),
        feed_href = escape(chrome.feed.1),
        stylesheet = escape(chrome.stylesheet),
        section = chrome.section,
        script = escape(chrome.script),
        topbar = topbar(
            &state.digest.topics(),
            chrome.current,
            en,
            &localized(path, !en)
        ),
        back_to_top = back_to_top(en),
        track_url = crate::reads::TRACK_URL,
    )
}

/// The bar atop every public page: the brand, the topic tabs, the workspace, the switch to
/// `other` (this page in the other language) and the source on GitHub.
fn topbar(topics: &[TopicMeta], current: &str, en: bool, other: &str) -> String {
    format!(
        r##"<header class="topbar"><a class="brand" href="{home}" aria-label="News"><img class="brand-logo" src="/icon.svg" alt=""><span>𝑵𝒆𝒘𝒔</span></a><nav class="tabs" aria-label="{topics_label}">{tabs}</nav><div class="tools"><a class="space-link button button-quiet button-small" href="/me/">{workspace}</a><a class="space-link button button-quiet button-small" href="{other_lang}" hreflang="{other_code}">{language_label}</a><a class="source-link button button-quiet button-small" href="https://github.com/xabcos/news" target="_blank" rel="noopener noreferrer" aria-label="{source}" title="{source}"><svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 19c-5 1.5-5-2.5-7-3m14 6v-3.87a3.37 3.37 0 0 0-.94-2.61c3.14-.35 6.44-1.54 6.44-7A5.44 5.44 0 0 0 20 4.77 5.07 5.07 0 0 0 19.91 1S18.73.65 16 2.48a13.38 13.38 0 0 0-7 0C6.27.65 5.09 1 5.09 1A5.07 5.07 0 0 0 5 4.77a5.44 5.44 0 0 0-1.5 3.78c0 5.42 3.3 6.61 6.44 7A3.37 3.37 0 0 0 9 18.13V22"/></svg></a></div></header>"##,
        home = localized("/", en),
        topics_label = choose(en, "主题", "Topics"),
        tabs = tabs(topics, current, en),
        workspace = choose(en, "工作台", "Workspace"),
        other_lang = escape(other),
        other_code = choose(en, "en", "zh-CN"),
        language_label = choose(en, "EN", "中文"),
        source = choose(en, "开源代码 · AGPL-3.0", "Source on GitHub · AGPL-3.0"),
    )
}

/// The button back to the top, which public.js shows once a reader has scrolled away from it.
fn back_to_top(en: bool) -> String {
    format!(
        r##"<button id="backToTop" class="button back-to-top" type="button" aria-label="{back_to_top}" title="{back_to_top}"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 12 6-6 6 6M12 6v12"/></svg></button>"##,
        back_to_top = choose(en, "回到顶部", "Back to top"),
    )
}

#[cfg(test)]
mod tests {
    use super::{TopicMeta, headline, markdown, parsed, preloads, tabs};
    use pulldown_cmark::{Options, Parser};

    fn topic(id: &str, name: &str, name_en: &str, sort_weight: i32) -> TopicMeta {
        TopicMeta {
            id: id.into(),
            name: name.into(),
            name_en: name_en.into(),
            description: String::new(),
            visible: true,
            sort_weight,
        }
    }

    fn topics() -> [TopicMeta; 7] {
        [
            topic("x", "要闻", "𝐗", 0),
            topic("overseas-ai", "海外 AI", "𝑶", 30),
            topic("security", "安全", "𝑺𝒆𝒄𝒖𝒓𝒊𝒕𝒚", 42),
            topic("github", "开源", "𝑮", 52),
            topic("movie", "影视", "𝑴𝒐𝒗𝒊𝒆", 80),
            topic("book", "图书", "𝑩𝒐𝒐𝒌", 80),
            topic("reads", "精读", "𝑹𝒆𝒂𝒅𝒔", 82),
        ]
    }

    #[test]
    fn every_page_draws_the_same_tabs_and_marks_its_own() {
        assert_eq!(
            tabs(&topics(), "", false),
            concat!(
                "<a class=tab href=\"/\" aria-current=\"true\">𝐀𝐋𝐋</a>",
                "<a class=tab href=\"/?topic=insight\">𝐀𝐈</a>",
                "<a class=tab href=\"/?topic=x\">要闻</a>",
                "<a class=tab href=\"/?topic=overseas-ai\">海外 AI</a>",
                "<a class=tab href=\"/?topic=security\">安全</a>",
                "<a class=tab href=\"/research\">调研</a>",
                "<a class=tab href=\"/articles\">分享</a>",
                "<a class=tab href=\"/people\">时代</a>",
                "<a class=tab href=\"/tracker\">追踪</a>",
                "<a class=tab href=\"/?topic=github\">开源</a>",
                "<a class=tab href=\"/culture\" data-last=\"/culture/book\">文娱</a>",
                "<a class=tab href=\"/reads\">精读</a>",
            )
        );
        let english = tabs(&topics(), "x", true);
        assert!(english.starts_with("<a class=tab href=\"/?lang=en\">𝐀𝐋𝐋</a>"));
        for tab in [
            "<a class=tab href=\"/?topic=x&amp;lang=en\" aria-current=\"true\">𝐗</a>",
            "<a class=tab href=\"/culture?lang=en\" data-last=\"/culture/book?lang=en\">𝑪𝒖𝒍𝒕𝒖𝒓𝒆</a>",
            "<a class=tab href=\"/research?lang=en\">𝑹𝒆𝒔𝒆𝒂𝒓𝒄𝒉</a>",
            "<a class=tab href=\"/people?lang=en\">𝑷𝒆𝒐𝒑𝒍𝒆</a>",
            "<a class=tab href=\"/tracker?lang=en\">𝑻𝒓𝒂𝒄𝒌𝒆𝒓</a>",
        ] {
            assert!(english.contains(tab), "{tab}");
        }
        for (current, tab) in [
            ("insight", ">𝐀𝐈</a>"),
            ("movie", ">文娱</a>"),
            ("research", ">调研</a>"),
            ("people", ">时代</a>"),
            ("tracker", ">追踪</a>"),
            ("reads", ">精读</a>"),
        ] {
            let html = tabs(&topics(), current, false);
            assert_eq!(html.matches("aria-current").count(), 1, "{current}");
            assert!(
                html.contains(&format!(" aria-current=\"true\"{tab}")),
                "{current}"
            );
        }
    }

    #[test]
    fn tabs_keep_their_places_without_their_neighbours() {
        // 调研, 分享, 时代 and 追踪 keep their weight whoever is shown: before every topic weighted
        // more, last after topics weighted no more. Without 图书, swiping back into 文娱 enters 影视.
        // Names are escaped in both languages.
        let articles = concat!(
            "<a class=tab href=\"/research\">调研</a>",
            "<a class=tab href=\"/articles\">分享</a>",
            "<a class=tab href=\"/people\">时代</a>",
            "<a class=tab href=\"/tracker\">追踪</a>",
        );
        let topics = [
            topic("movie", "影视", "𝑴𝒐𝒗𝒊𝒆", 80),
            topic("odd", "A&B \"</template>", "<b>", 90),
        ];
        assert_eq!(
            tabs(&topics, "", false),
            [
                "<a class=tab href=\"/\" aria-current=\"true\">𝐀𝐋𝐋</a>",
                "<a class=tab href=\"/?topic=insight\">𝐀𝐈</a>",
                articles,
                "<a class=tab href=\"/culture\" data-last=\"/culture/movie\">文娱</a>",
                "<a class=tab href=\"/?topic=odd\">A&amp;B &quot;&lt;/template&gt;</a>",
            ]
            .concat()
        );
        assert!(
            tabs(&topics, "", true)
                .contains("<a class=tab href=\"/?topic=odd&amp;lang=en\">&lt;b&gt;</a>")
        );
        let light = [
            topic("x", "要闻", "𝐗", 0),
            topic("security", "安全", "𝑺", 42),
        ];
        assert!(tabs(&light, "", false).ends_with(&format!("安全</a>{articles}")));
        // A section's one tab goes where its first shown topic is, though another of its topics
        // weighs more than the four.
        let split = [
            topic("movie", "影视", "𝑴𝒐𝒗𝒊𝒆", 18),
            topic("x", "要闻", "𝐗", 30),
            topic("book", "图书", "𝑩𝒐𝒐𝒌", 50),
        ];
        assert!(tabs(&split, "", false).ends_with(&format!(
            "data-last=\"/culture/book\">文娱</a><a class=tab href=\"/?topic=x\">要闻</a>{articles}"
        )));
    }

    #[test]
    fn the_news_page_preloads_what_app_js_asks_for_first() {
        let link = |href: &str, first: bool| {
            format!(
                "<link rel=\"preload\" href=\"{href}\" as=\"fetch\" crossorigin{}>",
                if first { " fetchpriority=\"high\"" } else { "" }
            )
        };
        assert_eq!(
            preloads(&topics(), "", false),
            link("/digest?limit=6", true) + &link("/days?limit=366", false)
        );
        assert_eq!(
            preloads(&topics(), "x", false),
            link("/digest?topic=x&amp;limit=12", true)
                + &link("/days?topic=x&amp;limit=366", false)
        );
        assert_eq!(
            preloads(&topics(), "insight", false),
            link("/insight", true)
        );
        // Dated addresses, pages of their own, and topics readers can't see.
        for (topic, dated) in [
            ("", true),
            ("x", true),
            ("reads", false),
            ("movie", false),
            ("articles", false),
            ("w", false),
        ] {
            assert_eq!(preloads(&topics(), topic, dated), "", "{topic}");
        }
        // The preloads only help while they are the very requests app.js makes.
        let script = include_str!("../pages/app.js");
        for constant in [
            "const ALL_PAGE_LIMIT = 6;",
            "const PAGE_LIMIT = 12;",
            "const DAYS_LIMIT = 366;",
        ] {
            assert!(script.contains(constant), "{constant}");
        }
    }

    #[test]
    fn headlines_break_after_their_punctuation_and_stay_escaped() {
        assert_eq!(
            headline("认识 xabcnews.com：让 AI 替你读全网，你只读值得读的"),
            "<span class=phrase>认识 xabcnews.com：</span><span class=phrase>让 AI 替你读全网，</span><span class=phrase>你只读值得读的</span>"
        );
        // Punctuation that runs together stays with the phrase before it.
        assert_eq!(
            headline("真的吗？！<b>不</b>"),
            "<span class=phrase>真的吗？！</span><span class=phrase>&lt;b&gt;不&lt;/b&gt;</span>"
        );
        // One phrase, or none to cut, is left as it is.
        assert_eq!(
            headline("廉价模型正在动摇 <AI> 基建"),
            "廉价模型正在动摇 &lt;AI&gt; 基建"
        );
        assert_eq!(headline("Why it matters: a test"), "Why it matters: a test");
        assert_eq!(headline("结尾的问号？"), "结尾的问号？");
    }

    #[test]
    fn bold_next_to_chinese_punctuation_is_bold() {
        let render = |source: &str| {
            markdown(
                Parser::new_ext(
                    source,
                    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
                ),
                |_| None,
            )
        };
        for (source, html) in [
            (
                "- **孩子年龄越接近，孩子也更常一起玩。**把亲缘关系纳入统计后仍然存在。\n- 看**“引号”**里的话\n- 约 **80 厘米（31 英寸）**的长度",
                "<ul>\n<li><strong>孩子年龄越接近，孩子也更常一起玩。</strong>把亲缘关系纳入统计后仍然存在。</li>\n<li>看<strong>“引号”</strong>里的话</li>\n<li>约 <strong>80 厘米（31 英寸）</strong>的长度</li>\n</ul>\n",
            ),
            (
                "| **甲。**乙 |\n|---|\n| [**丙。**丁](https://a.example/) |",
                "<table><thead><tr><th><strong>甲。</strong>乙</th></tr></thead><tbody>\n<tr><td><a href=\"https://a.example/\"><strong>丙。</strong>丁</a></td></tr>\n</tbody></table>\n",
            ),
            // What CommonMark reads stays as it is.
            (
                "他说：**重要**，正常 **加粗** 文字，****双重****",
                "<p>他说：<strong>重要</strong>，正常 <strong>加粗</strong> 文字，<strong><strong>双重</strong></strong></p>\n",
            ),
            // Stars around letters or spaces, longer runs, code and pairs across blocks stay.
            (
                "x\\*\\*2 + y\\*\\*2，2 ** 3，***粗斜。***的，`**kwargs`",
                "<p>x**2 + y**2，2 ** 3，***粗斜。***的，<code>**kwargs</code></p>\n",
            ),
            (
                "```\n**甲。**乙\n```",
                "<pre><code>**甲。**乙\n</code></pre>\n",
            ),
            ("**甲。\n\n乙。**丙", "<p>**甲。</p>\n<p>乙。**丙</p>\n"),
        ] {
            assert_eq!(render(source), html, "{source}");
        }
    }

    #[test]
    fn formulas_come_out_as_mathml_and_leave_tables_code_links_and_prices_alone() {
        let render = |source: &str| {
            markdown(
                parsed(
                    source,
                    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
                ),
                |_| None,
            )
        };
        // Within a line and set apart, a row's end inside the formula included; no TeX is left.
        let html = render(
            "动量 \\(\\mathbf p=m\\mathbf v\\) 很熟悉：\n\n\\[\n\\begin{aligned}P^2&=a\\\\\n&=b\\end{aligned}\n\\]",
        );
        assert_eq!(html.matches("<math").count(), 2, "{html}");
        assert!(
            html.contains("<math display=\"block\">") && !html.contains('\\'),
            "{html}"
        );
        // A bar inside a formula leaves a table's cells alone.
        let table = render("| 方向 | 长度 |\n|---|---|\n| 垂直 | \\(|\\mathbf C-\\mathbf V|\\) |");
        assert_eq!(
            (
                table.matches("<td>").count(),
                table.matches("<math>").count()
            ),
            (2, 1),
            "{table}"
        );
        for (source, html) in [
            // Code, escaped backslashes and prices stay as written.
            (
                "`\\(a\\)` 与\n\n```\nre.sub(r'\\(\\d+\\)', x)\n```",
                "<p><code>\\(a\\)</code> 与</p>\n<pre><code>re.sub(r'\\(\\d+\\)', x)\n</code></pre>\n",
            ),
            (
                "\\\\(不是公式\\\\) 单价 $1.46→$2.06，（$0.05/分钟）",
                "<p>\\(不是公式\\) 单价 $1.46→$2.06，（$0.05/分钟）</p>\n",
            ),
            // A link escapes its parentheses with backslashes.
            (
                "[水星](https://en.wikipedia.org/wiki/Mercury_\\(planet\\) \"轨道 \\(a\\)\")",
                "<p><a href=\"https://en.wikipedia.org/wiki/Mercury_(planet)\" title=\"轨道 (a)\">水星</a></p>\n",
            ),
            // So does a link's definition, title included.
            (
                "[水星][p]\n\n[p]: https://en.wikipedia.org/wiki/Mercury_\\(planet\\) \"轨道 \\(a\\)\"",
                "<p><a href=\"https://en.wikipedia.org/wiki/Mercury_(planet)\" title=\"轨道 (a)\">水星</a></p>\n",
            ),
            // No formula runs past its paragraph, however its blank line is written.
            ("\\(开头\n  \n结尾\\)", "<p>(开头</p>\n<p>结尾)</p>\n"),
            ("\\(开头\r\n\r\n结尾\\)", "<p>(开头</p>\n<p>结尾)</p>\n"),
            // What math-core can't read, or isn't given, stays as its TeX, escaped, as code: here
            // a formula defining a command that would expand to itself without end.
            (
                "看 \\(\\def\\rec{{{\\rec}}}\\rec\\)",
                "<p>看 <code>\\def\\rec{{{\\rec}}}\\rec</code></p>\n",
            ),
        ] {
            assert_eq!(render(source), html, "{source}");
        }
        // An operator name is laid out with its markup escaped, however a formula reaches it.
        for source in [
            "\\(\\operatorname*{arg\\,max}_x f(x)\\)",
            "\\(\\sqrt\\operatorname{<img/src=x>}\\)",
            "\\(\\sqrt\\mathchoice{a}\\operatorname{b}{c}{<img/src=x>}\\)",
        ] {
            let html = render(source);
            assert!(
                html.starts_with("<p><math>") && !html.contains("<img"),
                "{html}"
            );
        }
        // So are math-core's own commands that expand.
        let expanded = render("\\(a\\implies b\\iff c,\\ x_1,\\ldots,x_n\\equiv 0\\pmod{n}\\)");
        assert!(
            expanded.starts_with("<p><math>") && !expanded.contains('\\'),
            "{expanded}"
        );
        // Nesting deeper than math-core's stack allows is not given to it.
        let deep = render(&format!("\\({}x\\)", "{".repeat(2000)));
        assert!(deep.starts_with("<p><code>{{{") && !deep.contains("<math"));
        // A mark the text forges, as a character or an entity, takes a formula at most once.
        let forged = render(&format!(
            "{} {} \\(\\frac{{1}}{{2}}\\)",
            "\u{F0000}".repeat(50),
            "&#xF0000;".repeat(50)
        ));
        assert_eq!(forged.matches("<math").count(), 1, "{forged}");
    }
}
