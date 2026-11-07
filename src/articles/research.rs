//! 调研: readers ask XABC to look something up, and the article it researches and writes goes out
//! under the reader's pen name. Questions wait in one queue in the database, so a restart loses
//! none: those being written first, then Pro before Free, each in the order asked; XABC writes up
//! to `NEWS_MAX_CONCURRENT_RESEARCH` at once (`tick`), beside the run slots collection and its
//! other lines share, so a reader's question never waits behind them. A reader asks as many a day as their plan allows, an
//! administrator, who waits with Pro, as many as they like; a question taken back before XABC
//! starts on it, or one it could not write, gives its allowance back. The AI looks at a question
//! before it joins the queue, and at the article, the question with it, before it goes public, as
//! at 分享's. Pro readers and administrators may keep their articles to themselves. The words are
//! XABC's: only an administrator edits them.
use super::{Quota, check, parse, take, xabc};
use crate::{
    auth::User,
    error::{ApiJson, AppError, Result},
    state::AppState,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, QueryBuilder};
use uuid::Uuid;

/// Where 调研 lists its articles.
pub(crate) const PATH: &str = "/research";
pub(super) const QUESTION_CHARS: usize = 1000;
/// About how many minutes a research takes, which tells a reader how long theirs waits.
const MINUTES: i64 = 8;
/// How the research XABC is writing is marked among the jobs in flight, before its id.
const JOB: &str = "__research__";

/// 调研's name, in the page's language.
pub(crate) fn label(en: bool) -> &'static str {
    crate::site::choose(en, "调研", "𝑹𝒆𝒔𝒆𝒂𝒓𝒄𝒉")
}

pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/api/research", get(list).post(ask))
        .route("/api/research/{id}", patch(publicize).delete(remove))
}

/// A question, and whether its article goes public: only Pro may say no.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    question: String,
    public: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Public {
    public: bool,
}

/// A question waiting, being written or failed, whether the reader asked it, and how it stands
/// while it waits (`standing`).
#[derive(FromRow, Serialize)]
struct Queued {
    id: Uuid,
    question: String,
    author: String,
    mine: bool,
    pro: bool,
    public: bool,
    asked_at: DateTime<Utc>,
    started_at: Option<DateTime<Utc>>,
    writing: bool,
    failed: bool,
    ahead: Option<i64>,
    minutes: Option<i64>,
    now: Option<bool>,
}

/// A research article, whether the reader asked for it and whether its author is on Pro now or
/// an administrator, who asks with Pro.
#[derive(FromRow, Serialize)]
struct Researched {
    id: Uuid,
    title: String,
    author: String,
    mine: bool,
    status: String,
    held: Option<String>,
    views: i64,
    created_at: DateTime<Utc>,
    pro: bool,
}

/// Whether a reader may keep research to themselves and waits with Pro: on Pro, or an
/// administrator. A handler reads it once, so a plan ending meanwhile changes nothing.
fn private(user: &User) -> bool {
    user.is_admin() || user.is_pro()
}

/// The questions this process is writing, by their marks among the jobs in flight. One marked
/// started that isn't among them was cut off by a restart.
fn writing(state: &AppState) -> Vec<Uuid> {
    state
        .in_flight()
        .iter()
        .filter_map(|job| job.strip_prefix(JOB)?.parse().ok())
        .collect()
}

/// Pushes the questions waiting, all but failed ones and those being `written`, in the order
/// XABC takes them, each with how many wait before it (`ahead`): any cut off by a restart first,
/// then Pro before Free, each in the order asked.
fn queue(query: &mut QueryBuilder<sqlx::Postgres>, written: &[Uuid]) {
    query
        .push(
            "SELECT id, row_number() OVER (ORDER BY started_at IS NULL, NOT pro, asked_at, id) - 1 AS ahead
             FROM research WHERE NOT failed AND id <> ALL(",
        )
        .push_bind(written.to_vec())
        .push(")");
}

/// Pushes how a waiting question `q` of `queue` stands while `written` are being written,
/// `writers` at a time: how many wait before it (`ahead`), about how many minutes until it is
/// written (`minutes`), and whether a writer is free for it at once (`now`).
fn standing(query: &mut QueryBuilder<sqlx::Postgres>, written: usize, writers: usize) {
    let (written, writers) = (written as i64, writers as i64);
    query
        .push("q.ahead, ((q.ahead + ")
        .push_bind(written)
        .push(") / ")
        .push_bind(writers)
        .push(" + 1) * ")
        .push_bind(MINUTES)
        .push(" AS minutes, q.ahead + ")
        .push_bind(written)
        .push(" < ")
        .push_bind(writers)
        .push(" AS now");
}

/// How many research a reader may ask for in a day, as `private` says; an administrator, any
/// number.
fn allowance(state: &AppState, user: &User, private: bool) -> Option<u8> {
    match (user.is_admin(), private) {
        (true, _) => None,
        (false, true) => Some(state.config.research_pro_daily),
        (false, false) => Some(state.config.research_free_daily),
    }
}

/// The reader's questions and research articles, or for an administrator everyone's questions
/// and latest 30 articles; with what the reader may do: keep research to themselves, how many
/// they may ask a day and have left today; and how many XABC writes at once.
async fn list(State(state): State<AppState>, user: User) -> Result<Json<Value>> {
    let admin = user.is_admin();
    let (written, writers) = (writing(&state), state.config.max_concurrent_research);
    let mut query = QueryBuilder::new("SELECT r.id, r.question, w.name AS author, r.author_id = ");
    query
        .push_bind(user.id)
        .push(" AS mine, r.pro, r.public, r.asked_at, r.started_at, r.id = ANY(")
        .push_bind(written.clone())
        .push(") AS writing, r.failed, ");
    standing(&mut query, written.len(), writers);
    // Only what waits stands somewhere: what is being written, or failed, waits for nothing.
    query.push(" FROM research r JOIN authors w ON w.user_id = r.author_id LEFT JOIN (");
    queue(&mut query, &written);
    let queue: Vec<Queued> = query
        .push(") q ON q.id = r.id WHERE r.author_id = ")
        .push_bind(user.id)
        .push(" OR ")
        .push_bind(admin)
        .push(" ORDER BY r.failed, q.ahead NULLS FIRST, r.asked_at")
        .build_query_as()
        .fetch_all(&state.pool)
        .await?;
    let mut articles: Vec<Researched> = sqlx::query_as(
        "SELECT a.id, a.title, w.name AS author, a.author_id = $1 AS mine, a.status, a.held,
                a.views, a.created_at, u.role = 'admin' OR COALESCE(u.pro_until > now(), false) AS pro
         FROM articles a JOIN authors w ON w.user_id = a.author_id JOIN users u ON u.id = a.author_id
         WHERE a.question IS NOT NULL AND (a.author_id = $1 OR $2)
         ORDER BY a.created_at DESC, a.id DESC LIMIT $3",
    )
    .bind(user.id)
    .bind(admin)
    .bind(admin.then_some(30_i64))
    .fetch_all(&state.pool)
    .await?;
    // A reader isn't told why an article waits: it would help word around the review.
    if !admin {
        for article in &mut articles {
            article.held = None;
        }
    }
    let used: i16 =
        sqlx::query_scalar("SELECT research FROM authors WHERE user_id = $1 AND day = $2")
            .bind(user.id)
            .bind(state.digest.today())
            .fetch_optional(&state.pool)
            .await?
            .unwrap_or(0);
    let private = private(&user);
    let limit = allowance(&state, &user, private);
    Ok(Json(json!({
        "private": private,
        "limit": limit,
        "left": limit.map(|limit| (i16::from(limit) - used).max(0)),
        "writers": writers,
        "writing": written.len(),
        "queue": queue,
        "articles": articles,
    })))
}

/// Puts a reader's question in the queue, against their allowance for the day, unless the AI
/// finds pornography, gambling, drugs or political risk in it; tells how it stands (`standing`).
/// The AI's look counts as one of the reviews the reader has a day, so no one keeps it busy.
async fn ask(
    State(state): State<AppState>,
    user: User,
    ApiJson(request): ApiJson<Asked>,
) -> Result<Json<Value>> {
    let question = request.question.trim();
    if question.is_empty() {
        return Err(AppError::BadRequest("写上想了解什么".into()));
    }
    if question.chars().count() > QUESTION_CHARS {
        return Err(AppError::BadRequest(
            format!("问题最多 {QUESTION_CHARS} 个字").into(),
        ));
    }
    if !sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM authors WHERE user_id = $1)")
        .bind(user.id)
        .fetch_one(&state.pool)
        .await?
    {
        return Err(AppError::Conflict(
            "调研以笔名公开发布，请先在工作台设置笔名".into(),
        ));
    }
    let pro = private(&user);
    if !request.public && !pro {
        return Err(AppError::Forbidden("不公开是 Pro 功能".into()));
    }
    let limit = allowance(&state, &user, pro);
    if limit == Some(0) && !pro {
        return Err(AppError::Forbidden(
            format!(
                "调研是 Pro 功能，升级 Pro 后每天可以调研 {} 次",
                state.config.research_pro_daily
            )
            .into(),
        ));
    }
    if !user.is_admin() {
        take(&state.pool, user.id, state.digest.today(), Quota::Reviews).await?;
        if let Some(reason) = check::question(&state, question).await {
            return Err(AppError::BadRequest(
                format!("这个问题不能调研：{reason}").into(),
            ));
        }
    }
    let id = Uuid::new_v4();
    let mut tx = state.pool.begin().await?;
    let day = match limit {
        Some(limit) => Some(
            take(
                &mut *tx,
                user.id,
                state.digest.today(),
                Quota::Research(limit),
            )
            .await?,
        ),
        None => None,
    };
    sqlx::query(
        "INSERT INTO research (id, author_id, question, pro, public, day) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(user.id)
    .bind(question)
    .bind(pro)
    .bind(request.public)
    .bind(day)
    .execute(&mut *tx)
    .await?;
    let written = writing(&state);
    let mut query = QueryBuilder::new("SELECT ");
    standing(
        &mut query,
        written.len(),
        state.config.max_concurrent_research,
    );
    query.push(" FROM (");
    queue(&mut query, &written);
    let (ahead, minutes, now): (i64, i64, bool) = query
        .push(") q WHERE id = ")
        .push_bind(id)
        .build_query_as()
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({ "id": id, "ahead": ahead, "minutes": minutes, "now": now }),
    ))
}

/// Whether a Pro reader's research goes public: the question's choice while it waits, the
/// article's status once written. Going public, the article is reviewed by the AI as a
/// submitted 分享 is, and an administrator's goes out at once. Each switch makes a new revision,
/// so a review of an earlier one publishes nothing, while its refusal still holds.
async fn publicize(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
    ApiJson(request): ApiJson<Public>,
) -> Result<Json<Value>> {
    if !private(&user) {
        return Err(AppError::Forbidden("不公开是 Pro 功能".into()));
    }
    let id = parse(&id)?;
    if sqlx::query("UPDATE research SET public = $3 WHERE id = $1 AND author_id = $2")
        .bind(id)
        .bind(user.id)
        .bind(request.public)
        .execute(&state.pool)
        .await?
        .rows_affected()
        > 0
    {
        return Ok(Json(json!({ "public": request.public })));
    }
    let status = match (request.public, user.is_admin()) {
        (false, _) => "draft",
        (true, true) => "published",
        (true, false) => "pending",
    };
    let revision: i32 = sqlx::query_scalar(
        "UPDATE articles SET status = $3, revision = revision + 1,
             published_at = CASE WHEN $3 = 'published' THEN COALESCE(published_at, now()) ELSE published_at END
         WHERE id = $1 AND author_id = $2 AND question IS NOT NULL AND (status = 'draft') = $4
         RETURNING revision",
    )
    .bind(id)
    .bind(user.id)
    .bind(status)
    .bind(request.public)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| AppError::Conflict("调研不存在或刚有变化，请刷新后再试".into()))?;
    let status = match status {
        "pending" => check::settled(&state, id, revision).await?.0,
        status => status.to_owned(),
    };
    Ok(Json(json!({ "status": status })))
}

/// Takes a waiting question back, giving back its allowance, or clears one XABC could not
/// write, which gave its allowance back when it failed. An administrator takes back anyone's,
/// even while it is written.
async fn remove(
    State(state): State<AppState>,
    user: User,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    let taken: i64 = sqlx::query_scalar(
        "WITH taken AS (
             DELETE FROM research
             WHERE id = $1 AND (author_id = $2 OR $3) AND (started_at IS NULL OR failed OR $3)
             RETURNING author_id, CASE WHEN NOT failed THEN day END AS day
         ), given AS (
             UPDATE authors w SET research = research - 1 FROM taken t
             WHERE w.user_id = t.author_id AND w.day = t.day AND w.research > 0
         )
         SELECT count(*) FROM taken",
    )
    .bind(parse(&id)?)
    .bind(user.id)
    .bind(user.is_admin())
    .fetch_one(&state.pool)
    .await?;
    if taken == 0 {
        return Err(AppError::NotFound("调研不存在或已经开始写".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Starts writing the first questions in the queue while XABC writes fewer than it may at once,
/// each marked among the jobs in flight while it is written; the scheduler calls this on every
/// tick. A question cut off by a restart comes first again.
pub(crate) async fn tick(state: &AppState) {
    let written = writing(state);
    let free = state
        .config
        .max_concurrent_research
        .saturating_sub(written.len());
    if free == 0 {
        return;
    }
    let mut query =
        QueryBuilder::new("UPDATE research SET started_at = now() WHERE id IN (SELECT id FROM (");
    queue(&mut query, &written);
    let next = query
        .push(") q ORDER BY ahead LIMIT ")
        .push_bind(free as i64)
        .push(") RETURNING id, question")
        .build_query_as::<(Uuid, String)>()
        .fetch_all(&state.pool)
        .await;
    let next = match next {
        Ok(next) => next,
        Err(error) => {
            tracing::warn!(%error, "could not read the research queue");
            return;
        }
    };
    for (id, question) in next {
        let job = format!("{JOB}{id}");
        state.in_flight().insert(job.clone());
        let worker = state.clone();
        state.spawn_worker(async move {
            tokio::select! {
                _ = worker.shutdown.cancelled() => {}
                written = write(&worker, id, &question) => {
                    if let Err(error) = written {
                        tracing::warn!(%id, %error, "XABC could not write a research");
                        if let Err(error) = fail(&worker, id).await {
                            tracing::warn!(%id, %error, "could not mark a research failed");
                        }
                    }
                }
            }
            worker.in_flight().remove(&job);
        });
    }
}

/// Researches the question and saves the article, each once more if it fails; then a reader's
/// article that goes public is reviewed by the AI, the question with it.
async fn write(state: &AppState, id: Uuid, question: &str) -> Result<()> {
    let article = match xabc::researched(state, id, question).await {
        Ok(article) => article,
        Err(error) => {
            tracing::info!(%id, %error, "XABC researches a question again");
            xabc::researched(state, id, question).await?
        }
    };
    let status = match save(state, id, question, &article).await {
        Ok(status) => status,
        Err(error) => {
            tracing::info!(%id, %error, "XABC saves a research again");
            save(state, id, question, &article).await?
        }
    };
    match status.as_deref() {
        None => tracing::info!(%id, "a research was taken back while XABC wrote it"),
        Some("pending") => {
            if let Err(error) = check::settled(state, id, 1).await {
                tracing::warn!(%id, %error, "the AI could not review a research");
            }
        }
        Some(_) => {}
    }
    Ok(())
}

/// Saves the article under the reader's pen name as the question's id: out for review, or kept
/// to the reader as they chose; an administrator's goes out at once. Its status, or nothing for
/// a question taken back meanwhile; a save whose answer was lost is found as it was saved.
async fn save(
    state: &AppState,
    id: Uuid,
    question: &str,
    article: &xabc::Finished,
) -> Result<Option<String>> {
    let mut tx = state.pool.begin().await?;
    let Some((author, status)) = sqlx::query_as::<_, (i64, String)>(
        "DELETE FROM research r USING users u WHERE r.id = $1 AND u.id = r.author_id
         RETURNING r.author_id,
             CASE WHEN NOT r.public THEN 'draft' WHEN u.role = 'admin' THEN 'published' ELSE 'pending' END",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(
            sqlx::query_scalar("SELECT status FROM articles WHERE id = $1 AND revision = 1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?,
        );
    };
    let subject = Some(xabc::Subject::Question(question));
    xabc::save(&mut tx, id, author, &status, subject, article).await?;
    tx.commit().await?;
    tracing::info!(%id, %status, "XABC wrote a research");
    Ok(Some(status))
}

/// Marks a question XABC could not write, giving back the allowance it took.
async fn fail(state: &AppState, id: Uuid) -> Result<()> {
    sqlx::query(
        "WITH failed AS (
             UPDATE research SET failed = true WHERE id = $1 AND NOT failed RETURNING author_id, day
         )
         UPDATE authors w SET research = research - 1 FROM failed f
         WHERE w.user_id = f.author_id AND w.day = f.day AND w.research > 0",
    )
    .bind(id)
    .execute(&state.pool)
    .await?;
    Ok(())
}
