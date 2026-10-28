//! 时代: XABC profiles the people this age talks about, one article a person. Each time both
//! scouts name the people talked about most now, and one of them is written about (`pick`):
//! anew, or once their profile is `REVISE_AFTER_DAYS` days old brought up to date. Searching,
//! writing and pictures are 分享's (`super`), but a profile shows only pictures from pages, the
//! first standing for it on the lists.
use super::{
    Drafted, Find, Finished, KEEPING, Line, Scout, Subject, draft, figures, finish, linked, listed,
    look, publish, revise,
};
use crate::{
    error::{AppError, Result},
    fetch::Usage,
    state::AppState,
};
use chrono::{DateTime, Utc};
use std::{cmp::Reverse, collections::HashSet, fmt::Write};
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
    let (person, finds) = pick(std::mem::take(&mut found.finds), &passed)
        .ok_or_else(|| AppError::Internal("两路检索都没有找到可写的人物".into()))?;
    let material = format!("今天是 {today}。\n人物：{}\n", finds[0].1.person.trim());
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
        let article = finish(state, drafted.article, found.known, &[], false).await?;
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
    let article = finish(state, drafted.article, found.known, &kept, false).await?;
    // The cover follows the picture the profile opens with now.
    let cover = crate::articles::pictures(&article.body)
        .first()
        .copied()
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

/// The person to write about and every find about them: of the people the finds name, leaving
/// out `passed`, the one most scouts named, then the one placed highest on either scout's list,
/// then the one met first. Each scout lists the people talked about most first.
fn pick(
    finds: Vec<(Scout, Find)>,
    passed: &HashSet<String>,
) -> Option<(String, Vec<(Scout, Find)>)> {
    // Each person once, in the order they were first named, with the scouts that named them and
    // the best place they were given.
    let mut named: Vec<(String, [bool; 2], usize)> = Vec::new();
    let mut places = [0; 2];
    for (scout, find) in &finds {
        let by = *scout as usize;
        let place = places[by];
        places[by] += 1;
        let person = key(&find.person);
        if person.is_empty() || passed.contains(&person) {
            continue;
        }
        match named.iter_mut().find(|(known, ..)| *known == person) {
            Some((_, scouts, best)) => {
                scouts[by] = true;
                *best = (*best).min(place);
            }
            None => {
                let mut scouts = [false; 2];
                scouts[by] = true;
                named.push((person, scouts, place));
            }
        }
    }
    let (chosen, ..) = named.into_iter().min_by_key(|(_, scouts, best)| {
        (
            Reverse(scouts.iter().filter(|named| **named).count()),
            *best,
        )
    })?;
    let finds = finds
        .into_iter()
        .filter(|(_, find)| key(&find.person) == chosen)
        .collect();
    Some((chosen, finds))
}

/// How a person is told apart: their English name in lower case, its words joined by hyphens,
/// so `Sam Altman` and `sam altman.` are one person.
fn key(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

/// A draft for a profile, which shows only pictures from pages: none is drawn.
fn photographed(mut drafted: Drafted) -> Drafted {
    for picture in &mut drafted.article.pictures {
        picture.prompt.clear();
    }
    drafted
}

fn log(id: Uuid, done: &str, person: &str, article: &Finished, usage: Usage) {
    tracing::info!(
        %id, person, title = article.title, pictures = article.pictures.len(),
        input_tokens = usage.input_tokens, output_tokens = usage.output_tokens,
        web_searches = usage.server_side_tool_usage_details.web_search_calls,
        x_searches = usage.server_side_tool_usage_details.x_search_calls,
        cost_ticks = usage.cost_in_usd_ticks,
        "{done}"
    );
}

pub(super) const SEARCHING: &str = "你是 XABC「时代」栏目的选题编辑。XABC 在 xabcnews.com 为这个时代最受关注的人物写全方位的人物长文，他们再次成为焦点时就更新，读者是关心科技、科学和世界的中文读者。
今天请你{scope}，找出最近一两天全网讨论最多、最受关注的人物，至少 8 位，按受关注的程度从高到低排列。以科技与 AI 的创业者、研究者和工程师，以及物理、数学等基础科学的科学家为主；政治人物和网络热点人物，只选真正处在舆论中心的，政治人物只选中国以外的。只选公众人物，不选未成年人，也不选偶然走红的普通人。
每条线索是一个人：person 写他最常用的英文名（中国人用拼音），只写名字，不加头衔、括号和说明；title 一句话说明他是谁、为什么现在受关注；why 写热度依据，比如讨论、转发、报道和事件；points 写他这次受关注的事，以及与他相关的公司和机构的近况；pictures 优先给有他本人照片的页面。
";

const WRITING: &str = "你是 XABC，在 xabcnews.com 的「时代」栏目写人物，读者是关心科技、科学和世界的中文读者。
下面是这次要写的人物和两位编辑找来的线索。多用 web_search 把他的公开资料查全：维基百科等资料、官方主页、权威媒体的报道和长篇访谈、他本人的文章与演讲；然后写一篇完整的中文人物长文。
怎么写：
- 把这个人写透：先写他是谁、为什么现在大家都在谈论他；再写他的经历、做成的事和代表作、主要观点与做事方式、外界的评价与争议；与他相关的公司和机构，写清来历、业务、产品和最新动向；最后在「近况」里按时间写清最近发生的事，带上日期。
- 像一个做足功课、会讲故事的记者：具体、生动，用事实、细节、数字和他本人的话说话；可以有判断，但不吹捧、不贬低。篇幅不限，内容需要多长就写多长，用小标题组织。
- 只写公开、查得到出处的信息：经历、数字和引语都要来自资料和检索；传闻写明是传闻，有争议的事写清各方的说法；不写住址、联系方式和家人的私事。
- 人名第一次出现写通行的中文译名并附原名，标题带上他的中文名。
- 配图都用页面上的真实照片，不画插图：第一张放在正文开头，是能看清他本人的照片；其余放他工作、演讲和重要时刻的照片，以及他的产品和公司。pictures 的 page 填带图的页面（维基百科条目、官方页面、新闻报道或他本人的 X 帖子），prompt 留空。
";

const REVISING: &str = "- 这次是更新：输入里有这篇人物介绍的现有全文和他最近受关注的线索。在现有全文的基础上把它改得更好：把新的动态按时间写进「近况」，补上以前缺的资料，修正不准确和已过时的说法；「近况」只留最近一两年的要事，更早的写进经历。交回更新后的完整全文。";

#[cfg(test)]
mod tests {
    use super::*;

    fn find(scout: Scout, person: &str) -> (Scout, Find) {
        (
            scout,
            Find {
                title: format!("{person} 的事"),
                why: String::new(),
                points: String::new(),
                sources: Vec::new(),
                pictures: Vec::new(),
                person: person.into(),
            },
        )
    }

    #[test]
    fn a_person_is_told_apart_by_the_words_of_their_english_name() {
        assert_eq!(key(" Sam  Altman. "), "sam-altman");
        assert_eq!(key("Yann LeCun"), "yann-lecun");
        assert_eq!(key("Kurt Gödel"), "kurt-gödel");
        assert_eq!(key("（）"), "");
    }

    #[test]
    fn the_person_most_scouts_name_goes_first_then_the_one_named_first() {
        let named = |finds: Vec<(Scout, Find)>, passed: &[&str]| {
            let passed = passed.iter().map(|person| person.to_string()).collect();
            pick(finds, &passed).map(|(person, finds)| (person, finds.len()))
        };
        // Both scouts named Hassabis, the web's first; grok's first and second only grok did.
        let finds = || {
            vec![
                find(Scout::X, "Jensen Huang"),
                find(Scout::X, "Lisa Su"),
                find(Scout::X, "demis hassabis"),
                find(Scout::Web, "Demis Hassabis"),
                find(Scout::Web, "Terence Tao"),
            ]
        };
        assert_eq!(named(finds(), &[]), Some(("demis-hassabis".into(), 2)));
        // Without him, the best place: grok's first, ahead of both second places.
        assert_eq!(
            named(finds(), &["demis-hassabis"]),
            Some(("jensen-huang".into(), 1))
        );
        // Two second places: the one named first, grok's.
        assert_eq!(
            named(finds(), &["demis-hassabis", "jensen-huang"]),
            Some(("lisa-su".into(), 1))
        );
        // The web's first place beats grok's second.
        assert_eq!(
            named(
                vec![
                    find(Scout::X, "Jensen Huang"),
                    find(Scout::X, "Lisa Su"),
                    find(Scout::Web, "Terence Tao"),
                ],
                &["jensen-huang"]
            ),
            Some(("terence-tao".into(), 1))
        );
        assert_eq!(named(vec![find(Scout::X, " ")], &[]), None);
    }
}
