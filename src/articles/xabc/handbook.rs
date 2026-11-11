//! Codex & Claude: XABC writes the handbook's chapters (`articles::handbook`) and keeps them in
//! step with the official documentation. Every round reads the official pages the chapters rest
//! on and notes each one's fingerprint. A chapter not yet written is written, one after another
//! until all are, from its pages, what both scouts find on X and the web and the site's stories
//! on its subject. Otherwise one chapter is brought up to date, if any is due: one whose official
//! pages changed since it was last checked, then the one the site gathered the most new stories
//! on, then one unchecked for a week; told what changed, the writer may find nothing to change,
//! and a page it wasn't shown whole stays changed. What a chapter's code names that belongs to the
//! products, their options, variables, model IDs and configuration keys, the official pages must
//! name too: a draft naming others, or JSON that doesn't parse, is written again once. Writing,
//! links and saving are 分享's (`super`); a chapter shows no pictures.
use super::{
    KEEPING, Kind, Line, draft, figures, finish, identity, linked, listed, log, look, publish,
    revise,
};
use crate::{
    articles::{
        MARKDOWN,
        handbook::{self, CHAPTERS, Chapter},
    },
    error::{AppError, Result},
    fetch::Usage,
    reads,
    site::{absolute, containing},
    state::AppState,
};
use chrono::{DateTime, Utc};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::Write,
    sync::Arc,
};
use tokio::{sync::Semaphore, task::JoinSet};
use url::Url;
use uuid::Uuid;

/// Official pages read at once.
const READING: usize = 4;
/// What the writer reads of all a chapter's official pages: those changed since its last check
/// whole, the others their first `PAGE_CHARS`, and past it only their addresses. The longest page
/// fits whole; the writing model reads a million tokens for a dollar.
const OFFICIAL_CHARS: usize = 480_000;
const PAGE_CHARS: usize = 30_000;
/// The site's stories a chapter's writer reads at most, and how far back a new chapter's go.
const STORIES: i64 = 30;
const FIRST_DAYS: i64 = 60;
/// A chapter is brought up to date once this many new stories on its subject came, a day after
/// its last check at least; and checked again a week after, whatever came.
const FRESH: usize = 3;
const FRESH_DAYS: i64 = 1;
const CHECK_DAYS: i64 = 7;
/// Where a chapter's stories come from, and the names each must hold: news, problems and their
/// fixes, open source and long reads on Claude, Codex, DeepSeek and their makers.
const SOURCES: [&str; 5] = [handbook::TOPIC, "issues", "overseas-ai", "github", "reads"];
const PRODUCTS: [&str; 6] = [
    "%claude%",
    "%codex%",
    "%anthropic%",
    "%openai%",
    "%deepseek%",
    "%深度求索%",
];

/// An official page as read this round, with its fingerprint.
struct Page {
    text: String,
    print: String,
}

/// A chapter XABC has written, as it reads now.
#[derive(sqlx::FromRow)]
struct Kept {
    id: Uuid,
    chapter: String,
    title: String,
    body: String,
    revision: i32,
    published: bool,
    updated_at: DateTime<Utc>,
    /// The fingerprint of each official page as the chapter was last checked against it.
    sources: Option<Json<HashMap<String, String>>>,
    checked_at: Option<DateTime<Utc>>,
}

/// A chapter due to be brought up to date: why, by rank, when it was last checked, the official
/// pages changed since and the stories that came.
struct Due<'a> {
    rank: (u8, usize),
    checked: DateTime<Utc>,
    kept: &'a Kept,
    chapter: &'static Chapter,
    changed: Vec<&'static str>,
    stories: Vec<Story>,
}

/// A story the site keeps on a chapter's subject.
#[derive(sqlx::FromRow)]
struct Story {
    title: String,
    summary: String,
    url: String,
    source: String,
    at: DateTime<Utc>,
}

/// When the line's last round ended: the latest check of a chapter, or none while a chapter is
/// still to write, which makes the line due at once.
pub(super) async fn last(state: &AppState) -> Result<Option<DateTime<Utc>>> {
    let keys: Vec<&str> = CHAPTERS.iter().map(|chapter| chapter.key).collect();
    Ok(sqlx::query_scalar(
        "SELECT CASE WHEN count(*) = $2 THEN max(checked_at) END FROM articles
         WHERE kind = 'chapter' AND subject = ANY($1)",
    )
    .bind(&keys)
    .bind(keys.len() as i64)
    .fetch_one(&state.pool)
    .await?)
}

/// Writes every chapter not yet written, or else brings the one most due up to date; none while
/// the topic collects nothing or no chapter is due.
pub(super) async fn write(state: &AppState, author: i64) -> Result<Option<Uuid>> {
    let collecting: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM topics WHERE id = $1 AND enabled)")
            .bind(handbook::TOPIC)
            .fetch_one(&state.pool)
            .await?;
    if !collecting {
        return Ok(None);
    }
    // What came before this moment is the round's material; what comes later, the next round's.
    let cutoff = Utc::now();
    let pages = official(state).await;
    let kept: Vec<Kept> = sqlx::query_as(
        "SELECT id, subject AS chapter, title, body, revision, status = 'published' AS published,
                updated_at, sources, checked_at
         FROM articles WHERE kind = 'chapter'",
    )
    .fetch_all(&state.pool)
    .await?;
    let mut written = None;
    for chapter in CHAPTERS
        .iter()
        .filter(|chapter| !kept.iter().any(|kept| kept.chapter == chapter.key))
    {
        written = Some(create(state, author, chapter, &pages, cutoff).await?);
    }
    if written.is_some() {
        return Ok(written);
    }
    // One chapter a round: one whose official pages changed first, the longest unchecked of
    // them first; then the one with the most new stories, a day after its check; then one
    // unchecked for a week. One an administrator took back stays as it is.
    let mut due: Option<Due> = None;
    for kept in kept.iter().filter(|kept| kept.published) {
        let Some(chapter) = handbook::chapter(&kept.chapter) else {
            continue;
        };
        if !chapter.pages.iter().any(|url| pages.contains_key(url)) {
            continue;
        }
        let stored = kept.sources.as_ref().map(|sources| &sources.0);
        let changed: Vec<&'static str> = chapter
            .pages
            .iter()
            .copied()
            .filter(|url| {
                pages.get(url).is_some_and(|page| {
                    stored.and_then(|stored| stored.get(*url)) != Some(&page.print)
                })
            })
            .collect();
        let checked = kept.checked_at.unwrap_or(kept.updated_at);
        let stories = stories(state, chapter, checked, cutoff).await?;
        let rank = if !changed.is_empty() {
            (0, 0)
        } else if stories.len() >= FRESH && cutoff - checked >= chrono::Duration::days(FRESH_DAYS) {
            (1, usize::MAX - stories.len())
        } else if cutoff - checked >= chrono::Duration::days(CHECK_DAYS) {
            (2, 0)
        } else {
            continue;
        };
        if due
            .as_ref()
            .is_none_or(|due| (rank, checked) < (due.rank, due.checked))
        {
            due = Some(Due {
                rank,
                checked,
                kept,
                chapter,
                changed,
                stories,
            });
        }
    }
    match due {
        Some(due) => update(state, &due, &pages, cutoff).await,
        None => Ok(None),
    }
}

/// Every official page the chapters rest on, read once this round; one that can't be read is
/// left out, and so its chapter isn't taken for changed.
async fn official(state: &AppState) -> HashMap<&'static str, Page> {
    let urls: BTreeSet<&'static str> = CHAPTERS
        .iter()
        .flat_map(|chapter| chapter.pages.iter().copied())
        .collect();
    let turns = Arc::new(Semaphore::new(READING));
    let mut reads = JoinSet::new();
    for url in urls {
        let (state, turns) = (state.clone(), turns.clone());
        reads.spawn(async move {
            let _turn = turns.acquire_owned().await;
            (url, read(&state, url).await)
        });
    }
    let mut pages = HashMap::new();
    let mut missed = Vec::new();
    for (url, page) in reads.join_all().await {
        match page {
            Ok(page) => {
                pages.insert(url, page);
            }
            Err(error) => missed.push(format!("{url}: {error}")),
        }
    }
    if !missed.is_empty() {
        tracing::info!(?missed, "the handbook could not read some official pages");
    }
    pages
}

/// An official page's Markdown, unless it moved off its site or is gone.
async fn read(state: &AppState, url: &str) -> Result<Page> {
    let asked = Url::parse(url).map_err(|_| AppError::Internal("官方页面地址无效".into()))?;
    let (got, bytes) = reads::download(state, asked.clone(), reads::Content::Markdown).await?;
    if got.host_str() != asked.host_str() {
        return Err(AppError::Internal("官方页面跳到了别的网站".into()));
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| AppError::Internal("官方页面不是 UTF-8 文本".into()))?;
    if html(&text) {
        return Err(AppError::Internal("官方页面不是 Markdown".into()));
    }
    let print = hex::encode(&Sha256::digest(text.as_bytes())[..16]);
    Ok(Page { text, print })
}

/// Whether text served as Markdown is a web page: DeepSeek's site answers so, as Markdown, for
/// a page it doesn't have.
fn html(text: &str) -> bool {
    let start = text.trim_start().as_bytes();
    ["<!doctype html", "<html"].iter().any(|tag| {
        start
            .get(..tag.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(tag.as_bytes()))
    })
}

/// Writes a chapter for the first time and publishes it, checked at `cutoff` against the pages
/// read: what it was written from, a long reference's first part included.
async fn create(
    state: &AppState,
    author: i64,
    chapter: &Chapter,
    pages: &HashMap<&str, Page>,
    cutoff: DateTime<Utc>,
) -> Result<Uuid> {
    let begun = Utc::now();
    let (official, _) = told(chapter, pages, &[]).ok_or_else(|| {
        AppError::Internal(format!("「{}」的官方文档一页都没读到", chapter.title).into())
    })?;
    let since = cutoff - chrono::Duration::days(FIRST_DAYS);
    let stories = stories(state, chapter, since, cutoff).await?;
    let input = format!(
        "{}\n这一章：「{}」（{}）\n范围：{}\n",
        clock(state, begun),
        chapter.title,
        chapter.group(false),
        chapter.about
    );
    let found = look(state, Line::Handbook, SCOUTING, &input, &HashSet::new()).await;
    let material = format!(
        "{input}{}\n{official}\n{}\n两位编辑找来的线索：\n{}",
        contents(state),
        listed_stories(&stories),
        listed(&found.finds)
    );
    let mut known = reachable(state);
    known.extend(found.known);
    known.extend(stories.iter().filter_map(|story| identity(&story.url)));
    let corpus = corpus(pages, &material);
    let (article, usage) = compose(state, chapter, WRITING, &material, known, &[], &corpus).await?;
    let article = article.ok_or_else(|| AppError::Internal("新的一章没有正文".into()))?;
    // Chapters go out one after another in a round; the unique key keeps any from going out twice.
    let id = publish(
        state,
        author,
        begun,
        Some((Kind::Chapter, chapter.key)),
        &article,
    )
    .await?;
    note(state, id, 1, chapter, pages, None, chapter.pages, cutoff).await?;
    let mut spent = found.usage;
    spent += usage;
    log(
        id,
        "XABC's handbook chapter written",
        chapter.key,
        &article,
        spent,
    );
    Ok(id)
}

/// Brings a chapter up to date from its official pages, those changed since its last check
/// marked, and the stories since; when the writer finds nothing to change, only its check moves
/// to `cutoff`.
async fn update(
    state: &AppState,
    due: &Due<'_>,
    pages: &HashMap<&str, Page>,
    cutoff: DateTime<Utc>,
) -> Result<Option<Uuid>> {
    let Due {
        kept,
        chapter,
        changed,
        stories,
        ..
    } = due;
    let begun = Utc::now();
    let (official, whole) = told(chapter, pages, changed).unwrap_or_default();
    let zone = state.config.timezone;
    let (body, shown) = figures(&kept.body);
    let material = format!(
        "{}\n这一章：「{}」（{}）\n范围：{}\n{}\n{official}\n上次核对以来，{}\n这一章上次更新于 {}，现有全文：\n标题：{}\n\n{body}",
        clock(state, begun),
        chapter.title,
        chapter.group(false),
        chapter.about,
        contents(state),
        listed_stories(stories),
        kept.updated_at.with_timezone(&zone).format("%Y-%m-%d"),
        kept.title,
    );
    let mut known = reachable(state);
    known.extend(linked(&kept.body));
    known.extend(stories.iter().filter_map(|story| identity(&story.url)));
    let corpus = corpus(pages, &material);
    let (article, usage) = compose(
        state,
        chapter,
        &format!("{WRITING}{REVISING}{KEEPING}"),
        &material,
        known,
        &shown,
        &corpus,
    )
    .await?;
    let stored = kept.sources.as_ref().map(|sources| &sources.0);
    let Some(article) = article else {
        note(
            state,
            kept.id,
            kept.revision,
            chapter,
            pages,
            stored,
            &whole,
            cutoff,
        )
        .await?;
        tracing::info!(
            chapter = chapter.key,
            changed = changed.len(),
            stories = stories.len(),
            input_tokens = usage.input_tokens,
            output_tokens = usage.output_tokens,
            web_searches = usage.server_side_tool_usage_details.web_search_calls,
            cost_ticks = usage.cost_in_usd_ticks,
            "XABC's handbook chapter had nothing to change"
        );
        return Ok(None);
    };
    revise(state, kept.id, kept.revision, None, &article).await?;
    note(
        state,
        kept.id,
        kept.revision + 1,
        chapter,
        pages,
        stored,
        &whole,
        cutoff,
    )
    .await?;
    log(
        kept.id,
        "XABC's handbook chapter brought up to date",
        chapter.key,
        &article,
        usage,
    );
    Ok(Some(kept.id))
}

/// The chapter as the writer drafts it from `material`, written again once if its code names
/// what neither the official pages nor the material do; none when the writer, bringing it up to
/// date, finds nothing to change.
async fn compose(
    state: &AppState,
    chapter: &Chapter,
    instructions: &str,
    material: &str,
    mut known: HashSet<String>,
    shown: &[String],
    corpus: &HashSet<&str>,
) -> Result<(Option<super::Finished>, Usage)> {
    let mut drafted = draft(state, instructions, material).await?;
    let mut usage = std::mem::take(&mut drafted.usage);
    if drafted.article.body.trim().is_empty() {
        return Ok((None, usage));
    }
    let odd = unknown(&drafted.article.body, corpus);
    if !odd.is_empty() {
        let mut again = draft(
            state,
            instructions,
            &format!(
                "{material}\n\n你写好的全文：\n\n{}\n\n其中这些写法在本轮读到的官方文档和素材里都找不到，或者代码块不是能解析的 JSON：{}。逐个核对：官方没有的删掉或改成官方文档里的写法，json 代码块写成严格 JSON（只给片段时标成 jsonc）；示例里自己起的名字（服务器名、脚本名、路径、自定义变量）可以保留。改好后交回完整全文。",
                drafted.article.body,
                odd.join("、")
            ),
        )
        .await?;
        usage += std::mem::take(&mut again.usage);
        tracing::info!(
            chapter = chapter.key,
            ?odd,
            left = ?unknown(&again.article.body, corpus),
            "XABC wrote a handbook chapter again over what the official pages don't name"
        );
        // A check that came back empty is not done, and the draft it was asked about doesn't go out.
        if again.article.body.trim().is_empty() {
            return Err(AppError::Internal("核对后的正文是空的".into()));
        }
        known.extend(std::mem::take(&mut drafted.cited));
        drafted = again;
    }
    known.extend(drafted.cited);
    let mut article = drafted.article;
    article.title = chapter.title.to_owned();
    article.pictures.clear();
    let article = finish(state, article, known, shown, false).await?;
    Ok((Some(article), usage))
}

/// Notes that the chapter, as of `revision`, was checked at `at` against the pages of `whole` as
/// this round read them; the others keep the fingerprints they had, so a change the writer wasn't
/// shown whole comes up again, and pages the chapter no longer rests on drop out. An
/// administrator who saved the chapter meanwhile leaves it to be checked again next round.
#[allow(clippy::too_many_arguments)]
async fn note(
    state: &AppState,
    id: Uuid,
    revision: i32,
    chapter: &Chapter,
    pages: &HashMap<&str, Page>,
    stored: Option<&HashMap<String, String>>,
    whole: &[&str],
    at: DateTime<Utc>,
) -> Result<()> {
    let sources: HashMap<&str, &str> = chapter
        .pages
        .iter()
        .filter_map(|url| {
            let print = pages
                .get(url)
                .filter(|_| whole.contains(url))
                .map(|page| page.print.as_str())
                .or_else(|| stored?.get(*url).map(String::as_str))?;
            Some((*url, print))
        })
        .collect();
    let noted = sqlx::query(
        "UPDATE articles SET sources = $3, checked_at = $4
         WHERE id = $1 AND revision = $2 AND status = 'published'",
    )
    .bind(id)
    .bind(revision)
    .bind(Json(sources))
    .bind(at)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if noted == 0 {
        tracing::info!(
            chapter = chapter.key,
            "a handbook chapter changed while XABC checked it, and is checked again next round"
        );
    }
    Ok(())
}

/// The site's stories on the chapter's subject that came after `since` and by `until`, newest
/// first, each page once.
async fn stories(
    state: &AppState,
    chapter: &Chapter,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<Story>> {
    let words: Vec<String> = chapter.words.iter().map(|word| containing(word)).collect();
    Ok(sqlx::query_as(
        "SELECT title, summary, url, source, at FROM (
             SELECT DISTINCT ON (i.fingerprint) i.title, i.summary, i.url, i.source,
                    COALESCE(i.published_at, i.first_seen_at) AS at
             FROM items i
             WHERE i.topic_id = ANY($1) AND NOT i.hidden AND i.first_seen_at > $2
               AND i.first_seen_at <= $6
               AND concat_ws(' ', i.title, i.summary, i.title_en, i.summary_en) ILIKE ANY($3)
               AND concat_ws(' ', i.title, i.summary, i.title_en, i.summary_en) ILIKE ANY($4)
             ORDER BY i.fingerprint, i.first_seen_at DESC
         ) found ORDER BY at DESC LIMIT $5",
    )
    .bind(SOURCES)
    .bind(since)
    .bind(PRODUCTS)
    .bind(&words)
    .bind(STORIES)
    .bind(until)
    .fetch_all(&state.pool)
    .await?)
}

fn listed_stories(stories: &[Story]) -> String {
    if stories.is_empty() {
        return "本站还没有收到这个主题的新动态。\n".into();
    }
    let mut listed = "本站收集的相关动态与网友实践（新的在前）：\n".to_owned();
    for story in stories {
        let _ = writeln!(
            listed,
            "- {}｜{}｜{}｜{}｜{}",
            story.at.format("%Y-%m-%d"),
            story.title,
            story.summary,
            story.source,
            story.url
        );
    }
    listed
}

/// The chapter's official pages as the writer reads them, and those it reads whole: the pages
/// changed since its last check first and whole while room is left, a changed page longer than
/// all the room as far as it goes and counted as read; then the others' first `PAGE_CHARS`. None
/// when no page could be read.
fn told(
    chapter: &Chapter,
    pages: &HashMap<&str, Page>,
    changed: &[&str],
) -> Option<(String, Vec<&'static str>)> {
    let mut order = chapter.pages.to_vec();
    order.sort_by_key(|url| !changed.contains(url));
    let mut told = "官方文档原文（以此为准）：\n".to_owned();
    let mut whole = Vec::new();
    let mut room = OFFICIAL_CHARS;
    let mut any = false;
    for url in order {
        let Some(page) = pages.get(url) else {
            continue;
        };
        any = true;
        let [readable, _] = handbook::readable(url);
        let fresh = changed.contains(&url);
        let mark = if fresh {
            "（自上次核对以来有改动）"
        } else {
            ""
        };
        let limit = if fresh { room } else { PAGE_CHARS.min(room) };
        if limit == 0 {
            let _ = writeln!(
                told,
                "【{readable}】{mark}篇幅所限没有附上原文，需要时用 web_search 打开。"
            );
            continue;
        }
        let text: String = page.text.chars().take(limit).collect();
        let cut = text.len() < page.text.len();
        let given = text.chars().count();
        // A changed page longer than all the room counts as read when it comes first, or it would
        // stay changed for good; none is that long now.
        if !cut || (fresh && room == OFFICIAL_CHARS) {
            whole.push(url);
        }
        if cut && fresh && room == OFFICIAL_CHARS {
            tracing::info!(
                page = url,
                given,
                length = page.text.chars().count(),
                "an official page is longer than the handbook reads whole"
            );
        }
        room -= given;
        let _ = writeln!(
            told,
            "【{readable}】{mark}\n{text}{}\n",
            if cut {
                "\n（原文较长，后面没有附上，需要时用 web_search 打开完整页面。）"
            } else {
                ""
            }
        );
    }
    any.then_some((told, whole))
}

fn clock(state: &AppState, at: DateTime<Utc>) -> String {
    let zone = state.config.timezone;
    format!(
        "现在是 {}（{}）。",
        at.with_timezone(&zone).format("%Y-%m-%d %H:%M"),
        zone.name()
    )
}

/// The handbook's chapters with their addresses, for the writer to link to.
fn contents(state: &AppState) -> String {
    let mut contents = "手册的各章（正文提到其他章时可以链接）：\n".to_owned();
    for chapter in &CHAPTERS {
        let _ = writeln!(
            contents,
            "- {}：{}",
            chapter.title,
            absolute(state, &chapter.path())
        );
    }
    contents
}

/// The pages a chapter may link to besides what the writer and the scouts cite: every official
/// page the handbook rests on, as readers read it and in Chinese, and the handbook's chapters.
fn reachable(state: &AppState) -> HashSet<String> {
    CHAPTERS
        .iter()
        .flat_map(|chapter| chapter.pages.iter())
        .flat_map(|page| {
            let [readable, chinese] = handbook::readable(page);
            [(*page).to_owned(), readable, chinese]
        })
        .chain(
            CHAPTERS
                .iter()
                .map(|chapter| absolute(state, &chapter.path())),
        )
        .filter_map(|url| identity(&url))
        .collect()
}

/// The words the official pages read this round and the writer's material are made of.
fn corpus<'a>(pages: &'a HashMap<&str, Page>, material: &'a str) -> HashSet<&'a str> {
    pages
        .values()
        .flat_map(|page| words(&page.text))
        .chain(words(material))
        .collect()
}

/// The runs of letters, digits, `_`, `-` and `.` a configuration or a command is made of, less
/// the full stop or dash a sentence ends one with.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
        .map(|word| word.trim_end_matches(['.', '-']))
        .filter(|word| !word.is_empty())
}

/// What the body's code names that belongs to the products and `corpus` lacks, in order.
fn unknown(body: &str, corpus: &HashSet<&str>) -> Vec<String> {
    named(body)
        .into_iter()
        .filter(|name| !corpus.contains(name.as_str()))
        .collect()
}

/// What the body's code names that only the products define: the long options of `claude` and
/// `codex` commands, the products' environment variables and model IDs, and the keys of TOML
/// and JSON configuration, but not names a configuration makes up, such as `[mcp_servers.x]`,
/// `"mcpServers": {"x": …}` or another program's variables.
fn named(body: &str) -> BTreeSet<String> {
    let mut named = BTreeSet::new();
    let mut block: Option<String> = None;
    let mut code = String::new();
    for event in Parser::new_ext(body, MARKDOWN) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                block = Some(match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                    CodeBlockKind::Indented => String::new(),
                });
                code.clear();
            }
            Event::Text(text) if block.is_some() => code.push_str(&text),
            Event::End(TagEnd::CodeBlock) => {
                let language = block.take().unwrap_or_default();
                scan(&code, &language, &mut named);
            }
            Event::Code(inline) => scan(&inline, "", &mut named),
            _ => {}
        }
    }
    named
}

fn scan(code: &str, language: &str, named: &mut BTreeSet<String>) {
    // A command a shell continues over lines ending in `\` is one line.
    for line in code.replace("\\\n", " ").lines() {
        let words: Vec<&str> = words(line).collect();
        let command = words.iter().any(|word| matches!(*word, "claude" | "codex"));
        for word in words {
            if (command && word.len() > 2 && word.starts_with("--"))
                || variable(word)
                || model(word)
            {
                named.insert(word.to_owned());
            }
        }
        if language == "toml"
            && let Some(key) = toml_key(line)
        {
            named.insert(key.to_owned());
        }
    }
    // Configuration to copy is strict JSON; a fragment is marked `jsonc`.
    match serde_json::from_str::<Value>(code) {
        Ok(value) if matches!(language, "json" | "jsonc") => json_keys(&value, named),
        Err(_) if language == "json" => {
            named.insert(UNPARSED.to_owned());
        }
        _ => {}
    }
}

/// What a `json` code block that doesn't parse is told as, in no official page's words.
const UNPARSED: &str = "（无法解析的 json 代码块）";

/// A variable of Claude's or OpenAI's, which their documentation names.
fn variable(word: &str) -> bool {
    ["ANTHROPIC_", "CLAUDE_", "CODEX_", "OPENAI_"]
        .iter()
        .any(|prefix| word.starts_with(prefix))
        && word
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// A model's ID: `claude-` and a family, `gpt-` and a number, or DeepSeek's.
fn model(word: &str) -> bool {
    word.strip_prefix("claude-").is_some_and(|rest| {
        ["opus", "sonnet", "haiku", "fable", "mythos"]
            .iter()
            .any(|family| rest.starts_with(family))
    }) || word
        .strip_prefix("gpt-")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        || word.len() > "deepseek-".len() && word.starts_with("deepseek-")
}

/// The key a line of TOML sets, the last of a dotted one; not a table's name, a quoted key or
/// one holding an inline table, which may be names the configuration makes up.
fn toml_key(line: &str) -> Option<&str> {
    let (key, value) = line.trim().split_once('=')?;
    if value.trim_start().starts_with('{') || key.contains(['"', '\'', '[', '#']) {
        return None;
    }
    let key = key.trim().rsplit('.').next()?;
    (key.starts_with(|c: char| c.is_ascii_lowercase())
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
    .then_some(key)
}

/// The keys of a JSON configuration but those holding an object, which may be names it makes
/// up, and upper-case ones, variables of whatever reads them.
fn json_keys(value: &Value, named: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                // A schema's own keys start with `$`, which the pages' words leave out: `$schema`
                // is `schema`. Other keys are checked whole.
                if !value.is_object()
                    && !key
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    named.insert(key.strip_prefix('$').unwrap_or(key).to_owned());
                }
                json_keys(value, named);
            }
        }
        Value::Array(values) => {
            for value in values {
                json_keys(value, named);
            }
        }
        _ => {}
    }
}

/// What both scouts look for, for a chapter to be written.
const SCOUTING: &str = "你是 XABC 的资料编辑。XABC 在 xabcnews.com 维护「Codex & Claude」手册：写给中文开发者的 Claude Code 与 Codex 实用手册，以官方文档为准，汇集官方与网友的最佳实践和实用配置。现在要写其中一章，输入里有这一章的主题和范围。
请你{scope}，找出这一章最值得收进来的资料：官方的指南、更新说明和工程博客，开发者在 X、GitHub、技术博客和中文社区分享的、带具体步骤或配置的最佳实践、实用配置和踩坑经验。按价值和传播从高到低排列，至少 10 条，证据不足就少列，不凑数。points 写清可以照做的步骤、配置项和命令（原样照抄）、适用的版本和效果。没有步骤的泛泛心得、营销软文、中转站与账号买卖、只是转述官方文档的内容不要。
";

/// How the writer writes a chapter.
const WRITING: &str = "你是 XABC，在 xabcnews.com 维护「Codex & Claude」手册：写给中文开发者的 Claude Code 与 Codex 实用手册，以官方文档为准，汇集官方与网友的最佳实践和实用配置，官方一更新就跟着改。手册分四组、十七章，每章讲透一个主题。输入里有这一章的主题和范围、它登记的官方文档原文、本站收集的相关动态与网友实践，写新的一章时还有两位编辑从 X 和全网找来的线索。多用 web_search 补全和核实，写出这一章完整的中文正文。
怎么写：
- 开头两三句话说清这一章解决什么、结论是什么，写明对应的版本（如「Claude Code 2.1.3xx、Codex 0.17x」），拿不准版本就写日期。
- 按读者要做的事组织：操作步骤、可以直接复制的配置、网友的做法、常见的坑；最后两节依次是「近期变化」（最近一两个月影响用法的变化，每条写日期和版本，没有就省略这一节）和「参考来源」（列表，每条是指向原始页面的 Markdown 链接，后面一句话说明它是什么）。小标题用 ## 和 ###。
- Claude Code 和 Codex 都讲；两边机制不同就分开讲，不硬凑对照，适合对比的地方用表格。这一章讲的是 API 或 SDK 时，讲对应的接口和 SDK。
- 以官方文档为准：命令、参数、环境变量、配置项、文件路径和模型 ID 的写法与官方原文逐字一致，官方没有的不写进代码块。分清官方的说法、网友的经验和你的建议；网友的做法写明出自谁，链接原帖或原文。
- 可复制的配置放进代码块并标明语言（json、toml、bash 等），代码块前用一句话说明文件放在哪、起什么作用；json 代码块写成严格 JSON（不加注释、不用省略号），只给片段时标成 jsonc；密钥和账号写成占位符。
- 只写现在有效的做法；改名、弃用或过时的，只在「近期变化」或注意事项里点明。
- 链接官方文档时链到读者看的页面（地址去掉 .md）；Claude Code 与 Claude API 的页面优先链官方中文版（地址里的 /docs/en/ 换成 /docs/zh-CN/），DeepSeek 的页面链中文版（域名后面加 /zh-cn）。
- 不配图：正文不放图片，pictures 留空，cover 留空。
- 篇幅以讲清为准，信息要密，不写空话；title 照写这一章的标题。
";

/// How the writer brings a chapter up to date.
const REVISING: &str = "- 这次是更新：输入里还有这一章的现有全文，官方文档自上次核对以来有改动的页面已经标出，后面是上次核对以来本站新收的动态。先对照官方原文：官方改了的照新的改，新的功能和配置补进去，过时、改名或弃用的改掉并写进「近期变化」；再把新动态里有价值的做法、配置和坑补到对应的位置。保留仍然正确的内容和出处，交回更新后的完整全文。
- 对照之后没有需要改动的地方：body 留空，pictures 留空。
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_the_products_define_is_checked_against_their_pages() {
        let body = r#"先装好再运行 `claude --dangerously-skip-permissions`，或在脚本里用 `npx prettier --write`。

```bash
export ANTHROPIC_BASE_URL=https://gateway.example
export ANTHROPIC_MODEL=deepseek-flash[1m]
export GITHUB_TOKEN=secret
claude -p "总结" --output-format json --max-turnz 3
codex exec --json "修复测试"
claude -p "长任务" \
  --max-turns 3
```

```json
{
  // 注释
  "model": "x"
}
```

```jsonc
{ "hooks": { /* 片段 */ } }
```

```toml
model = "gpt-6.1-terra"
review_model = "deepseek-v4-pros"
approval_mode = "never"
[mcp_servers.github]
command = "npx"
env = { GITHUB_TOKEN = "x" }
```

```json
{
  "$schema": "https://json.schemastore.org/claude-code-settings.json",
  "model": "claude-opus-5-5",
  "model-": "x",
  "permissions": { "deny": ["Read(./.env)"] },
  "mcpServers": { "playwright": { "command": "npx", "args": ["@playwright/mcp"] } },
  "env": { "CLAUDE_CODE_USE_BEDROCK": "1", "MY_FLAG": "1" }
}
```

正文里说 claude --not-code 不算。"#;
        let named: Vec<String> = named(body).into_iter().collect();
        assert_eq!(
            named,
            [
                "--dangerously-skip-permissions",
                "--json",
                "--max-turns",
                "--max-turnz",
                "--output-format",
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_MODEL",
                "CLAUDE_CODE_USE_BEDROCK",
                "approval_mode",
                "args",
                "claude-opus-5-5",
                "command",
                "deepseek-flash",
                "deepseek-v4-pros",
                "deny",
                "gpt-6.1-terra",
                "model",
                "model-",
                "review_model",
                "schema",
                UNPARSED,
            ]
        );
        let corpus: HashSet<&str> = words(
            "Use --dangerously-skip-permissions, --output-format, --max-turns and --json. Set ANTHROPIC_BASE_URL, ANTHROPIC_MODEL or CLAUDE_CODE_USE_BEDROCK. model, review_model, approval_policy, command, args, deny, $schema. Models: claude-opus-5-5, gpt-6.1-terra, deepseek-flash, deepseek-v4-pro.",
        )
        .collect();
        assert_eq!(
            unknown(body, &corpus),
            [
                "--max-turnz",
                "approval_mode",
                "deepseek-v4-pros",
                "model-",
                UNPARSED
            ]
        );
    }

    #[test]
    fn a_web_page_served_as_markdown_is_not_read() {
        assert!(html("<!doctype html>\n<html lang=\"en\">"));
        assert!(html("\n  <HTML>"));
        assert!(!html("# 接入 Claude Code\n\n<details>"));
        assert!(!html("# Integrate with Codex"));
    }

    #[test]
    fn a_line_of_toml_sets_its_last_key_unless_it_names_a_table_or_one_of_its_own() {
        assert_eq!(
            toml_key("model_reasoning_effort = \"high\""),
            Some("model_reasoning_effort")
        );
        assert_eq!(
            toml_key("  sandbox_workspace_write.network_access = true"),
            Some("network_access")
        );
        for line in [
            "[profiles.fast]",
            "github = { command = \"npx\" }",
            "\"my server\" = 1",
            "GITHUB_TOKEN = \"x\"",
            "# model = \"x\"",
        ] {
            assert_eq!(toml_key(line), None, "{line}");
        }
    }

    #[test]
    fn the_writer_reads_changed_pages_first_and_whole_and_long_others_in_part() {
        // The second page is long and unchanged; the third and fourth changed, the fourth long.
        let pages: HashMap<&str, Page> = CHAPTERS[0]
            .pages
            .iter()
            .enumerate()
            .map(|(at, url)| {
                let text = match at {
                    1 => "长".repeat(PAGE_CHARS + 5),
                    3 => "变".repeat(PAGE_CHARS + 7),
                    _ => format!("第 {at} 页"),
                };
                (
                    *url,
                    Page {
                        text,
                        print: String::new(),
                    },
                )
            })
            .collect();
        let changed = [CHAPTERS[0].pages[2], CHAPTERS[0].pages[3]];
        let (text, whole) = told(&CHAPTERS[0], &pages, &changed).unwrap();
        let [first, _] = handbook::readable(CHAPTERS[0].pages[2]);
        assert!(text.starts_with(&format!(
            "官方文档原文（以此为准）：\n【{first}】（自上次核对以来有改动）\n第 2 页"
        )));
        assert!(text.contains(&"变".repeat(PAGE_CHARS + 7)));
        assert!(text.contains("（原文较长，后面没有附上"));
        assert_eq!(text.matches('长').count(), PAGE_CHARS + 1);
        // What the writer read whole is checked; the long page it read in part is not.
        assert!(whole.contains(&CHAPTERS[0].pages[3]) && whole.contains(&CHAPTERS[0].pages[0]));
        assert!(!whole.contains(&CHAPTERS[0].pages[1]));
        // An unchanged long page coming first is read in part all the same.
        let first = HashMap::from([(
            CHAPTERS[0].pages[0],
            Page {
                text: "长".repeat(PAGE_CHARS + 1),
                print: String::new(),
            },
        )]);
        assert!(told(&CHAPTERS[0], &first, &[]).unwrap().1.is_empty());
        assert!(told(&CHAPTERS[0], &HashMap::new(), &[]).is_none());
    }
}
