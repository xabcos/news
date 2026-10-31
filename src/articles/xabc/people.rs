//! 时代: XABC profiles the people this age talks about, one article a person. Each time both
//! scouts name the people talked about most now, and one of them is written about (`pick`):
//! anew, or once their profile is `REVISE_AFTER_DAYS` days old brought up to date. Searching,
//! writing and pictures are 分享's (`super`), but a profile shows only pictures from pages, and
//! the lists show the person: its cover is their portrait on Wikipedia (`portrait`).
use super::{
    ACCEPT, KEEPING, Line, Picture, Subject, draft, figures, finish, linked, listed, log, look,
    photographed, pick, publish, revise, uploaded,
};
use crate::{
    error::{AppError, Result},
    media,
    state::AppState,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::{collections::HashSet, fmt::Write};
use url::Url;
use uuid::Uuid;

/// A profile is brought up to date once this many days have passed since it last was.
const REVISE_AFTER_DAYS: i32 = 3;

/// A profile to bring up to date, as it reads now.
#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    title: String,
    body: String,
    revision: i32,
    cover: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

/// Profiles the person talked about most, or brings their profile up to date.
pub(super) async fn write(state: &AppState, author: i64, since: DateTime<Utc>) -> Result<Uuid> {
    // People profiled lately wait, and so do those whose profile an administrator took back;
    // the scouts are told who they are.
    let waiting: Vec<(String, String)> = sqlx::query_as(
        "SELECT person, title FROM articles WHERE person IS NOT NULL
           AND (status <> 'published' OR updated_at > now() - make_interval(days => $1))
         ORDER BY updated_at DESC",
    )
    .bind(REVISE_AFTER_DAYS)
    .fetch_all(&state.pool)
    .await?;
    let today = Utc::now()
        .with_timezone(&state.config.timezone)
        .date_naive();
    let mut input = format!("今天是 {today}。\n");
    if !waiting.is_empty() {
        input.push_str("这些人物最近写过，这次不要选：\n");
        for (_, title) in &waiting {
            let _ = writeln!(input, "- {title}");
        }
    }
    let mut found = look(state, Line::People, &input, &HashSet::new()).await;
    let passed: HashSet<String> = waiting.into_iter().map(|(person, _)| person).collect();
    let (person, finds) = pick(found.finds, &passed, usize::MAX)
        .ok_or_else(|| AppError::Internal("两路检索都没有找到可写的人物".into()))?;
    let name = finds[0].1.subject.trim();
    let material = format!("今天是 {today}。\n人物：{name}\n");
    let due: Option<Due> = sqlx::query_as(
        "SELECT id, title, body, revision, cover, updated_at FROM articles WHERE person = $1",
    )
    .bind(&person)
    .fetch_optional(&state.pool)
    .await?;
    let Some(due) = due else {
        let material = format!("{material}线索：\n{}", listed(&finds));
        let drafted = photographed(draft(state, WRITING, &material).await?);
        found.usage += drafted.usage;
        found.known.extend(drafted.cited);
        let mut article = finish(state, drafted.article, found.known, &[], false).await?;
        article.cover = portrait(state, name).await;
        let id = publish(
            state,
            author,
            since,
            Some(Subject::Person(&person)),
            &article,
        )
        .await?;
        log(id, "XABC's profile written", &person, &article, found.usage);
        return Ok(id);
    };
    // The profile's links were checked when it went out.
    found.known.extend(linked(&due.body));
    let (body, kept) = figures(&due.body);
    let last = due
        .updated_at
        .with_timezone(&state.config.timezone)
        .format("%Y-%m-%d");
    let material = format!(
        "{material}这篇人物介绍上次更新于 {last}，现有全文：\n标题：{}\n\n{body}\n\n他最近受关注的线索：\n{}",
        due.title,
        listed(&finds)
    );
    let drafted =
        photographed(draft(state, &format!("{WRITING}{REVISING}{KEEPING}"), &material).await?);
    found.usage += drafted.usage;
    found.known.extend(drafted.cited);
    let mut article = finish(state, drafted.article, found.known, &kept, false).await?;
    // The cover is the portrait, or else the picture the profile opens with now.
    article.cover = portrait(state, name).await;
    let cover = article
        .cover
        .as_ref()
        .map(|cover| cover.id)
        .or_else(|| crate::articles::pictures(&article.body).first().copied())
        .or(due.cover);
    revise(state, due.id, due.revision, cover, &article).await?;
    log(
        due.id,
        "XABC's profile brought up to date",
        &person,
        &article,
        found.usage,
    );
    Ok(due.id)
}

/// The photograph that leads the person's article on English Wikipedia, which shows the person:
/// none for a page without one, or for a disambiguation page, which names several people. A small
/// portrait still beats any other picture, so only its type is checked, not its size.
async fn portrait(state: &AppState, name: &str) -> Option<Picture> {
    let mut url = Url::parse("https://en.wikipedia.org/w/api.php").expect("wiki API URL");
    url.query_pairs_mut().extend_pairs([
        ("action", "query"),
        ("prop", "pageimages|pageprops"),
        ("piprop", "thumbnail"),
        ("pithumbsize", "960"),
        ("redirects", "1"),
        ("format", "json"),
        ("formatversion", "2"),
        ("titles", name),
    ]);
    let reply: Value = match media::wikimedia(state, url).await {
        Ok(bytes) => serde_json::from_slice(&bytes).ok()?,
        Err(error) => {
            tracing::warn!(%error, name, "Wikipedia was not asked for a portrait");
            return None;
        }
    };
    let page = &reply["query"]["pages"][0];
    if !page["pageprops"]["disambiguation"].is_null() {
        return None;
    }
    let (bytes, _) =
        media::download_image(state, page["thumbnail"]["source"].as_str()?, ACCEPT).await?;
    let source = format!(
        "https://en.wikipedia.org/wiki/{}",
        page["title"].as_str()?.replace(' ', "_")
    );
    uploaded(state, bytes, Some(("en.wikipedia.org".into(), source))).await
}

pub(super) const SEARCHING: &str = "你是 XABC「时代」栏目的选题编辑。XABC 在 xabcnews.com 为这个时代全球最受关注的人物写全方位的人物长文，他们再次成为焦点时就更新，读者是关心科技、科学和世界的中文读者。
今天请你{scope}，放眼全球的网络热点，找出最近一两天讨论最多、最受关注的人物，至少 8 位，按受关注的程度从高到低排列。美国、中国、欧洲、日本、韩国、印度等各地的人物都要看，中文和英文的讨论都要搜；一个人在本国或本地区是焦点，就按那里的热度算，不因为英文讨论少就排到后面。以全球的科技人才为主：热点企业的创始人、掌门人和核心技术人才，AI 的研究者和工程师，物理、数学等基础科学的科学家；政治人物和网络热点人物，只选真正处在舆论中心的，政治人物只选中国以外的。只选公众人物，不选未成年人，也不选偶然走红的普通人。
每条线索是一个人：person 写他最常用的英文名（中国人用拼音），只写名字，不加头衔、括号和说明；title 一句话说明他是谁、为什么现在受关注；why 写热度依据，比如讨论、转发、报道和事件；points 写他这次受关注的事，以及与他相关的公司和机构的近况；pictures 优先给有他本人照片的页面。
";

const WRITING: &str = "你是 XABC，在 xabcnews.com 的「时代」栏目写人物，读者是关心科技、科学和世界的中文读者。
下面是这次要写的人物和两位编辑找来的线索。多用 web_search 把他的公开资料查全：维基百科等资料、官方主页、权威媒体的报道和长篇访谈、他本人的文章与演讲；然后写一篇完整的中文人物长文。
怎么写：
- 把这个人写透，越详细越好：先写他是谁、为什么现在大家都在谈论他，接着用「你可能不知道」列出 3 到 5 条关于他的冷知识，每条一句话；再写他的成长与求学、职业生涯的每个关键阶段、做成的事和代表作（讲清做了什么、为什么重要）、主要观点与做事方式、外界的评价与争议的来龙去脉；与他相关的公司和机构，写清来历、业务、产品、数据和最新动向；用「轶事与个人生活」写他的小故事、习惯与怪癖、名言、恩怨与友谊，以及公开的感情与家庭生活；最后在「近况」里按时间写清最近发生的事，带上日期。
- 像一个做足功课、会讲故事的记者：具体、生动，用事实、细节、数字和他本人的话说话；可以有判断，但不吹捧、不贬低。篇幅不限，资料足够时写到一万字以上，资料多就写得更长，但不为凑字数重复或空谈；用小标题组织。
- 只写公开、查得到出处的信息：经历、数字和引语都要来自资料和检索；传闻写明是传闻，有争议的事写清各方的说法。感情与家庭更严：只写本人公开过或主流媒体确认过的婚姻、伴侣、离婚和子女情况，以及他本人公开谈过的经历，媒体广泛报道的争议注明出处，没有这类公开信息就不写这一块；不写未经证实的传闻、小报爆料和猜测，不写未成年子女的细节和身为普通人的家人的隐私，也不写住址和联系方式。
- 人名第一次出现写通行的中文译名并附原名，标题带上他的中文名。
- 配图都用页面上的真实照片，不画插图，pictures 的 page 填带图的页面，prompt 留空。第一张放在正文开头，是他本人的照片：page 填他的维基百科条目（英文或中文），没有条目才用他的官方介绍页或本人的 X 帖子。其余配上与他的工作、演讲、产品和公司有关的报道和页面；你看不到这些页面上的图，图注就写这张图出自哪篇报道、讲的什么事，不说画面里是谁。
";

const REVISING: &str = "- 这次是更新：输入里有这篇人物介绍的现有全文和他最近受关注的线索。在现有全文的基础上把它改得更好：把新的动态按时间写进「近况」，补上以前缺的资料，修正不准确和已过时的说法；「近况」只留最近一两年的要事，更早的写进经历。交回更新后的完整全文。";
