//! 文娱 · 解说: XABC explains a film or series in an article of its own. A work has one article,
//! which XABC revises once 影视 has gathered enough new about the work; when none is due, a new
//! one alternates between the work 影视 discusses most and an acclaimed classic. Searching,
//! writing and pictures are 分享's (`super`).
use super::{
    Finished, Line, Subject, draft, figures, finish, identity, linked, listed, look, publish,
    revise, written_lately,
};
use crate::{
    ai, catalog,
    error::{AppError, Result},
    state::AppState,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, fmt::Write};
use uuid::Uuid;

/// An article is revised once 影视 has this many new stories about its work, and these days have
/// passed since it last was.
const FRESH_STORIES: i64 = 5;
const REVISE_AFTER_DAYS: i32 = 3;
/// 影视's kinds of work.
const KINDS: [&str; 3] = ["movie", "series", "animation"];

/// A work an article explains.
#[derive(sqlx::FromRow)]
struct Work {
    /// Its key in 影视 (`catalog::work_key`).
    key: String,
    kind: String,
    name: String,
    name_en: String,
    creator: String,
    #[sqlx(default)]
    year: Option<i32>,
    /// The guide 影视 wrote from the sources it read, when it knows the work.
    guide: Option<Value>,
}

impl Work {
    /// The work as the models are told it: `《教父》（The Godfather，1972，电影，主创 …）`.
    fn describe(&self) -> String {
        let mut notes = vec![self.name_en.clone()];
        notes.extend(self.year.map(|year| year.to_string()));
        notes.push(
            match self.kind.as_str() {
                "series" => "剧集",
                "animation" => "动画",
                _ => "电影",
            }
            .into(),
        );
        if !self.creator.is_empty() {
            notes.push(format!("主创 {}", self.creator));
        }
        format!("《{}》（{}）", self.name, notes.join("，"))
    }

    /// The work, and what 影视's guide checked against its sources.
    fn material(&self) -> String {
        let mut material = format!("作品：{}\n", self.describe());
        if let Some(guide) = &self.guide {
            let _ = writeln!(
                material,
                "影视栏目核实过的资料：{}",
                guide["intro"].as_str().unwrap_or_default()
            );
            for fact in guide["facts"].as_array().into_iter().flatten() {
                let _ = writeln!(
                    material,
                    "- {}：{}",
                    fact["label"].as_str().unwrap_or_default(),
                    fact["text"].as_str().unwrap_or_default()
                );
            }
        }
        material
    }
}

/// An article whose work 影视 has gathered enough new about, as it reads now.
#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    title: String,
    body: String,
    revision: i32,
    cover: Option<Uuid>,
    updated_at: DateTime<Utc>,
    entry: Uuid,
    #[sqlx(flatten)]
    work: Work,
}

/// Revises the article that is due, or else explains a new work.
pub(super) async fn write(state: &AppState, author: i64, since: DateTime<Utc>) -> Result<Uuid> {
    if let Some(due) = due(state, author).await? {
        return revised(state, due).await;
    }
    let written: i64 = sqlx::query_scalar("SELECT count(*) FROM articles WHERE film IS NOT NULL")
        .fetch_one(&state.pool)
        .await?;
    let hot = if written % 2 == 0 {
        hot(state).await?
    } else {
        None
    };
    let work = match hot {
        Some(work) => work,
        None => classic(state).await?,
    };
    let mut found = look(state, Line::Film, &work.describe(), &HashSet::new()).await;
    if found.finds.is_empty() {
        return Err(AppError::Internal("两路检索都没有找到资料".into()));
    }
    let material = format!("{}\n线索：\n{}", work.material(), listed(&found.finds));
    let drafted = draft(state, WRITING, &material).await?;
    found.usage += drafted.usage;
    found.known.extend(drafted.cited);
    let article = finish(state, drafted.article, found.known, &[], true).await?;
    let id = publish(state, author, since, Subject::Film(&work.key), &article).await?;
    log(id, "XABC's 解说 written", &work, &article, found.usage);
    Ok(id)
}

/// The article longest waiting among those whose work 影视 has `FRESH_STORIES` new stories about
/// since its last revision, `REVISE_AFTER_DAYS` ago at least.
async fn due(state: &AppState, author: i64) -> Result<Option<Due>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.title, a.body, a.revision, a.cover, a.updated_at, e.id AS entry,
                e.key, e.kind, e.name, e.name_en, e.creator, e.guide
         FROM articles a
         JOIN catalog_entries e ON e.topic_id = 'movie' AND e.key = a.film
         JOIN catalog_items c ON c.entry_id = e.id
         JOIN items i ON i.id = c.item_id
         WHERE a.author_id = $1 AND a.status = 'published'
           AND a.updated_at <= now() - make_interval(days => $2)
           AND NOT i.hidden AND i.first_seen_at > a.updated_at
         GROUP BY a.id, e.id
         HAVING count(*) >= $3
         ORDER BY a.updated_at
         LIMIT 1",
    )
    .bind(author)
    .bind(REVISE_AFTER_DAYS)
    .bind(FRESH_STORIES)
    .fetch_optional(&state.pool)
    .await?)
}

/// The work 影视 discussed most in the last 30 days that has no article yet: it must have been
/// reviewed or posted about, and reviews and posts count before news.
async fn hot(state: &AppState) -> Result<Option<Work>> {
    Ok(sqlx::query_as(
        "SELECT e.key, e.kind, e.name, e.name_en, e.creator, e.guide
         FROM catalog_entries e
         JOIN catalog_items c ON c.entry_id = e.id
         JOIN items i ON i.id = c.item_id
         WHERE e.topic_id = 'movie' AND NOT i.hidden AND i.first_seen_at >= now() - INTERVAL '30 days'
           AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.film = e.key)
         GROUP BY e.id
         HAVING count(*) FILTER (WHERE c.role <> 'news') > 0
         ORDER BY count(*) FILTER (WHERE c.role <> 'news') DESC, count(*) DESC, max(i.first_seen_at) DESC
         LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?)
}

#[derive(Deserialize)]
struct Classics {
    works: Vec<Classic>,
}

#[derive(Deserialize)]
struct Classic {
    kind: String,
    name: String,
    name_en: String,
    year: i32,
}

/// An acclaimed classic without an article: the briefing model suggests several, avoiding those
/// written about lately, and the first without an article is taken.
async fn classic(state: &AppState) -> Result<Work> {
    let recent: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM articles WHERE film IS NOT NULL ORDER BY published_at DESC LIMIT 60",
    )
    .fetch_all(&state.pool)
    .await?;
    let work = ai::object(json!({
        "kind": { "type": "string", "enum": KINDS },
        "name": { "type": "string" },
        "name_en": { "type": "string" },
        "year": { "type": "integer" },
    }));
    let reply = ai::request(
        state,
        json!({
            "model": state.config.ai_insight_model,
            "reasoning": { "effort": "low" },
            "store": false,
            "instructions": CLASSICS,
            "input": [{ "role": "user", "content": written_lately(recent.iter().map(String::as_str)) }],
            "text": { "format": {
                "type": "json_schema", "name": "classics", "strict": true,
                "schema": ai::object(json!({ "works": { "type": "array", "items": work } })),
            } },
        }),
    )
    .await?;
    let classics: Classics = serde_json::from_value(ai::parse_payload(&reply)?)
        .map_err(|_| AppError::Internal("经典推荐不符合约定的结构".into()))?;
    let keys: Vec<String> = classics
        .works
        .iter()
        .map(|work| catalog::work_key(&work.kind, &work.name_en))
        .collect();
    let taken: HashSet<String> =
        sqlx::query_scalar("SELECT film FROM articles WHERE film = ANY($1)")
            .bind(&keys)
            .fetch_all(&state.pool)
            .await?
            .into_iter()
            .collect();
    let (classic, key) = classics
        .works
        .into_iter()
        .zip(keys)
        .find(|(work, key)| {
            KINDS.contains(&work.kind.as_str())
                && !work.name.trim().is_empty()
                && work.name_en.chars().any(char::is_alphanumeric)
                && !taken.contains(key)
        })
        .ok_or_else(|| AppError::Internal("推荐的经典都写过了".into()))?;
    // 影视 may know it too.
    let guide: Option<Value> = sqlx::query_scalar(
        "SELECT guide FROM catalog_entries WHERE topic_id = 'movie' AND key = $1",
    )
    .bind(&key)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    Ok(Work {
        key,
        kind: classic.kind,
        name: classic.name.trim().to_owned(),
        name_en: classic.name_en.trim().to_owned(),
        creator: String::new(),
        year: Some(classic.year),
        guide,
    })
}

/// The article revised with what came out since: 影视's new stories about the work, and both
/// searches. Its figures and its cover stay.
async fn revised(state: &AppState, due: Due) -> Result<Uuid> {
    let stories: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT i.title, i.summary, i.url FROM items i JOIN catalog_items c ON c.item_id = i.id
         WHERE c.entry_id = $1 AND NOT i.hidden AND i.first_seen_at > $2
         ORDER BY i.first_seen_at DESC LIMIT 24",
    )
    .bind(due.entry)
    .bind(due.updated_at)
    .fetch_all(&state.pool)
    .await?;
    let last = due
        .updated_at
        .with_timezone(&state.config.timezone)
        .format("%Y-%m-%d");
    let input = format!(
        "{}\n这篇解说上次修订于 {last}，重点找那之后的新资料，以及当时还缺的完整剧情和结局。",
        due.work.describe()
    );
    let mut found = look(state, Line::Film, &input, &HashSet::new()).await;
    // The article's links were checked when it went out, and 影视's stories were read.
    found.known.extend(linked(&due.body));
    found
        .known
        .extend(stories.iter().filter_map(|(_, _, url)| identity(url)));
    let (body, kept) = figures(&due.body);
    let mut news = String::new();
    for (title, summary, url) in &stories {
        let _ = writeln!(news, "- {title}：{summary}（{url}）");
    }
    let material = format!(
        "{}\n这篇解说的现有全文：\n标题：{}\n\n{body}\n\n影视栏目在上次修订之后收到的内容：\n{news}\n线索：\n{}",
        due.work.material(),
        due.title,
        listed(&found.finds)
    );
    let drafted = draft(state, &format!("{WRITING}{REVISING}"), &material).await?;
    found.usage += drafted.usage;
    found.known.extend(drafted.cited);
    let article = finish(state, drafted.article, found.known, &kept, false).await?;
    revise(state, due.id, due.revision, due.cover, None, &article).await?;
    log(
        due.id,
        "XABC's 解说 revised",
        &due.work,
        &article,
        found.usage,
    );
    Ok(due.id)
}

fn log(id: Uuid, done: &str, work: &Work, article: &Finished, usage: crate::fetch::Usage) {
    tracing::info!(
        %id, work = work.key, title = article.title, pictures = article.pictures.len(),
        input_tokens = usage.input_tokens, output_tokens = usage.output_tokens,
        web_searches = usage.server_side_tool_usage_details.web_search_calls,
        x_searches = usage.server_side_tool_usage_details.x_search_calls,
        cost_ticks = usage.cost_in_usd_ticks,
        "{done}"
    );
}

pub(super) const SEARCHING: &str = "你是 XABC「文娱 · 解说」的资料编辑。XABC 在 xabcnews.com 为一部部好电影和好剧写完整的解说，读者是爱看电影和剧的中文读者。
输入是这次要解说的作品。请你{scope}，找出写解说用得上的资料，越多越具体越好：完整剧情和结局、关键情节与人物关系、主题与结局的解读、创作背景与幕后故事、主创访谈、专业影评与观众口碑、奖项与票房。只找这部作品的资料，分清同名作品、翻拍和续集。
";

const WRITING: &str = "你是 XABC，在 xabcnews.com 的「文娱 · 解说」栏目写影视解说，读者是爱看电影和剧的中文读者。
下面是这部作品的资料和两位编辑找来的线索。需要时用 web_search 继续查，然后写一篇完整的中文解说。
怎么写：
- 把这部作品讲透：用自己的话完整讲出剧情，一直讲到结局，再解读结局和关键情节、人物的动机与变化、主题和细节，以及拍摄、表演、配乐和幕后故事里值得说的地方。直接讲，不加剧透提示。
- 像一个懂电影、会讲故事的朋友：开头直接进入，具体、生动，有自己的判断。篇幅不限，内容需要多长就写多长；结构跟着内容走，需要时用小标题。
- 剧情、台词、人物关系、数字和幕后故事只写资料和检索里查得到的；资料里还没有完整剧情或结局时，写到查得到的地方为止，多写看点和评价。
- 片名第一次出现写《中文名》并附原名，人名用通行的中文译名，第一次出现时附原名；标题带上片名。
- 配图优先用剧照和海报所在的页面；插图不画演员，也不画片中角色的样子。
";

const REVISING: &str = "- 这次是修订：输入里有这篇解说的现有全文、影视栏目在上次修订之后收到的内容和新的线索。在现有全文的基础上把它改得更好：补上以前查不到的剧情和结局，修正不准确的地方，把新的评价、幕后、奖项和讨论自然地写进去，删掉过时的说法，交回修订后的完整全文。现有配图写作 ![](keep:1)、![](keep:2)……，单独成段，可以保留、挪动或删掉，不要改写，下面也不要加图注；新图照常用 ![](img:N)。
";

const CLASSICS: &str = "你为 xabcnews.com 的「文娱 · 解说」挑选下一部要解说的作品。推荐 8 部影史公认的好电影或好剧（动画也可以），年代、国家和地区、类型尽量分散，名作和不那么出名的佳作都可以，避开输入里最近写过的。kind 从 movie、series、animation 选；name 是通行的中文名；name_en 是通行的英文正式名称，没有英文名时用原名的通行罗马字写法；year 是首映年份。严格按 JSON Schema 输出。";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_work_is_described_with_what_its_guide_checked() {
        let work = Work {
            key: "movie:thegodfather".into(),
            kind: "movie".into(),
            name: "教父".into(),
            name_en: "The Godfather".into(),
            creator: "Francis Ford Coppola".into(),
            year: Some(1972),
            guide: Some(json!({
                "intro": "黑帮家族的故事。",
                "facts": [{ "label": "片长", "text": "175 分钟" }],
            })),
        };
        assert_eq!(
            work.material(),
            "作品：《教父》（The Godfather，1972，电影，主创 Francis Ford Coppola）\n影视栏目核实过的资料：黑帮家族的故事。\n- 片长：175 分钟\n"
        );
        let series = Work {
            kind: "series".into(),
            creator: String::new(),
            year: None,
            guide: None,
            ..work
        };
        assert_eq!(series.material(), "作品：《教父》（The Godfather，剧集）\n");
    }
}
