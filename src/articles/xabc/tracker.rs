//! 追踪: XABC follows the events the whole web talks about, one article an event, brought up to
//! date as it develops. Each round both scouts rank the events talked about most now, those XABC
//! follows among them, and the hottest one with something new is written about (`pick`): anew,
//! or in the article it has once that has stood `WAIT_HOURS`. Searching, writing and pictures
//! are 分享's (`super`); like 时代's, an article shows only pictures from pages, and its cover is
//! the first of them, or for the lists alone one a page the finds name gives (`cover_from`).
use super::{
    Find, KEEPING, Line, Picture, Scout, Subject, draft, figures, finish, found, linked, listed,
    log, look, photographed, pick, publish, revise, uploaded,
};
use crate::{
    error::{AppError, Result},
    media,
    state::AppState,
};
use chrono::{DateTime, Utc};
use std::{
    collections::{BTreeMap, HashSet},
    fmt::Write,
    sync::{Mutex, PoisonError},
};
use uuid::Uuid;

/// An event written about waits this long before it is again, so the biggest has four turns a
/// day at most and the others get theirs.
const WAIT_HOURS: i64 = 6;
/// The scouts hear of the events followed in these last days, so that a development lands in
/// the article its event has, and of every event taken back, so that it stays out.
const TRACKED_DAYS: i32 = 30;
/// Only an event placed this high on either scout's list is hot enough to write about.
const TOP: usize = 5;
/// The pages tried at most for a cover when an article shows no picture of its own.
const COVER_PAGES: usize = 8;

/// Events whose article had nothing new to take, and when it was asked: they wait as if brought
/// up to date then. Kept in the process; after a restart they are looked at again.
static CHECKED: Mutex<BTreeMap<String, DateTime<Utc>>> = Mutex::new(BTreeMap::new());

/// An event XABC follows, or followed until an administrator took its article back.
#[derive(sqlx::FromRow)]
struct Tracked {
    event: String,
    title: String,
    published: bool,
    updated_at: DateTime<Utc>,
}

/// An article to bring up to date, as it reads now.
#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    title: String,
    body: String,
    revision: i32,
    cover: Option<Uuid>,
    published: bool,
    updated_at: DateTime<Utc>,
}

/// Follows the hottest event with something new, in a new article or in the one it has; none
/// when no event placed high enough may be written about now.
pub(super) async fn write(
    state: &AppState,
    author: i64,
    since: DateTime<Utc>,
) -> Result<Option<Uuid>> {
    let tracked: Vec<Tracked> = sqlx::query_as(
        "SELECT event, title, status = 'published' AS published, updated_at FROM articles
         WHERE event IS NOT NULL
           AND (status <> 'published' OR updated_at > now() - make_interval(days => $1))
         ORDER BY updated_at DESC",
    )
    .bind(TRACKED_DAYS)
    .fetch_all(&state.pool)
    .await?;
    let zone = state.config.timezone;
    let at = |time: DateTime<Utc>| time.with_timezone(&zone).format("%Y-%m-%d %H:%M");
    let now = Utc::now();
    let clock = format!("现在是 {}（{}）。\n", at(now), zone.name());
    let input = format!("{clock}{}", scouting(&tracked, at));
    let mut found = look(state, Line::Tracker, &input, &HashSet::new()).await;
    if found.answered == 0 {
        return Err(AppError::Internal("两路检索都失败了".into()));
    }
    // An event waits while its article is taken back, after it was written about, and after it
    // had nothing new to take.
    let wait = chrono::Duration::hours(WAIT_HOURS);
    let mut passed: HashSet<String> = tracked
        .iter()
        .filter(|tracked| !tracked.published || now - tracked.updated_at < wait)
        .map(|tracked| tracked.event.clone())
        .collect();
    passed.extend(checked(now - wait));
    let Some((event, finds)) = pick(found.finds, &passed, TOP) else {
        return Ok(None);
    };
    let material = format!("{clock}事件：{event}\n");
    let due: Option<Due> = sqlx::query_as(
        "SELECT id, title, body, revision, cover, status = 'published' AS published, updated_at
         FROM articles WHERE event = $1",
    )
    .bind(&event)
    .fetch_optional(&state.pool)
    .await?;
    let Some(due) = due else {
        let material = format!("{material}线索：\n{}", listed(&finds));
        let drafted = photographed(draft(state, WRITING, &material).await?);
        found.usage += drafted.usage;
        found.known.extend(drafted.cited);
        let mut article = finish(state, drafted.article, found.known, &[], false).await?;
        if crate::articles::pictures(&article.body).is_empty() {
            article.cover = cover_from(state, &finds).await;
        }
        let id = publish(state, author, since, Some(Subject::Event(&event)), &article).await?;
        log(id, "XABC's 追踪 written", &event, &article, found.usage);
        return Ok(Some(id));
    };
    // An administrator may have saved or taken back the article while the scouts searched.
    if !due.published || Utc::now() - due.updated_at < wait {
        return Ok(None);
    }
    // The article's links were checked when it went out.
    found.known.extend(linked(&due.body));
    let (body, kept) = figures(&due.body);
    let material = format!(
        "{material}这篇追踪上次更新于 {}，现有全文：\n标题：{}\n\n{body}\n\n最新的线索：\n{}",
        at(due.updated_at),
        due.title,
        listed(&finds)
    );
    let drafted =
        photographed(draft(state, &format!("{WRITING}{REVISING}{KEEPING}"), &material).await?);
    found.usage += drafted.usage;
    if drafted.article.body.trim().is_empty() {
        tracing::info!(
            event,
            input_tokens = found.usage.input_tokens,
            output_tokens = found.usage.output_tokens,
            cost_ticks = found.usage.cost_in_usd_ticks,
            "XABC's 追踪 had nothing new to take"
        );
        CHECKED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(event, Utc::now());
        return Ok(None);
    }
    found.known.extend(drafted.cited);
    let mut article = finish(state, drafted.article, found.known, &kept, false).await?;
    // The cover is the picture the article opens with now, or else the one it had, or else one
    // the finds' pages give.
    let mut cover = crate::articles::pictures(&article.body)
        .first()
        .copied()
        .or(due.cover);
    if cover.is_none() {
        article.cover = cover_from(state, &finds).await;
        cover = article.cover.as_ref().map(|picture| picture.id);
    }
    revise(state, due.id, due.revision, cover, &article).await?;
    log(
        due.id,
        "XABC's 追踪 brought up to date",
        &event,
        &article,
        found.usage,
    );
    Ok(Some(due.id))
}

/// What the scouts hear of the events XABC follows, by their keys with when each was last
/// brought up to date, and of the events taken back.
fn scouting<T: std::fmt::Display>(tracked: &[Tracked], at: impl Fn(DateTime<Utc>) -> T) -> String {
    let mut input = String::new();
    let (followed, dropped): (Vec<&Tracked>, Vec<&Tracked>) =
        tracked.iter().partition(|tracked| tracked.published);
    if !followed.is_empty() {
        input.push_str("正在追踪的事件（标识｜标题｜上次更新）：\n");
        for tracked in followed {
            let _ = writeln!(
                input,
                "- {}｜{}｜{}",
                tracked.event,
                tracked.title,
                at(tracked.updated_at)
            );
        }
    }
    if !dropped.is_empty() {
        input.push_str("不再追踪的事件（标识｜标题）：\n");
        for tracked in dropped {
            let _ = writeln!(input, "- {}｜{}", tracked.event, tracked.title);
        }
    }
    input
}

/// A cover for the lists when the article shows no picture of its own: the first picture a page
/// the finds name gives, the pages they offer pictures from first. Many news sites turn programs
/// away, and the writer can't tell which. The article page doesn't show the cover, so it owes no
/// caption. The tries are one image job, holding a slot within a batch's time, so a page or an
/// upload that never answers costs the cover and not the article.
async fn cover_from(state: &AppState, finds: &[(Scout, Find)]) -> Option<Picture> {
    let mut tried = HashSet::new();
    let pages: Vec<String> = finds
        .iter()
        .flat_map(|(_, find)| &find.pictures)
        .chain(finds.iter().flat_map(|(_, find)| &find.sources))
        .filter(|page| tried.insert(page.as_str()))
        .take(COVER_PAGES)
        .cloned()
        .collect();
    let worker = state.clone();
    let job = move |slot| async move {
        let _slot = slot;
        for page in pages {
            if let Some((bytes, credit)) = found(&worker, &page).await
                && let Some(picture) = uploaded(&worker, bytes, Some(credit)).await
            {
                return Some(picture);
            }
        }
        None
    };
    media::batch(state, [job])
        .await
        .into_iter()
        .flatten()
        .next()
}

/// The events found with nothing new to take since `after`; older marks go.
fn checked(after: DateTime<Utc>) -> Vec<String> {
    let mut checked = CHECKED.lock().unwrap_or_else(PoisonError::into_inner);
    checked.retain(|_, at| *at > after);
    checked.keys().cloned().collect()
}

pub(super) const SEARCHING: &str = "你是 XABC「追踪」栏目的选题编辑。XABC 在 xabcnews.com 为全网最受关注的热点事件各写一篇持续更新的追踪：事情怎么发生的、现在到了哪一步、各方怎么说、接下来会怎样，有了新进展就在同一篇上更新；读者是关心科技、商业和世界的中文读者。
今天请你{scope}，放眼全球的网络热点，找出最近一两天讨论最多、传播最广、还在发酵的事件，按热度从高到低排列，最多 10 件。美国、中国、欧洲、日本、韩国、印度等各地的事件都要看，中英文的讨论都要搜。以科技、互联网与 AI、公司与市场、加密货币、网络安全、科学与航天为主，也收全球关注的重大事故、灾害和突发事件；娱乐圈和明星的事，以及社会新闻一类的热点，都不收。
一件事要有起点、还会有后续：比如一次被盗、宕机、泄露或事故，一场诉讼或调查，一笔收购或一道禁令，一次翻车的发布，一项政策或法案的推进。一次性的发布、财报和融资，只有引发了连锁反应、各方还在跟进时才算；同一波连锁反应算一件事，要有来源说明它们出自同一个起因。旷日持久的战争、局势和选举本身不算一件事，其中具体的一次袭击、停火、谈判，或某一届选举的计票与结果争议可以。政治事件只选中国以外、真正处在全球舆论中心的。
输入里列出了正在追踪的事件。它们和别的事件一样按热度排进来，原样使用它的标识，不要换个说法另起标识；points 只写它上次更新之后（往前多看两个小时）的新进展，没有新进展就留空。不再追踪的事件不要列出，也不要换个标识再列。
每条线索是一件事：event 是它的标识，用三五个英文词加事件起始的年月（不用这次报道或更新的年月），比如 binance-hack-2026-10，只写标识；title 一句话说明发生了什么、现在到了哪一步；why 写热度依据，比如讨论、转发、报道和跟进；points 写时间线上的关键事实（带日期，能确定时带时间）、各方的说法和数字，注明出自谁，说法有变化的写清先后，数字写清是什么口径（比如受影响、被盗、冻结、追回、赔付）；pictures 优先给有现场照片的报道页面或 X 帖子。
";

const WRITING: &str = "你是 XABC，在 xabcnews.com 的「追踪」栏目追踪热点事件，读者是关心科技、商业和世界的中文读者。
下面是这次要追踪的事件和两位编辑找来的线索。多用 web_search 把这件事查全：当事方的公告和声明、官方通报、监管与法院文件、通讯社和权威媒体的报道与跟进、调查者和研究机构的报告、当事人和亲历者的发言；然后写一篇完整的中文追踪。
怎么写：
- 开头固定三部分：先用两三句话说清发生了什么、现在到了哪一步；接着是「最新进展」，按时间写最新的进展；然后是「时间线」，从这件事的起点到现在按时间顺序列出它本身的每个关键节点，每条注明出处，以前的同类事件和相关背景写到后面的小节里。日期都要写，能确定时写到几点几分；时间一律换算成输入里的时区（Asia/Shanghai 就写北京时间），不用另外解释时区。
- 接下来把这件事追踪完整，越详细越好：来龙去脉与背景，涉及的公司、机构和人物，事情是怎么发生的（技术事件讲清原理和影响范围），各方的说法与回应，调查、处置和追责的进展，对用户、市场、行业和政策的影响，历史上的同类事件和对比，争议和还没解开的疑问，接下来已经定下的日程和值得关注的节点。用小标题组织，篇幅不限，资料多就写得更长，但不为凑字数重复或空谈。
- 只写查得到出处的事实，关键的事实、数字和引语都要来自资料和检索。数字写清时间、口径和出处，口径不同的数字分开写：受影响、被盗、冻结、追回和赔付的金额各是各的，不能写成同一个数字的变化。早期的说法后来被推翻或修正的，在时间线里写清先后。分清已证实的事实、当事方的说法和外界的推测；归因和指控写明是谁说的，比如「某机构称……」；几家媒体转述同一份报告，不算几个独立的来源。
- 安全事件只写影响、原理和处置，不写攻击步骤、利用代码和能照着做的细节；不写普通受害者的姓名和隐私，不传播泄露的数据；伤亡数字注明时间和来源，不渲染血腥细节。
- 人名、公司和机构第一次出现写通行的中文译名并附原名。标题先写事件，再写它现在到了哪一步。
- 配图都用页面上的真实照片，不画插图：pictures 的 page 填带图的页面（报道、公告或 X 帖子），prompt 留空。第一张放在正文开头，是最能说明这件事的现场或相关照片。你看不到这些页面上的图，图注只写这篇报道讲的事和它与这件事的关系，不描述画面，出处由程序注明。
";

const REVISING: &str = "- 这次是更新：输入里有这篇追踪的现有全文和最新的线索。先用 web_search 查清上次更新之后的新进展，再在现有全文的基础上更新：新的进展写进「最新进展」；核对上一版「最新进展」里的事，时间线里还没有的补进去，已有的不重复；保留仍然有效的事实和出处，补上以前缺的资料，修正已被推翻或过时的说法，在时间线里写清先后；标题改成事件现在的状态。交回更新后的完整全文。
- 线索和检索里都没有现有全文之外的新事实时，这次不更新：body 留空，title 照写现在的标题，pictures 留空。
";

#[cfg(test)]
mod tests {
    use super::super::{Find, Scout, key};
    use super::*;

    fn find(scout: Scout, event: &str, points: &str) -> (Scout, Find) {
        (
            scout,
            Find {
                title: format!("{event} 的进展"),
                why: String::new(),
                points: points.into(),
                sources: Vec::new(),
                pictures: Vec::new(),
                subject: event.into(),
            },
        )
    }

    #[test]
    fn an_event_is_told_apart_by_the_words_of_its_slug() {
        assert_eq!(key("Binance hack 2026-10"), "binance-hack-2026-10");
        assert_eq!(key(" binance-hack-2026-10 "), "binance-hack-2026-10");
    }

    #[test]
    fn only_a_hot_event_with_something_new_is_written_and_none_moves_up_into_a_place_left() {
        let picked = |finds: Vec<(Scout, Find)>, passed: &[&str]| {
            let passed = passed.iter().map(|event| event.to_string()).collect();
            pick(finds, &passed, TOP).map(|(event, finds)| (event, finds.len()))
        };
        // grok's six hottest; the briefing model's first is grok's sixth.
        let finds = |news: bool| {
            let mut finds: Vec<(Scout, Find)> = (1..=6)
                .map(|place| find(Scout::X, &format!("event-{place}"), "新进展"))
                .collect();
            if !news {
                finds[0].1.points.clear();
            }
            finds.push(find(Scout::Web, "event-6", "新进展"));
            finds
        };
        // Named by both scouts, the sixth wins: first on one list is high enough.
        assert_eq!(picked(finds(true), &[]), Some(("event-6".into(), 2)));
        // Without it, grok's first; once that tells nothing new, its second.
        assert_eq!(
            picked(finds(true), &["event-6"]),
            Some(("event-1".into(), 1))
        );
        assert_eq!(
            picked(finds(false), &["event-6"]),
            Some(("event-2".into(), 1))
        );
        // Grok's first five wait or tell nothing new: its sixth doesn't move up, and the
        // briefing model named nothing else.
        let finds_without_web = || {
            let mut finds = finds(true);
            finds.pop();
            finds
        };
        assert_eq!(
            picked(
                finds_without_web(),
                &["event-1", "event-2", "event-3", "event-4", "event-5"]
            ),
            None
        );
        // First on grok's list with nothing new, sixth on the briefing model's with news: hot
        // enough by the one, new by the other.
        let mut split = vec![find(Scout::X, "event-1", "")];
        split.extend((2..=6).map(|place| find(Scout::Web, &format!("other-{place}"), "新进展")));
        split.push(find(Scout::Web, "event-1", "新进展"));
        assert_eq!(picked(split, &[]), Some(("event-1".into(), 2)));
    }

    #[test]
    fn the_scouts_hear_the_events_followed_and_those_taken_back() {
        let at = |time: DateTime<Utc>| time.format("%m-%d %H:%M");
        let time = |hour: u32| {
            DateTime::parse_from_rfc3339(&format!("2026-10-31T{hour:02}:00:00Z"))
                .unwrap()
                .to_utc()
        };
        let tracked = [
            Tracked {
                event: "binance-hack-2026-10".into(),
                title: "币安被盗".into(),
                published: true,
                updated_at: time(6),
            },
            Tracked {
                event: "rumor-2026-10".into(),
                title: "一则传闻".into(),
                published: false,
                updated_at: time(5),
            },
        ];
        assert_eq!(
            scouting(&tracked, at),
            "正在追踪的事件（标识｜标题｜上次更新）：\n- binance-hack-2026-10｜币安被盗｜10-31 06:00\n不再追踪的事件（标识｜标题）：\n- rumor-2026-10｜一则传闻\n"
        );
        assert_eq!(
            scouting(&tracked[1..], at),
            "不再追踪的事件（标识｜标题）：\n- rumor-2026-10｜一则传闻\n"
        );
        assert_eq!(scouting(&[], at), "");
    }

    #[test]
    fn an_event_with_nothing_new_waits_and_then_is_looked_at_again() {
        let now = Utc::now();
        CHECKED
            .lock()
            .unwrap()
            .extend([("old".to_owned(), now - chrono::Duration::hours(7))]);
        CHECKED.lock().unwrap().insert("fresh".into(), now);
        let waiting = checked(now - chrono::Duration::hours(WAIT_HOURS));
        assert!(waiting.contains(&"fresh".to_owned()) && !waiting.contains(&"old".to_owned()));
        assert!(!CHECKED.lock().unwrap().contains_key("old"));
    }
}
