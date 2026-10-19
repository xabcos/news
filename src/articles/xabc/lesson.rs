//! 学习: every quarter of an hour XABC keeps the courses current before it extends them. It revises the lesson
//! that is due, one the site's news has moved on from or one whose review date has come, the most
//! read first; when none is, it writes the next lesson of the track whose newest lesson is oldest.
//! Each starts from a plan: the next lesson after the track's contents, or what a revision should
//! bring, with the subjects news about the lesson would name and how soon it should be reviewed
//! again. The site's recent stories on those subjects and a search of the web are the writer's
//! material; writing and pictures are 分享's (`super`).
use super::{
    Finished, Line, MARKDOWN, Subject, Upkeep, draft, figures, finish, identity, linked, listed,
    look, publish, revise,
};
use crate::{
    ai,
    articles::learn::{self, Course, Lesson},
    error::{AppError, Result},
    fetch::Usage,
    site::absolute,
    state::AppState,
};
use chrono::{DateTime, Utc};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashSet, fmt::Write};
use uuid::Uuid;

/// How many days on a lesson is reviewed again: the planner picks, by how fast its subject
/// changes, within these.
const REVIEW_DAYS: std::ops::RangeInclusive<i32> = 30..=180;
/// A lesson is due sooner once the site has gathered this many stories on its subjects in the
/// last `NEWS_DAYS` since its last revision, `NEWS_DAYS` after that at least.
const NEWS: i64 = 2;
const NEWS_DAYS: i32 = 7;
/// A keyword more of the week's stories hold than this is too common to tell that a lesson
/// changed: popular products are named in a hundred stories a week.
const COMMON: i64 = 10;
/// Where the site's stories for a lesson come from: news, tips, problems and their fixes,
/// open source, cases and long reads on AI and its hardware.
const SOURCES: [&str; 9] = [
    "x",
    "china-ai",
    "overseas-ai",
    "skills",
    "issues",
    "github",
    "cases",
    "ai-hardware",
    "reads",
];
/// The most of them a lesson's writer reads.
const STORIES: i64 = 12;

/// The next lesson, or what a lesson's revision brings.
#[derive(Deserialize)]
struct Plan {
    chapter: String,
    title: String,
    goal: String,
    outline: Vec<String>,
    /// Names of the features, tools or techniques it teaches, to find the site's stories on them.
    keywords: Vec<String>,
    review_days: i32,
}

impl Plan {
    /// The keywords as a lesson keeps them: in lower case, two characters at least, five at most.
    fn keywords(&self) -> Vec<String> {
        let mut kept: Vec<String> = Vec::new();
        for keyword in &self.keywords {
            let keyword = keyword.trim().to_lowercase();
            if keyword.chars().count() >= 2 && !kept.contains(&keyword) && kept.len() < 5 {
                kept.push(keyword);
            }
        }
        kept
    }

    fn review_days(&self) -> i32 {
        self.review_days
            .clamp(*REVIEW_DAYS.start(), *REVIEW_DAYS.end())
    }
}

/// The lesson due for revision as it reads now, and whether news made it due.
#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    course: String,
    chapter: String,
    lesson: i32,
    title: String,
    body: String,
    revision: i32,
    cover: Option<Uuid>,
    updated_at: DateTime<Utc>,
    news: bool,
}

/// Revises the lesson that is due, or else writes a course's next lesson; once a week it first
/// looks through the news for a course to open (`course`).
pub(super) async fn write(state: &AppState, author: i64, since: DateTime<Utc>) -> Result<Uuid> {
    if let Err(error) = super::course::discover(state).await {
        tracing::warn!(%error, "XABC could not look for a new course");
    }
    if let Some(due) = due(state).await? {
        return revised(state, due).await;
    }
    let track = &next(state).await?;
    let lessons = learn::lessons(state, &track.id).await?;
    let (plan, mut usage) = plan(state, track, &lessons, None).await?;
    let chapter = plan.chapter.trim();
    if !(1..=40).contains(&chapter.chars().count()) {
        return Err(AppError::Internal("课程计划的章名不合格".into()));
    }
    let keywords = plan.keywords();
    let stories = stories(state, &keywords).await?;
    let mut found = look(
        state,
        Line::Lesson,
        &searching(state, track, chapter, &plan, None),
        &HashSet::new(),
    )
    .await;
    if found.finds.is_empty() {
        return Err(AppError::Internal("检索没有找到资料".into()));
    }
    found.known.extend(own(state, &lessons, &stories));
    let material = format!(
        "{}\n课程目录：\n{}\n{}\n站内最近收录的相关资料（可以引用）：\n{}\n线索：\n{}",
        describe(track),
        contents(state, &lessons, true),
        planned(chapter, &plan),
        listed_stories(&stories),
        listed(&found.finds)
    );
    let drafted = draft(state, WRITING, &material).await?;
    usage += found.usage;
    usage += drafted.usage;
    found.known.extend(drafted.cited);
    let article = finish(state, drafted.article, found.known, &[], true).await?;
    let id = publish(
        state,
        author,
        since,
        Subject::Lesson {
            course: &track.id,
            chapter,
            upkeep: Upkeep {
                keywords: &keywords,
                review_days: plan.review_days(),
            },
        },
        &article,
    )
    .await?;
    log(id, "XABC's lesson written", track, &article, usage);
    Ok(id)
}

/// The visible course whose newest lesson is oldest, those without lessons first, then in the
/// order courses came.
async fn next(state: &AppState) -> Result<Course> {
    sqlx::query_as(
        "SELECT c.id, c.grp, c.grp_en, c.title, c.title_en, c.intro, c.intro_en, c.brief
         FROM courses c LEFT JOIN articles a ON a.course = c.id
         WHERE c.visible
         GROUP BY c.id
         ORDER BY max(a.published_at) NULLS FIRST, c.place
         LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Internal("没有可写的课程".into()))
}

/// The public lesson of a visible course that is due for revision: news on its subjects first,
/// then its review date; the most read first, then the one longest unrevised. Each keyword is
/// looked for once in the week's stories, and the stories it is found in counted for each lesson
/// that keeps it, so the check stays quick however many lessons there are; a keyword found in more
/// than `COMMON` of them is left out.
async fn due(state: &AppState) -> Result<Option<Due>> {
    Ok(sqlx::query_as(
        "WITH lessons AS (
             SELECT a.* FROM articles a JOIN users u ON u.id = a.author_id
             JOIN courses c ON c.id = a.course AND c.visible
             WHERE u.status AND a.status = 'published'
         ),
         stories AS MATERIALIZED (
             SELECT i.id, i.first_seen_at, lower(i.title || ' ' || i.summary) AS text
             FROM items i JOIN topics t ON t.id = i.topic_id
             WHERE i.topic_id = ANY($1) AND t.enabled AND t.visible AND NOT i.hidden
               AND i.first_seen_at >= now() - make_interval(days => $2)
         ),
         words AS (
             SELECT l.id, l.updated_at, k AS word FROM lessons l, unnest(l.keywords) k
             WHERE l.updated_at <= now() - make_interval(days => $2)
         ),
         found AS (
             SELECT w.word, s.id, s.first_seen_at, count(*) OVER (PARTITION BY w.word) AS spread
             FROM (SELECT DISTINCT word FROM words) w JOIN stories s ON strpos(s.text, w.word) > 0
         ),
         news AS (
             SELECT w.id, count(DISTINCT f.id) >= $3 AS news
             FROM words w JOIN found f ON f.word = w.word AND f.first_seen_at > w.updated_at
             WHERE f.spread <= $4
             GROUP BY w.id
         )
         SELECT l.id, l.course, l.chapter, l.lesson, l.title, l.body, l.revision, l.cover, l.updated_at,
                COALESCE(n.news, false) AS news
         FROM lessons l LEFT JOIN news n ON n.id = l.id
         WHERE n.news OR l.review_at <= now()
         ORDER BY COALESCE(n.news, false) DESC, l.views DESC, l.updated_at
         LIMIT 1",
    )
    .bind(SOURCES.map(String::from).to_vec())
    .bind(NEWS_DAYS)
    .bind(NEWS)
    .bind(COMMON)
    .fetch_optional(&state.pool)
    .await?)
}

/// The lesson revised from a new plan, the site's stories and a search: it keeps its place, its
/// chapter, its figures and its cover, and takes the plan's keywords and review date.
async fn revised(state: &AppState, due: Due) -> Result<Uuid> {
    let track = &learn::course(state, &due.course)
        .await?
        .ok_or_else(|| AppError::Internal("课程已隐藏".into()))?;
    let lessons = learn::lessons(state, &track.id).await?;
    let (plan, mut usage) = plan(state, track, &lessons, Some(&due)).await?;
    let keywords = plan.keywords();
    let stories = stories(state, &keywords).await?;
    let mut found = look(
        state,
        Line::Lesson,
        &searching(state, track, &due.chapter, &plan, Some(&due)),
        &HashSet::new(),
    )
    .await;
    // The lesson's links were checked when it went out.
    found.known.extend(linked(&due.body));
    found.known.extend(own(state, &lessons, &stories));
    let (body, kept) = figures(&due.body);
    let material = format!(
        "{}\n课程目录：\n{}\n{}\n这一课的现有全文：\n标题：{}\n\n{body}\n\n站内最近收录的相关资料（可以引用）：\n{}\n线索：\n{}",
        describe(track),
        contents(state, &lessons, true),
        planned(&due.chapter, &plan),
        due.title,
        listed_stories(&stories),
        listed(&found.finds)
    );
    let drafted = draft(state, &format!("{WRITING}{REVISING}"), &material).await?;
    usage += found.usage;
    usage += drafted.usage;
    found.known.extend(drafted.cited);
    let article = finish(state, drafted.article, found.known, &kept, false).await?;
    let upkeep = Upkeep {
        keywords: &keywords,
        review_days: plan.review_days(),
    };
    revise(
        state,
        due.id,
        due.revision,
        due.cover,
        Some(upkeep),
        &article,
    )
    .await?;
    log(
        due.id,
        if due.news {
            "XABC's lesson revised after news"
        } else {
            "XABC's lesson revised for its review"
        },
        track,
        &article,
        usage,
    );
    Ok(due.id)
}

/// The briefing model plans the next lesson after the track's contents, or the revision of `due`.
async fn plan(
    state: &AppState,
    track: &Course,
    lessons: &[Lesson],
    due: Option<&Due>,
) -> Result<(Plan, Usage)> {
    let mut input = format!(
        "{}\n课程目录：\n{}",
        describe(track),
        contents(state, lessons, false)
    );
    if let Some(due) = due {
        let _ = write!(
            input,
            "\n要修订的是第 {} 课《{}》，在「{}」一章，上次修订于 {}。它现在的小标题：\n{}",
            due.lesson,
            due.title,
            due.chapter,
            day(state, due.updated_at),
            bullets(&headings(&due.body))
        );
    }
    let reply = ai::request(
        state,
        json!({
            "model": state.config.ai_insight_model,
            "reasoning": { "effort": "medium" },
            "store": false,
            "instructions": format!("{PLANNING}{}", if due.is_some() { AGAIN } else { NEXT }),
            "input": [{ "role": "user", "content": input }],
            "text": { "format": {
                "type": "json_schema", "name": "plan", "strict": true,
                "schema": ai::object(json!({
                    "chapter": { "type": "string" },
                    "title": { "type": "string" },
                    "goal": { "type": "string" },
                    "outline": { "type": "array", "items": { "type": "string" } },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "review_days": { "type": "integer" },
                })),
            } },
        }),
    )
    .await?;
    let plan = serde_json::from_value(ai::parse_payload(&reply)?)
        .map_err(|_| AppError::Internal("课程计划不符合约定的结构".into()))?;
    Ok((plan, Usage::from_reply(&reply)))
}

/// What the site gathered in the last 30 days on the plan's subjects, newest first: its title,
/// summary and address.
async fn stories(state: &AppState, keywords: &[String]) -> Result<Vec<(String, String, String)>> {
    let patterns: Vec<String> = keywords
        .iter()
        .map(|keyword| crate::site::containing(keyword))
        .collect();
    if patterns.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT i.title, i.summary, i.url FROM items i JOIN topics t ON t.id = i.topic_id
         WHERE t.id = ANY($1) AND t.enabled AND t.visible AND NOT i.hidden
           AND i.first_seen_at >= now() - INTERVAL '30 days'
           AND (i.title ILIKE ANY($2) OR i.summary ILIKE ANY($2))
         ORDER BY i.first_seen_at DESC
         LIMIT $3",
    )
    .bind(SOURCES.map(String::from).to_vec())
    .bind(&patterns)
    .bind(STORIES)
    .fetch_all(&state.pool)
    .await?)
}

/// The site's own pages a lesson may link to: the track's lessons, the stories it was given and
/// the price list of 𝑳𝑳𝑴.
fn own(state: &AppState, lessons: &[Lesson], stories: &[(String, String, String)]) -> Vec<String> {
    lessons
        .iter()
        .map(|lesson| absolute(state, &format!("/articles/{}", lesson.id)))
        .chain(stories.iter().map(|(_, _, url)| url.clone()))
        .chain([absolute(state, "/llm")])
        .filter_map(|url| identity(&url))
        .collect()
}

fn describe(track: &Course) -> String {
    format!(
        "板块：{}（{}）\n板块说明：{}",
        track.title, track.intro, track.brief
    )
}

/// The track's contents by chapter, each lesson with its address when the writer may link to it.
fn contents(state: &AppState, lessons: &[Lesson], links: bool) -> String {
    if lessons.is_empty() {
        return "（还没有课。）\n".into();
    }
    let mut contents = String::new();
    let numbers = learn::numbered(lessons.iter().map(|lesson| lesson.chapter.as_str()));
    let mut chapter = 0;
    for (lesson, number) in lessons.iter().zip(numbers) {
        if number != chapter {
            chapter = number;
            let _ = writeln!(contents, "第 {number} 章 {}", lesson.chapter);
        }
        let _ = write!(contents, "- 第 {} 课《{}》", lesson.lesson, lesson.title);
        if links {
            let _ = write!(
                contents,
                "（{}）",
                absolute(state, &format!("/articles/{}", lesson.id))
            );
        }
        contents.push('\n');
    }
    contents
}

fn planned(chapter: &str, plan: &Plan) -> String {
    format!(
        "这一课：《{}》，在「{chapter}」一章\n学完能做到：{}\n要点：\n{}",
        plan.title.trim(),
        plan.goal.trim(),
        bullets(&plan.outline)
    )
}

/// What the scout looks for: the lesson as planned and, for a revision, what came since.
fn searching(
    state: &AppState,
    track: &Course,
    chapter: &str,
    plan: &Plan,
    due: Option<&Due>,
) -> String {
    let mut input = format!(
        "今天是 {}。\n{}\n{}",
        day(state, Utc::now()),
        describe(track),
        planned(chapter, plan)
    );
    if let Some(due) = due {
        let _ = write!(
            input,
            "\n这是修订：这一课上次修订于 {}，重点找那之后的新版本、新变化和新做法。",
            day(state, due.updated_at)
        );
    }
    input
}

fn listed_stories(stories: &[(String, String, String)]) -> String {
    if stories.is_empty() {
        return "（没有。）\n".into();
    }
    stories
        .iter()
        .map(|(title, summary, url)| format!("- {title}：{summary}（{url}）\n"))
        .collect()
}

fn bullets(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("- {}\n", line.trim()))
        .collect()
}

fn day(state: &AppState, at: DateTime<Utc>) -> String {
    at.with_timezone(&state.config.timezone)
        .format("%Y-%m-%d")
        .to_string()
}

/// The body's headings, which tell a revision's planner what the lesson covers.
fn headings(body: &str) -> Vec<String> {
    let mut headings = Vec::new();
    let mut heading: Option<String> = None;
    for event in Parser::new_ext(body, MARKDOWN) {
        match event {
            Event::Start(Tag::Heading { .. }) => heading = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => headings.extend(heading.take()),
            Event::Text(text) | Event::Code(text) => {
                if let Some(heading) = &mut heading {
                    heading.push_str(&text);
                }
            }
            _ => {}
        }
    }
    headings
}

fn log(id: Uuid, done: &str, track: &Course, article: &Finished, usage: Usage) {
    tracing::info!(
        %id, course = track.id.as_str(), title = article.title, pictures = article.pictures.len(),
        input_tokens = usage.input_tokens, output_tokens = usage.output_tokens,
        web_searches = usage.server_side_tool_usage_details.web_search_calls,
        cost_ticks = usage.cost_in_usd_ticks,
        "{done}"
    );
}

pub(super) const SEARCHING: &str = "你是 XABC「学习」栏目的资料编辑。XABC 在 xabcnews.com 写一门门循序渐进的课程，读者是想系统学习的中文读者。
输入是这次要写的一课，也可能是一课的修订。请你{scope}，找写这一课用得上的资料，越具体越好：概念的权威解释、安装与配置步骤、命令、接口与参数、示例代码、版本差异、常见问题与解决办法、真实的使用经验。优先官方来源和最新的稳定版本，写明资料对应的版本和日期；过时的资料不要。
";

const PLANNING: &str = "你是 XABC「学习」栏目的课程主编。XABC 在 xabcnews.com 写一门门循序渐进的课程，每个板块是一串编号的课，分成若干章。输入是一个板块的说明（读者、范围和建议的章节路线）和已经写好的课程目录。
- title 是这一课的标题，具体、清楚，不超过 30 个字，不写「第几课」。
- goal 用一句话说学完这一课能做到什么；outline 列出这一课要讲的 3–8 个要点。
- keywords 是至多 5 个关键词，用来在新闻里找这一课的相关资料、发现它该更新了：只写这一课具体讲的功能、命令、版本、库或芯片的名字（如 Claude Code hooks、ESP-SR WakeNet、pgvector），不写只是产品名或泛泛的概念（如 Claude Code、Agent、大语言模型）；讲原理和方法、不依赖具体产品的课留空。
- review_days 是这一课多少天后该复查：讲具体命令、界面、配置、版本、价格或新产品的写 30–60，讲方法、原理和思路的写 90–180。
严格按 JSON Schema 输出。
";

const NEXT: &str = "这次安排这个板块的下一课：紧接在目录最后一课之后，循序渐进，在已有内容的基础上往前走一步，不重复目录里讲过的内容。先按建议的路线推进，路线走完后，继续往更深入、更新的方向扩展；目录还是空的，就从第一课开始。chapter 是这一课所在的章：一章一般 3–8 课，建议路线里的一项是一个主题，通常要几课才讲透；还没讲透就照抄最后一课那一章的名字，讲透了再开新的一章，写新章的名字，简短，不超过 16 个字，不写「第几章」。";

const AGAIN: &str = "这次安排目录里一课的修订，输入的最后写着是哪一课和它现在的小标题。修订是为了让这一课保持最新、讲得更好：按最新的官方版本更新内容，补上新的变化、做法和常见坑，改正过时或讲得不清楚的地方；仍讲这一课在目录里的主题，不和别的课重复。chapter 照抄这一课现在所在的章名；goal 和 outline 写修订后的这一课，outline 里标出要更新或补充的地方。";

const WRITING: &str = "你是 XABC，在 xabcnews.com 的「学习」栏目写课程，读者是想系统学习的中文读者。
下面是板块的说明、课程目录、这一课的计划、站内最近收录的相关资料和编辑找来的线索。需要时用 web_search 继续查官方文档，然后写出这一课的完整课文。
怎么写：
- 开头一两句说清学完这一课能做到什么；需要先学过目录里的哪几课，写一句「前置：」并链接到那几课。
- 先把概念讲透，必要时用类比；再一步步带着做：命令、代码和配置写完整、能照着做，代码放进代码块并标明语言；硬件接线用表格写清引脚。
- 写明适用的版本、平台和日期，内容以官方文档为准；查不到的不写，拿不准的提醒读者对照官方文档确认。
- 讲常见的坑和排查方法，最后给 2–3 个由浅入深的动手练习。
- 篇幅以讲透为准，不注水。目录里前面的课讲过的内容不重复，需要时一句话带过并链接到那一课。
- 文末用「参考资料」小标题列出这一课依据的官方文档和资料，链接到原页面。
- 标题用计划里的标题，可以改得更准确，不写「第几课」。
- 配图帮读者看懂：界面截图、开发板和硬件照片优先用资料页面里的图；示意图画成不带文字的插图。
";

const REVISING: &str = "- 这次是修订：输入里有这一课的现有全文和新的资料。在现有全文的基础上把它改得更好：按官方文档的最新版本更新命令、接口、配置和版本号，改正过时或不准确的地方，讲得更清楚，补上新的变化、做法和常见坑；结构可以调整，交回修订后的完整全文。现有配图写作 ![](keep:1)、![](keep:2)……，单独成段，可以保留、挪动或删掉，不要改写，下面也不要加图注；新图照常用 ![](img:N)。
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lesson_keeps_five_distinct_keywords_and_a_review_within_bounds() {
        let plan = |keywords: &[&str], review_days| Plan {
            chapter: String::new(),
            title: String::new(),
            goal: String::new(),
            outline: Vec::new(),
            keywords: keywords
                .iter()
                .map(|keyword| (*keyword).to_owned())
                .collect(),
            review_days,
        };
        let planned = plan(
            &[
                " Claude Code hooks ",
                "x",
                "claude code HOOKS",
                "ESP-SR",
                "a",
                "MCP",
                "RAG",
                "LoRA",
                "vLLM",
            ],
            7,
        );
        assert_eq!(
            planned.keywords(),
            ["claude code hooks", "esp-sr", "mcp", "rag", "lora"]
        );
        assert_eq!(planned.review_days(), 30);
        assert_eq!(plan(&[], 90).review_days(), 90);
        assert_eq!(plan(&[], 400).review_days(), 180);
        assert!(plan(&["x"], 90).keywords().is_empty());
    }

    #[test]
    fn a_revision_planner_reads_the_lessons_headings() {
        assert_eq!(
            headings("开头。\n\n## 安装 `idf.py`\n\n正文\n\n### 常见的坑\n\n- 列表\n\n## 参考资料"),
            ["安装 idf.py", "常见的坑", "参考资料"]
        );
    }
}
