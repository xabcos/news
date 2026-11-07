//! 数码 · 评测: XABC reviews a product 数码 collects, one article a product, and brings the review
//! up to date as reviewers and owners report more. Each time it takes the product 数码 discusses
//! most beyond what XABC has written about it (`next`): a product whose official specifications
//! 数码 checked gets a review once reviews and owners' posts about it come in, and a review whose
//! product gathers `FRESH_STORIES` more of them, `REVISE_AFTER_DAYS` days after its last update
//! at least, is brought up to date. XABC never held the product: a review puts together the
//! official specifications, what reviewers and owners report and their pictures, with the
//! product's official photographs that 数码 keeps (`official`). Searching, writing and pictures
//! are 分享's (`super`).
use super::{
    Finished, KEEPING, Line, Picture, Subject, draft, figure, figures, finish, identity, linked,
    listed, log, look, photographed, publish, revise,
};
use crate::{
    error::{AppError, Result},
    fetch::NewsImage,
    media,
    state::AppState,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::{collections::HashSet, fmt::Write};
use uuid::Uuid;

/// A review is brought up to date once this many new reviews and owners' posts came after its
/// last update, and these days have passed since.
const FRESH_STORIES: i64 = 5;
const REVISE_AFTER_DAYS: i32 = 3;
/// A product is reviewed once this many reviews and owners' posts about it came in these last
/// days.
const FIRST_STORIES: i64 = 2;
const WINDOW_DAYS: i32 = 14;
/// The official photographs a new review is offered, and the stories its writer reads.
const OFFICIAL_PICTURES: usize = 3;
const STORIES: i64 = 24;
/// An update also reads the stories of this last hour before the previous one was saved: they
/// came while it was being written.
const OVERLAP_HOURS: i32 = 1;

/// A product 数码 collects, as the entry that knows most about it tells: one product may sit in
/// two of 数码's topics under the same key.
#[derive(sqlx::FromRow)]
struct Product {
    key: String,
    name: String,
    name_en: String,
    creator: String,
    /// The guide 数码 wrote from the official pages it read, once it has.
    guide: Option<Value>,
    images: Value,
}

impl Product {
    /// The product as the models are told it: `iPhone Duo（iPhone Duo，Apple）`.
    fn describe(&self) -> String {
        format!("产品：{}（{}，{}）", self.name, self.name_en, self.creator)
    }

    /// The product, and what 数码 checked against its official pages.
    fn material(&self) -> String {
        let mut material = format!("{}\n", self.describe());
        if let Some(guide) = &self.guide {
            let _ = writeln!(
                material,
                "数码栏目核实过的官方介绍：{}\n官方参数：",
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

/// What `next` chose: the product's key in 数码, and its review with the revision read, if any.
#[derive(sqlx::FromRow)]
struct Next {
    key: String,
    article: Option<Uuid>,
    revision: Option<i32>,
}

/// A review to bring up to date, as it reads now.
#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    title: String,
    body: String,
    revision: i32,
    cover: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

/// Reviews the product 数码 discusses most, or brings its review up to date; none while 数码
/// has nothing new to review.
pub(super) async fn write(
    state: &AppState,
    author: i64,
    since: DateTime<Utc>,
) -> Result<Option<Uuid>> {
    let Some(next) = next(state, author).await? else {
        return Ok(None);
    };
    let product: Product = sqlx::query_as(
        "SELECT key, name, name_en, creator, guide, images FROM catalog_entries
         WHERE key = $1 AND topic_id = ANY($2)
         ORDER BY guide IS NULL, jsonb_array_length(images) DESC LIMIT 1",
    )
    .bind(&next.key)
    .bind(topics())
    .fetch_one(&state.pool)
    .await?;
    if let (Some(article), Some(revision)) = (next.article, next.revision) {
        let due: Due = sqlx::query_as(
            "SELECT id, title, body, revision, cover, updated_at FROM articles
             WHERE id = $1 AND revision = $2",
        )
        .bind(article)
        .bind(revision)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| AppError::Conflict("评测刚被改过".into()))?;
        return revised(state, &product, due).await.map(Some);
    }
    let (stories, linked_stories) = stories(state, &product, None).await?;
    let mut found = look(
        state,
        Line::Review,
        SEARCHING,
        &product.describe(),
        &HashSet::new(),
    )
    .await;
    found.known.extend(linked_stories);
    let (kept, official) = official(state, &product).await;
    found.known.extend(
        official
            .iter()
            .filter_map(|picture| picture.credit.as_ref())
            .filter_map(|(_, page)| identity(page)),
    );
    let mut material = product.material();
    if !kept.is_empty() {
        material.push_str("官方图片：\n");
        for (number, picture) in official.iter().enumerate() {
            let by = picture.credit.as_ref().map_or("", |(by, _)| by.as_str());
            let _ = writeln!(material, "{}. {}（{by}）", number + 1, product.name);
        }
    }
    let _ = write!(
        material,
        "数码栏目收集的评测和用户分享：\n{stories}\n线索：\n{}",
        listed(&found.finds)
    );
    let drafted = photographed(draft(state, WRITING, &material).await?);
    found.usage += drafted.usage;
    found.known.extend(drafted.cited);
    let mut article = finish(state, drafted.article, found.known, &kept, false).await?;
    placed(&mut article, official);
    let id = publish(
        state,
        author,
        since,
        Some(Subject::Product(&product.key)),
        &article,
    )
    .await?;
    log(
        id,
        "XABC's 评测 written",
        &product.key,
        &article,
        found.usage,
    );
    Ok(Some(id))
}

/// 数码's topics, which 评测 reviews the products of.
fn topics() -> Vec<&'static str> {
    crate::catalog::Section::Gadgets.topics().to_vec()
}

/// The product 数码 discusses most beyond what XABC has written about it, its stories counted
/// across the topics that hold it, and its review if it has one: a product with checked
/// specifications and no review once `FIRST_STORIES` reviews and owners' posts came in the last
/// `WINDOW_DAYS` days, a reviewed one once `FRESH_STORIES` came after the review's last update,
/// `REVISE_AFTER_DAYS` days ago at least. A story two topics both collected counts once. The
/// most stories go first, then the most reviews and posts, then the newest.
async fn next(state: &AppState, author: i64) -> Result<Option<Next>> {
    Ok(sqlx::query_as(
        "SELECT e.key, a.id AS article, a.revision FROM catalog_entries e
         JOIN catalog_items c ON c.entry_id = e.id
         JOIN items i ON i.id = c.item_id AND NOT i.hidden
         LEFT JOIN articles a ON a.product = e.key
         WHERE e.topic_id = ANY($1)
           AND i.first_seen_at > COALESCE(a.updated_at, now() - make_interval(days => $2))
           AND (a.id IS NULL
                OR a.author_id = $3 AND a.status = 'published'
                   AND a.updated_at <= now() - make_interval(days => $4))
         GROUP BY e.key, a.id
         HAVING (a.id IS NOT NULL
                 OR EXISTS (SELECT 1 FROM catalog_entries g
                            WHERE g.key = e.key AND g.topic_id = ANY($1) AND g.guide IS NOT NULL))
            AND count(DISTINCT i.fingerprint) FILTER (WHERE c.role <> 'news')
                >= CASE WHEN a.id IS NULL THEN $5 ELSE $6 END
         ORDER BY count(DISTINCT i.fingerprint) DESC,
                  count(DISTINCT i.fingerprint) FILTER (WHERE c.role <> 'news') DESC,
                  max(i.first_seen_at) DESC
         LIMIT 1",
    )
    .bind(topics())
    .bind(WINDOW_DAYS)
    .bind(author)
    .bind(REVISE_AFTER_DAYS)
    .bind(FIRST_STORIES)
    .bind(FRESH_STORIES)
    .fetch_optional(&state.pool)
    .await?)
}

/// The reviews, owners' posts and news 数码 collected about the product in any of its topics,
/// each once (as a review or a post where any topic took it so), reviews and posts first, the
/// newest first: those of the window for a new review,
/// those after `after` for an update. With them, the pages they are on, which the review may
/// link to.
async fn stories(
    state: &AppState,
    product: &Product,
    after: Option<DateTime<Utc>>,
) -> Result<(String, Vec<String>)> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT role, title, summary, url FROM (
             SELECT DISTINCT ON (i.fingerprint) c.role, i.title, i.summary, i.url, i.first_seen_at
             FROM items i
             JOIN catalog_items c ON c.item_id = i.id
             JOIN catalog_entries e ON e.id = c.entry_id
             WHERE e.key = $1 AND e.topic_id = ANY($2) AND NOT i.hidden
               AND i.first_seen_at > COALESCE($3, now() - make_interval(days => $4))
             ORDER BY i.fingerprint, c.role = 'news', i.first_seen_at DESC) stories
         ORDER BY role = 'news', first_seen_at DESC LIMIT $5",
    )
    .bind(&product.key)
    .bind(topics())
    .bind(after)
    .bind(WINDOW_DAYS)
    .bind(STORIES)
    .fetch_all(&state.pool)
    .await?;
    let mut listed = String::new();
    for (role, title, summary, url) in &rows {
        let role = match role.as_str() {
            "review" => "评测",
            "share" => "用户分享",
            _ => "动态",
        };
        let _ = writeln!(listed, "- 【{role}】{title}：{summary}（{url}）");
    }
    let pages = rows.iter().filter_map(|(.., url)| identity(url)).collect();
    Ok((listed, pages))
}

/// The product's official photographs that 数码 keeps, as figures a new review may place
/// (`keep:N`), each with its picture. The files stay where 数码 put them: each is only read to
/// tell its kind and size. One that can't be read is left out.
async fn official(state: &AppState, product: &Product) -> (Vec<String>, Vec<Picture>) {
    let images: Vec<NewsImage> = serde_json::from_value(product.images.clone()).unwrap_or_default();
    let jobs = images
        .into_iter()
        .filter_map(|image| Some((image.expires_at?, image)))
        .take(OFFICIAL_PICTURES)
        .enumerate()
        .map(|(index, (expires_at, image))| {
            let state = state.clone();
            move |slot| async move {
                let _slot = slot;
                let (bytes, _) = media::download_image(&state, &image.url, super::ACCEPT).await?;
                let (content_type, (width, height)) = crate::articles::kind(&bytes).ok()?;
                let picture = Picture {
                    id: Uuid::new_v4(),
                    url: image.url,
                    content_type,
                    width,
                    height,
                    expires_at,
                    credit: Some((image.credit, image.source_url)),
                };
                Some((index, picture, image.caption))
            }
        });
    let mut read: Vec<(usize, Picture, String)> = media::batch(state, jobs)
        .await
        .into_iter()
        .flatten()
        .collect();
    read.sort_by_key(|(index, ..)| *index);
    read.into_iter()
        .map(|(_, picture, caption)| (figure(&picture, &caption), picture))
        .unzip()
}

/// Keeps the official photographs the review shows; when it shows no picture at all, the first
/// stands for it on the lists.
fn placed(article: &mut Finished, official: Vec<Picture>) {
    let shown = crate::articles::pictures(&article.body, None);
    let (placed, unplaced): (Vec<Picture>, Vec<Picture>) = official
        .into_iter()
        .partition(|picture| shown.contains(&picture.id));
    if shown.is_empty() {
        article.cover = unplaced.into_iter().next();
    }
    article.pictures.extend(placed);
}

/// The review brought up to date with what 数码 gathered since and both searches found. Its
/// figures stay where the writer keeps them; the cover follows the picture it opens with.
async fn revised(state: &AppState, product: &Product, due: Due) -> Result<Uuid> {
    let after = due.updated_at - chrono::Duration::hours(OVERLAP_HOURS.into());
    let (stories, linked_stories) = stories(state, product, Some(after)).await?;
    let last = due
        .updated_at
        .with_timezone(&state.config.timezone)
        .format("%Y-%m-%d");
    let input = format!(
        "{}\n这篇评测上次更新于 {last}，重点找那之后的新评测、长期使用体验、固件更新、价格变化和已知问题。",
        product.describe()
    );
    let mut found = look(state, Line::Review, SEARCHING, &input, &HashSet::new()).await;
    // The review's links were checked when it went out, and 数码's stories were read.
    found.known.extend(linked(&due.body));
    found.known.extend(linked_stories);
    let (body, kept) = figures(&due.body);
    let material = format!(
        "{}这篇评测的现有全文：\n标题：{}\n\n{body}\n\n数码栏目在上次更新之后收到的评测和用户分享：\n{stories}\n线索：\n{}",
        product.material(),
        due.title,
        listed(&found.finds)
    );
    let drafted =
        photographed(draft(state, &format!("{WRITING}{REVISING}{KEEPING}"), &material).await?);
    found.usage += drafted.usage;
    found.known.extend(drafted.cited);
    let article = finish(state, drafted.article, found.known, &kept, false).await?;
    let cover = crate::articles::pictures(&article.body, None)
        .first()
        .copied()
        .or(due.cover);
    revise(state, due.id, due.revision, cover, &article).await?;
    log(
        due.id,
        "XABC's 评测 brought up to date",
        &product.key,
        &article,
        found.usage,
    );
    Ok(due.id)
}

const SEARCHING: &str = "你是 XABC「数码 · 评测」的资料编辑。XABC 在 xabcnews.com 为热门数码产品写综合评测：把官方参数、各家评测和真实用户的体验整理成一篇完整详细、图文并茂的文章，读者是关心科技和数码产品的中文读者。
输入是这次要评测的产品。请你{scope}，找出写评测用得上的资料，越多越具体越好：专业媒体和独立评测者的上手评测与结论（测了什么、优缺点，给了分数的写明是哪家评测给的多少分），视频评测的要点，长期使用报告，用户亲自拍摄的开箱、实拍和使用体验，常见问题和故障，固件与系统更新，价格变化，以及和主要竞品的对比。只找这款产品的资料，分清同系列的其他型号和上一代。pictures 优先给有产品实拍的页面：用户亲自拍照的 X 帖子、评测文章。
";

const WRITING: &str = "你是 XABC，在 xabcnews.com 的「数码 · 评测」栏目写综合评测，读者是关心科技和数码产品的中文读者。
下面是这次要评测的产品：数码栏目核实过的官方参数、它收集的评测和用户分享，以及两位编辑找来的线索。需要时用 web_search 继续查，然后写一篇完整的中文综合评测。
怎么写：
- XABC 没有亲自上手这款产品：文章综合官方资料、各家评测和真实用户的体验。上手的感受、测试结果和分数都写明出自哪家评测或哪位用户，不写成自己的体验。
- 开头用两三句话给出结论：它是什么、最大的亮点和短板、适合谁、不适合谁。
- 接着写清它是什么：定位、价格和版本、上市时间；然后用表格列出主要参数，只用核实过的官方参数，不自己补。
- 再按这类产品的看点逐项写透：设计与做工、屏幕、性能、续航与充电、影像、系统与软件、手感与日常使用等，按品类取舍；每一项都写清各家评测怎么说，有分歧的写清各方的说法。
- 用「各家评测怎么说」概括主要评测的结论和评分（只写评测里确实给出的分数，写明出自哪家，不自己打综合分），用「用户怎么说」写真实用户的开箱、实拍和吐槽；常见问题和已知故障单独写。
- 最后写优点和缺点、和主要竞品比、值不值得买，建议什么样的人买哪个版本。
- 篇幅不限，越详细越好，资料多就写得更长，但不为凑字数重复或空谈；用小标题、表格和列表组织。
- 产品名第一次出现写官方中文名并附原名；标题带上产品名和最核心的结论。
- 配图都用真实照片，不画插图，尽量图文并茂地展示产品的外观、细节和使用场景：pictures 的 page 填带图的页面（用户亲自拍照的 X 帖子、评测文章），prompt 留空。输入里列出的官方图片写作 ![](keep:1)、![](keep:2)……，单独成段，放在合适的位置，不要改写，下面也不要加图注；用不上可以不放。你看不到页面上的图，图注就写这张图出自哪篇评测或哪位用户、讲的什么，不描述画面。
";

const REVISING: &str = "- 这次是更新：输入里有这篇评测的现有全文、数码栏目在上次更新之后收到的评测和用户分享，以及新的线索。在现有全文的基础上把它改得更好：把新的评测结论、长期使用体验、固件更新、价格变化和已知问题写进去，修正已过时的说法，开头的结论随之更新；交回更新后的完整全文。
";
