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
    /// Why a hotspot matters; gossip has none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub angle: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub angle_en: String,
    pub status: StoryStatus,
    pub source_indexes: Vec<usize>,
}

/// A story's schema properties; hotspots narrow the status and add the angle.
pub(super) fn story_properties(source_indexes: Value) -> Value {
    let text = json!({ "type": "string" });
    json!({
        "title": text, "title_en": text, "summary": text, "summary_en": text,
        "short_summary": text,
        "status": { "type": "string", "enum": ["confirmed", "reported", "unconfirmed", "response", "feedback", "denied"] },
        "source_indexes": source_indexes
    })
}

/// Citation order follows reading order and is shared by both article formats.
pub(super) fn citation_indexes(analysis: &InsightAnalysis) -> Vec<usize> {
    let mut indexes = analysis.changes_source_indexes.clone();
    for story in &analysis.hotspots {
        indexes.extend(&story.source_indexes);
    }
    for figure in &analysis.figures {
        indexes.extend(&figure.source_indexes);
    }
    for story in &analysis.gossip {
        indexes.extend(&story.source_indexes);
    }
    indexes.extend(analysis.cover_source_index);
    indexes.extend(&analysis.picture_source_indexes);
    for signal in analysis.outlook.iter().chain(&analysis.watch_signals) {
        indexes.extend(&signal.source_indexes);
    }
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
    article.stories("今日热点", &analysis.hotspots, false);
    article.section(
        "今日数字",
        analysis
            .figures
            .iter()
            .map(|figure| {
                (
                    figure.value.clone(),
                    figure.label.clone(),
                    figure.label.clone(),
                )
            })
            .collect(),
    );
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
    for (label, signals) in [
        ("机会与风险", &analysis.outlook),
        ("后续观察", &analysis.watch_signals),
    ] {
        article.section(
            label,
            signals
                .iter()
                .map(|signal| {
                    let title = match outlook_label(&signal.kind) {
                        Some(kind) => format!("【{kind}】{}", signal.title),
                        None => signal.title.clone(),
                    };
                    (title, signal.summary.clone(), signal.summary.clone())
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
        self.section(
            label,
            stories
                .iter()
                .map(|story| {
                    let title = if gossip {
                        format!("【{}】{}", story.status.label(), story.title)
                    } else {
                        story.title.clone()
                    };
                    let short = if story.short_summary.trim().is_empty() {
                        &story.summary
                    } else {
                        &story.short_summary
                    };
                    let angle = |text: &str| match story.angle.trim() {
                        "" => text.to_owned(),
                        angle => format!("{text} 看点：{angle}"),
                    };
                    (title, angle(&story.summary), angle(short))
                })
                .collect(),
        );
    }

    /// A numbered list under its heading; an empty one is left out.
    fn section(&mut self, label: &str, items: Vec<(String, String, String)>) {
        if items.is_empty() {
            return;
        }
        self.heading(2, label);
        self.items(items);
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

/// The Chinese tag of an outlook item's kind.
fn outlook_label(kind: &str) -> Option<&'static str> {
    match kind {
        "opportunity" => Some("机会"),
        "employment" => Some("就业"),
        "risk" => Some("风险"),
        "action" => Some("行动"),
        _ => None,
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
