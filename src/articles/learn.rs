//! 学习: XABC's courses (`courses`). `/learn` maps them by group, `/learn/{course}` lists a
//! course's lessons by chapter, and a lesson reads on the article page, which shows where it
//! stands (`Place`). XABC writes and revises the lessons (`xabc::lesson`) and designs new courses
//! (`xabc::course`); an administrator opens one with a sentence and hides any (`admin`).
use crate::{
    auth::{self, User},
    digest::{Rendered, conditional},
    error::{ApiJson, AppError, Result},
    reading::{self, Reader},
    site::{Chrome, choose, escape, localized},
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, patch, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::FromRow;
use std::{collections::HashMap, fmt::Write};
use uuid::Uuid;

/// Where the courses live.
pub(crate) const PATH: &str = "/learn";

/// The section's name, in the page's language.
pub(crate) fn label(en: bool) -> &'static str {
    choose(en, "学习", "𝑳𝒆𝒂𝒓𝒏")
}

/// A course, as the pages show it and XABC teaches it: its address, its group and name in both
/// languages, a line on what it teaches, and what XABC is told about its readers, its scope and
/// the chapters it should go through.
#[derive(Clone, FromRow)]
pub(crate) struct Course {
    pub(crate) id: String,
    pub(crate) grp: String,
    pub(crate) grp_en: String,
    pub(crate) title: String,
    pub(crate) title_en: String,
    pub(crate) intro: String,
    pub(crate) intro_en: String,
    pub(crate) brief: String,
    /// How an administrator last turned it, and when.
    pub(crate) direction: String,
    pub(crate) adjusted_at: Option<DateTime<Utc>>,
}

/// The visible courses: group by group in the order the groups came, each group's courses in the
/// order they came.
pub(crate) async fn courses(state: &AppState) -> Result<Vec<Course>> {
    Ok(sqlx::query_as(
        "SELECT id, grp, grp_en, title, title_en, intro, intro_en, brief, direction, adjusted_at
         FROM courses WHERE visible
         ORDER BY min(place) OVER (PARTITION BY grp), place",
    )
    .fetch_all(&state.pool)
    .await?)
}

/// A visible course.
pub(crate) async fn course(state: &AppState, id: &str) -> Result<Option<Course>> {
    Ok(sqlx::query_as(
        "SELECT id, grp, grp_en, title, title_en, intro, intro_en, brief, direction, adjusted_at
         FROM courses WHERE id = $1 AND visible",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?)
}

const LEARN: Chrome<'static> = Chrome {
    current: "learn",
    section: "reads learn",
    stylesheet: "/reads.css?v=18",
    script: "/reads.js?v=9",
    feed: ("News", "/feed.xml"),
};

pub(super) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route(PATH, get(map))
            .route("/learn/{course}", get(contents)),
    )
    .layer(tower_http::compression::CompressionLayer::new())
    .route("/api/admin/courses", get(admin).post(open))
    .route("/api/admin/courses/{course}", patch(show))
    .route("/api/admin/courses/{course}/direction", post(direct))
}

/// A course as the administrator's list shows it, with its brief to read.
#[derive(FromRow, Serialize)]
struct Listed {
    id: String,
    title: String,
    group: String,
    brief: String,
    source: String,
    visible: bool,
    lessons: i64,
}

/// Every course, hidden ones too, in the order the pages give them, and what the administrator's
/// last sentence came to.
async fn admin(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let courses: Vec<Listed> = sqlx::query_as(
        "SELECT c.id, c.title, c.grp AS \"group\", c.brief, c.source, c.visible,
                count(a.id) FILTER (WHERE a.status = 'published') AS lessons
         FROM courses c LEFT JOIN articles a ON a.course = c.id
         GROUP BY c.id
         ORDER BY min(c.place) OVER (PARTITION BY c.grp), c.place",
    )
    .fetch_all(&state.pool)
    .await?;
    let (designing, error) = super::xabc::course::progress();
    Ok(Json(
        json!({ "courses": courses, "designing": designing, "error": error }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Idea {
    idea: String,
}

/// An administrator opens a course with a sentence; XABC designs it in the background.
async fn open(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<Idea>,
) -> Result<(StatusCode, Json<Value>)> {
    auth::require_admin(&user)?;
    let idea = request.idea.trim().to_owned();
    if !(4..=200).contains(&idea.chars().count()) {
        return Err(AppError::BadRequest("用 4–200 个字说想开的课".into()));
    }
    super::xabc::course::request(&state, idea)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "designing": true }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    change: String,
}

/// An administrator turns a course with a sentence; XABC rewrites it in the background.
async fn direct(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Change>,
) -> Result<(StatusCode, Json<Value>)> {
    auth::require_admin(&user)?;
    let change = request.change.trim().to_owned();
    if !(4..=200).contains(&change.chars().count()) {
        return Err(AppError::BadRequest("用 4–200 个字说怎么调整".into()));
    }
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1)")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?
    {
        return Err(AppError::NotFound("课程不存在".into()));
    }
    super::xabc::course::adjust(&state, id, change)?;
    Ok((StatusCode::ACCEPTED, Json(json!({ "designing": true }))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Shown {
    visible: bool,
}

/// Shows or hides a course: a hidden one is neither listed nor extended, and its lessons stay.
async fn show(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Shown>,
) -> Result<Json<Value>> {
    auth::require_admin(&user)?;
    let changed = sqlx::query("UPDATE courses SET visible = $2 WHERE id = $1")
        .bind(&id)
        .bind(request.visible)
        .execute(&state.pool)
        .await?
        .rows_affected();
    if changed == 0 {
        return Err(AppError::NotFound("课程不存在".into()));
    }
    Ok(Json(json!({ "visible": request.visible })))
}

#[derive(Deserialize)]
struct Params {
    lang: Option<String>,
}

/// A public lesson as a track's contents list it.
#[derive(FromRow)]
pub(super) struct Lesson {
    pub(super) id: Uuid,
    pub(super) chapter: String,
    pub(super) lesson: i32,
    pub(super) title: String,
    pub(super) published_at: DateTime<Utc>,
    pub(super) updated_at: DateTime<Utc>,
}

/// A track's public lessons in order.
pub(super) async fn lessons(state: &AppState, track: &str) -> Result<Vec<Lesson>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.chapter, a.lesson, a.title, a.published_at, a.updated_at
         FROM articles a JOIN users u ON u.id = a.author_id
         WHERE u.status AND a.status = 'published' AND a.course = $1 ORDER BY a.lesson",
    )
    .bind(track)
    .fetch_all(&state.pool)
    .await?)
}

/// Each lesson's chapter number: a chapter starts where a lesson names another than the lesson
/// before it.
pub(super) fn numbered<'a>(chapters: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut last = None;
    let mut number = 0;
    chapters
        .into_iter()
        .map(|chapter| {
            if last != Some(chapter) {
                number += 1;
                last = Some(chapter);
            }
            number
        })
        .collect()
}

/// The courses by group, in the order `courses` gives them.
fn groups(courses: &[Course]) -> Vec<((&str, &str), Vec<&Course>)> {
    let mut groups: Vec<((&str, &str), Vec<&Course>)> = Vec::new();
    for course in courses {
        match groups.last_mut() {
            Some(((group, _), members)) if *group == course.grp => members.push(course),
            _ => groups.push(((&course.grp, &course.grp_en), vec![course])),
        }
    }
    groups
}

/// A page with its contents beside it: on a wide screen they stand left of the page and stay in
/// view, on a narrow one the page stands alone (reads.css).
pub(super) fn columns(side: &str, main: &str) -> String {
    format!("<div class=learn-layout>{side}<div class=learn-main>{main}</div></div>")
}

/// The way back to every course, at the head of the contents.
fn back(en: bool) -> String {
    format!(
        "<a class=learn-back href=\"{}\">← {}</a>",
        localized(PATH, en),
        choose(en, "全部课程", "All courses")
    )
}

fn href(course: &Course, en: bool) -> String {
    escape(&localized(&format!("{PATH}/{}", course.id), en))
}

/// Beside a course's page: every course by group, this one marked.
fn catalogue(courses: &[Course], current: &str, en: bool) -> String {
    let mut html = format!(
        "<nav class=learn-side aria-label=\"{}\">{}",
        choose(en, "全部课程", "All courses"),
        back(en)
    );
    for ((name, name_en), members) in groups(courses) {
        let _ = write!(html, "<h2>{}</h2><ul>", escape(choose(en, name, name_en)));
        for course in members {
            let _ = write!(
                html,
                "<li><a href=\"{}\"{}>{}</a></li>",
                href(course, en),
                if course.id == current {
                    " aria-current=\"page\""
                } else {
                    ""
                },
                escape(choose(en, &course.title, &course.title_en))
            );
        }
        html.push_str("</ul>");
    }
    html.push_str("</nav>");
    html
}

fn day(at: DateTime<Utc>, state: &AppState) -> String {
    at.with_timezone(&state.config.timezone)
        .format("%m-%d")
        .to_string()
}

/// How many lessons and chapters a course has.
fn size(lessons: usize, chapters: usize, en: bool) -> String {
    choose(
        en,
        &format!("{lessons} 课 · {chapters} 章"),
        &format!("{lessons} lessons · {chapters} chapters"),
    )
    .to_owned()
}

/// `/learn`: the courses by group, how many lessons each has and when one last changed.
async fn map(
    State(state): State<AppState>,
    // The page is the same for everyone, but a preview of it still needs an administrator.
    _: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.lang.as_deref() == Some("en");
    let courses = courses(&state).await?;
    let counts: HashMap<String, (i64, DateTime<Utc>)> = sqlx::query_as::<_, (String, i64, DateTime<Utc>)>(
        "SELECT a.course, count(*), max(a.updated_at) FROM articles a JOIN users u ON u.id = a.author_id
         WHERE u.status AND a.status = 'published' AND a.course IS NOT NULL GROUP BY a.course",
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|(course, count, updated)| (course, (count, updated)))
    .collect();
    let mut content = format!(
        "<header class=reads-heading><h1>{}</h1><p>{}</p></header><div class=learn-map>",
        label(en),
        choose(
            en,
            "AI 与 ESP32 课程，持续更新，常学常新",
            "Courses on AI and the ESP32, growing and kept current around the clock"
        ),
    );
    for ((name, name_en), members) in groups(&courses) {
        let _ = write!(
            content,
            "<section class=learn-group><h2>{}</h2>",
            escape(choose(en, name, name_en))
        );
        for course in members {
            let meta = match counts.get(&course.id) {
                Some((count, updated)) => choose(
                    en,
                    &format!("{count} 课 · 更新 {}", day(*updated, &state)),
                    &format!("{count} lessons · updated {}", day(*updated, &state)),
                )
                .to_owned(),
                None => choose(en, "第一课正在写", "The first lesson is being written").to_owned(),
            };
            let _ = write!(
                content,
                "<div class=learn-track><h3><a href=\"{}\">{}</a><span class=learn-meta> · {meta}</span></h3><p>{}</p></div>",
                href(course, en),
                escape(choose(en, &course.title, &course.title_en)),
                escape(choose(en, &course.intro, &course.intro_en)),
            );
        }
        content.push_str("</section>");
    }
    content.push_str("</div>");
    let html = crate::site::layout(
        &state,
        en,
        PATH,
        &format!("{} · 𝑵𝒆𝒘𝒔", label(en)),
        choose(
            en,
            "AI 与 ESP32 的系列课程：入门、提示词、编程、Agent、RAG、模型 API、本地模型、创作与实战项目，持续更新",
            "Courses on AI and the ESP32, from the basics, prompting, coding, agents, RAG, model APIs and local models to creating and projects, kept current",
        ),
        &content,
        None,
        &LEARN,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// Lessons new or revised within this many days lead a course's page, unless they are all it has.
const RECENT_DAYS: i64 = 7;
/// How many of them, at most.
const RECENT: usize = 5;

/// `/learn/{course}`: the course's lessons by chapter, after the latest new or revised ones, with
/// every course beside them.
async fn contents(
    State(state): State<AppState>,
    // The page is the same for everyone, but a preview of it still needs an administrator.
    _: Reader,
    Path(id): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.lang.as_deref() == Some("en");
    let courses = courses(&state).await?;
    let course = courses
        .iter()
        .find(|course| course.id == id)
        .ok_or_else(|| AppError::NotFound("课程不存在".into()))?;
    let lessons = lessons(&state, &course.id).await?;
    let numbers = numbered(lessons.iter().map(|lesson| lesson.chapter.as_str()));
    let link = |lesson: &Lesson| localized(&format!("/articles/{}", lesson.id), en);
    let name = |lesson: &Lesson| {
        format!(
            "<span class=learn-number>{}</span>{}",
            choose(
                en,
                &format!("第 {} 课", lesson.lesson),
                &format!("Lesson {}", lesson.lesson)
            ),
            escape(&lesson.title)
        )
    };
    let time = |at: DateTime<Utc>| {
        format!(
            "<time datetime=\"{}\">{}</time>",
            at.to_rfc3339(),
            day(at, &state)
        )
    };
    let mut content = format!(
        "<header class=reads-heading><h1>{}</h1><p>{}</p></header>",
        escape(choose(en, &course.title, &course.title_en)),
        escape(choose(en, &course.intro, &course.intro_en)),
    );
    match lessons.iter().map(|lesson| lesson.updated_at).max() {
        Some(updated) => {
            let _ = write!(
                content,
                "<p class=learn-stats>{} · {} {}</p>",
                size(
                    lessons.len(),
                    numbers.last().copied().unwrap_or_default(),
                    en
                ),
                choose(en, "最近更新", "updated"),
                day(updated, &state)
            );
        }
        None => {
            let _ = write!(
                content,
                "<p class=empty>{}</p>",
                choose(en, "第一课正在写。", "The first lesson is being written.")
            );
        }
    }
    let since = Utc::now() - chrono::Duration::days(RECENT_DAYS);
    let mut recent: Vec<&Lesson> = lessons
        .iter()
        .filter(|lesson| lesson.updated_at > since)
        .collect();
    recent.sort_by_key(|lesson| std::cmp::Reverse(lesson.updated_at));
    recent.truncate(RECENT);
    if !recent.is_empty() && recent.len() < lessons.len() {
        let _ = write!(
            content,
            "<section class=learn-recent><h2>{}</h2><ul>",
            choose(en, "最近更新", "Latest")
        );
        for lesson in recent {
            // A lesson revised after it went out says so; a new one says it is new.
            let revised = lesson.updated_at - lesson.published_at > chrono::Duration::hours(1);
            let _ = write!(
                content,
                "<li><span class=learn-badge>{}</span><a href=\"{}\">{}</a>{}</li>",
                if revised {
                    choose(en, "更新", "Updated")
                } else {
                    choose(en, "新", "New")
                },
                escape(&link(lesson)),
                name(lesson),
                time(lesson.updated_at)
            );
        }
        content.push_str("</ul></section>");
    }
    let mut chapter = 0;
    for (lesson, number) in lessons.iter().zip(&numbers) {
        if *number != chapter {
            if chapter > 0 {
                content.push_str("</ol></section>");
            }
            chapter = *number;
            let _ = write!(
                content,
                "<section class=learn-chapter><h2><span class=learn-number>{}</span>{}</h2><ol class=learn-lessons>",
                choose(en, &format!("第 {number} 章"), &format!("Chapter {number}")),
                escape(&lesson.chapter)
            );
        }
        let _ = write!(
            content,
            "<li><a href=\"{}\">{}</a>{}</li>",
            escape(&link(lesson)),
            name(lesson),
            time(lesson.updated_at)
        );
    }
    if chapter > 0 {
        content.push_str("</ol></section>");
    }
    let html = crate::site::layout(
        &state,
        en,
        &format!("{PATH}/{}", course.id),
        &format!(
            "{} · {}",
            choose(en, &course.title, &course.title_en),
            label(en)
        ),
        choose(en, &course.intro, &course.intro_en),
        &columns(&catalogue(&courses, &course.id, en), &content),
        None,
        &LEARN,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// Where a lesson stands: its course, its number and chapter, among the course's public lessons.
pub(super) struct Place {
    course: Course,
    lesson: i32,
    chapter: usize,
    name: String,
    lessons: Vec<Lesson>,
    numbers: Vec<usize>,
}

impl Place {
    /// The lesson numbered `lesson` in a visible course; a lesson not public, as a preview shows
    /// it, still finds its chapter and neighbours. A hidden course's lesson stands alone.
    pub(super) async fn find(
        state: &AppState,
        id: &str,
        chapter: &str,
        lesson: i32,
    ) -> Result<Option<Self>> {
        let Some(course) = course(state, id).await? else {
            return Ok(None);
        };
        let lessons = lessons(state, id).await?;
        let numbers = numbered(lessons.iter().map(|lesson| lesson.chapter.as_str()));
        // Its chapter continues the one before it when it bears that name, or else follows it.
        let chapter_number = match lessons.iter().rposition(|other| other.lesson < lesson) {
            Some(at) if lessons[at].chapter == chapter => numbers[at],
            Some(at) => numbers[at] + 1,
            None => 1,
        };
        Ok(Some(Self {
            course,
            lesson,
            chapter: chapter_number,
            name: chapter.to_owned(),
            lessons,
            numbers,
        }))
    }

    fn title(&self, en: bool) -> String {
        escape(choose(en, &self.course.title, &self.course.title_en))
    }

    /// Above the title: 学习 › the course › the chapter.
    pub(super) fn crumbs(&self, en: bool) -> String {
        format!(
            "<p class=learn-crumbs><a href=\"{}\">{}</a> › <a href=\"{}\">{}</a> › {} {}</p>",
            localized(PATH, en),
            label(en),
            href(&self.course, en),
            self.title(en),
            choose(
                en,
                &format!("第 {} 章", self.chapter),
                &format!("Chapter {}", self.chapter)
            ),
            escape(&self.name)
        )
    }

    /// Beside the lesson: its course's chapters, each folding its lessons away but its own, which
    /// stands open with the lesson marked. A lesson not public, as a preview shows it, is not among
    /// them, and opens none.
    pub(super) fn contents(&self, en: bool) -> String {
        let open = self
            .lessons
            .iter()
            .position(|lesson| lesson.lesson == self.lesson)
            .map(|at| self.numbers[at]);
        let mut html = format!(
            "<nav class=learn-side aria-label=\"{}\">{}<a class=learn-course href=\"{}\">{}</a><p class=learn-meta>{}</p>",
            choose(en, "课程目录", "Contents"),
            back(en),
            href(&self.course, en),
            self.title(en),
            size(
                self.lessons.len(),
                self.numbers.last().copied().unwrap_or_default(),
                en
            )
        );
        let mut chapter = 0;
        for (lesson, number) in self.lessons.iter().zip(&self.numbers) {
            if *number != chapter {
                if chapter > 0 {
                    html.push_str("</ol></details>");
                }
                chapter = *number;
                let _ = write!(
                    html,
                    "<details{}><summary><span class=learn-number>{number}</span>{}</summary><ol>",
                    if Some(chapter) == open { " open" } else { "" },
                    escape(&lesson.chapter)
                );
            }
            let _ = write!(
                html,
                "<li><a href=\"{}\"{}><span class=learn-number>{}</span>{}</a></li>",
                escape(&localized(&format!("/articles/{}", lesson.id), en)),
                if lesson.lesson == self.lesson {
                    " aria-current=\"page\""
                } else {
                    ""
                },
                lesson.lesson,
                escape(&lesson.title)
            );
        }
        if chapter > 0 {
            html.push_str("</ol></details>");
        }
        html.push_str("</nav>");
        html
    }

    /// Under the lesson: the lessons before and after it, and the way back to the contents.
    pub(super) fn steps(&self, en: bool) -> String {
        let step = |lesson: Option<&Lesson>, rel: &str, arrow: (&str, &str)| {
            lesson
                .map(|lesson| {
                    format!(
                        "<a rel={rel} href=\"{}\">{}{}{}</a>",
                        escape(&localized(&format!("/articles/{}", lesson.id), en)),
                        arrow.0,
                        escape(&format!(
                            "{} {}",
                            choose(
                                en,
                                &format!("第 {} 课", lesson.lesson),
                                &format!("Lesson {}", lesson.lesson)
                            ),
                            lesson.title
                        )),
                        arrow.1
                    )
                })
                .unwrap_or_default()
        };
        let previous = self
            .lessons
            .iter()
            .rev()
            .find(|other| other.lesson < self.lesson);
        let next = self.lessons.iter().find(|other| other.lesson > self.lesson);
        format!(
            "<nav class=learn-steps aria-label=\"{}\">{}{}</nav><div class=read-actions><a href=\"{}\">← {}</a></div>",
            choose(en, "上一课与下一课", "Previous and next lessons"),
            if previous.is_some() {
                step(previous, "prev", ("← ", ""))
            } else {
                "<span></span>".into()
            },
            step(next, "next", ("", " →")),
            href(&self.course, en),
            choose(
                en,
                &format!("{}目录", self.title(en)),
                &format!("{} contents", self.title(en))
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_are_numbered_where_their_names_change() {
        assert_eq!(
            numbered(["选型", "选型", "工程化", "工程化", "工程化", "外设"]),
            [1, 1, 2, 2, 2, 3]
        );
        // A name coming back later starts a chapter again, so the numbers keep the order.
        assert_eq!(numbered(["甲", "乙", "甲"]), [1, 2, 3]);
        assert!(numbered([]).is_empty());
    }

    #[test]
    fn courses_group_as_they_come() {
        let course = |id: &str, grp: &str| Course {
            id: id.into(),
            grp: grp.into(),
            grp_en: format!("{grp} en"),
            title: id.into(),
            title_en: id.into(),
            intro: String::new(),
            intro_en: String::new(),
            brief: String::new(),
            direction: String::new(),
            adjusted_at: None,
        };
        let courses = [course("a", "甲"), course("b", "甲"), course("c", "乙")];
        let grouped: Vec<((&str, &str), Vec<&str>)> = groups(&courses)
            .into_iter()
            .map(|(group, members)| (group, members.iter().map(|c| c.id.as_str()).collect()))
            .collect();
        assert_eq!(
            grouped,
            [
                (("甲", "甲 en"), vec!["a", "b"]),
                (("乙", "乙 en"), vec!["c"])
            ]
        );
    }
}
