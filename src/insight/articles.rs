use super::{
    InsightAnalysis, InsightArticleImage, InsightArticles, InsightResearchSource, InsightSource,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoryStatus {
    Confirmed,
    Reported,
    Unconfirmed,
    Response,
    Feedback,
    Denied,
}

impl StoryStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Confirmed => "已确认",
            Self::Reported => "爆料",
            Self::Unconfirmed => "未证实",
            Self::Response => "当事方回应",
            Self::Feedback => "用户反馈",
            Self::Denied => "已否认",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Story {
    pub title: String,
    pub title_en: String,
    pub summary: String,
    pub summary_en: String,
    pub short_summary: String,
    pub status: StoryStatus,
    pub source_indexes: Vec<usize>,
}

pub(super) fn story_schema(source_indexes: Value) -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "required": ["title", "title_en", "summary", "summary_en", "short_summary", "status", "source_indexes"],
        "properties": {
            "title": { "type": "string" }, "title_en": { "type": "string" },
            "summary": { "type": "string" }, "summary_en": { "type": "string" },
            "short_summary": { "type": "string" },
            "status": { "type": "string", "enum": ["confirmed", "reported", "unconfirmed", "response", "feedback", "denied"] },
            "source_indexes": source_indexes
        }
    })
}

/// Citation order follows reading order and is shared by both article formats.
pub(super) fn citation_indexes(analysis: &InsightAnalysis) -> Vec<usize> {
    let mut indexes = analysis
        .hotspots
        .iter()
        .chain(&analysis.gossip)
        .flat_map(|story| story.source_indexes.iter().copied())
        .collect::<Vec<_>>();
    indexes.extend(analysis.cover_source_index);
    indexes.extend(&analysis.picture_source_indexes);
    indexes.extend(&analysis.changes_source_indexes);
    indexes.extend(
        analysis
            .data_signals
            .iter()
            .chain(&analysis.watch_signals)
            .chain(&analysis.wealth)
            .chain(&analysis.employment)
            .chain(&analysis.risks)
            .chain(&analysis.actions)
            .flat_map(|signal| signal.source_indexes.iter().copied()),
    );
    let mut seen = HashSet::new();
    indexes.retain(|index| seen.insert(*index));
    indexes
}

pub(super) fn render(
    edition_label: &str,
    analysis: &InsightAnalysis,
    sources: &[InsightSource],
    research: &[InsightResearchSource],
    images: Vec<InsightArticleImage>,
) -> InsightArticles {
    let title = analysis.headline.clone();
    let mut article = Article::default();
    article.heading(1, &title);
    article.paragraph(edition_label);
    article.stories("今日热点", &analysis.hotspots, false);
    article.stories("AI 圈热议", &analysis.gossip, true);
    if !images.is_empty() {
        article.heading(2, "图片新闻");
        for image in &images {
            let label = if image.archived { "资料图 · " } else { "" };
            let caption = format!("{label}{} · {}", image.caption, image.credit);
            article.wechat.push(format!(
                "**{}**\n\n![{}](<{}>)\n\n{}",
                markdown(&image.title),
                markdown(&image.caption),
                image.url,
                markdown(&caption)
            ));
            article
                .plain
                .push(format!("{}：{caption}", compact(&image.title)));
            // X images are uploaded separately; clipboard HTML only contains text.
            article.html.push(format!(
                "<p><strong>{}</strong> {}</p>",
                html(&image.title),
                html(&caption)
            ));
        }
    }
    if !analysis.changes.trim().is_empty() {
        article.heading(2, "趋势分析");
        for point in analysis
            .changes
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            article.paragraph(point);
        }
    }
    for (label, signals) in [
        ("关键数据", &analysis.data_signals),
        ("后续观察", &analysis.watch_signals),
        ("商业与机遇", &analysis.wealth),
        ("岗位与技能", &analysis.employment),
        ("风险与边界", &analysis.risks),
        ("可做的事", &analysis.actions),
    ] {
        if signals.is_empty() {
            continue;
        }
        article.heading(2, label);
        article.items(
            signals
                .iter()
                .map(|item| {
                    let mut summary = compact(&item.summary);
                    // Preserve an opportunity's limiting condition in either copy.
                    if !item.risk.is_empty() && !summary.contains(&item.risk) {
                        summary.push_str(&format!(" 边界：{}", compact(&item.risk)));
                    }
                    (item.title.clone(), summary.clone(), summary)
                })
                .collect(),
        );
    }
    let mut seen = HashSet::new();
    let mut references = citation_indexes(analysis)
        .into_iter()
        .chain(images.iter().map(|image| image.index))
        .filter_map(|index| sources.iter().find(|source| source.index == index))
        .map(|source| (&source.title, &source.url))
        .chain(research.iter().map(|source| (&source.title, &source.url)))
        .filter(|(_, url)| seen.insert(url.as_str()))
        .filter(|(_, url)| url.starts_with("https://") || url.starts_with("http://"))
        .collect::<Vec<_>>();
    if !references.is_empty() {
        article.heading(2, "参考来源");
        let mut md = Vec::new();
        let mut plain = Vec::new();
        let mut links = Vec::new();
        for (position, (label, url)) in references.drain(..).enumerate() {
            md.push(format!(
                "{}. [{}](<{}>)",
                position + 1,
                markdown(label),
                url
            ));
            plain.push(format!("{}. {}：{url}", position + 1, compact(label)));
            links.push(format!(
                "<li><a href=\"{}\">{}</a></li>",
                html(url),
                html(label)
            ));
        }
        article.wechat.push(md.join("\n"));
        article.plain.push(plain.join("\n"));
        article.html.push(format!("<ol>{}</ol>", links.join("")));
    }
    InsightArticles {
        title,
        wechat: article.wechat.join("\n\n"),
        x: article.plain.join("\n\n"),
        x_html: article.html.join(""),
        images,
    }
}

#[derive(Default)]
struct Article {
    wechat: Vec<String>,
    plain: Vec<String>,
    html: Vec<String>,
}

impl Article {
    fn heading(&mut self, level: usize, label: &str) {
        self.wechat
            .push(format!("{} {}", "#".repeat(level), markdown(label)));
        self.plain.push(compact(label));
        self.html
            .push(format!("<h{level}>{}</h{level}>", html(label)));
    }

    fn paragraph(&mut self, text: &str) {
        self.wechat.push(markdown(text));
        self.plain.push(compact(text));
        self.html.push(format!("<p>{}</p>", html(text)));
    }

    fn stories(&mut self, label: &str, stories: &[Story], gossip: bool) {
        if stories.is_empty() {
            return;
        }
        self.heading(2, label);
        self.items(
            stories
                .iter()
                .map(|story| {
                    let title = if gossip {
                        format!("【{}】{}", story.status.label(), story.title)
                    } else {
                        story.title.clone()
                    };
                    (title, story.summary.clone(), story.short_summary.clone())
                })
                .collect(),
        );
    }

    fn items(&mut self, items: Vec<(String, String, String)>) {
        let mut md = Vec::new();
        let mut plain = Vec::new();
        let mut html_items = Vec::new();
        for (index, (title, summary, short)) in items.iter().enumerate() {
            let short = if short.trim().is_empty() {
                summary
            } else {
                short
            };
            md.push(format!(
                "{}. **{}** — {}",
                index + 1,
                markdown(title),
                markdown(summary)
            ));
            plain.push(format!(
                "{}. {} — {}",
                index + 1,
                compact(title),
                compact(short)
            ));
            html_items.push(format!(
                "<li><strong>{}</strong> — {}</li>",
                html(title),
                html(short)
            ));
        }
        self.wechat.push(md.join("\n"));
        self.plain.push(plain.join("\n"));
        self.html.push(format!("<ol>{}</ol>", html_items.join("")));
    }
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn markdown(text: &str) -> String {
    let mut escaped = String::new();
    for character in compact(text).chars() {
        if "\\[]*_<>".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn html(text: &str) -> String {
    crate::site::escape(&compact(text))
}
