//! Public source pages, read through the same bounded, public-origin client as Reads.
use crate::{
    error::{AppError, Result},
    reads,
    state::AppState,
};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Source {
    pub title: String,
    pub url: String,
    pub text: String,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Link {
    pub label: String,
    pub url: String,
}

pub(super) fn public_url(value: &str) -> Option<String> {
    let url = Url::parse(value.trim()).ok()?;
    crate::media::is_public_origin(&url).then(|| crate::fetch::canonical_url(&url))
}

pub(super) fn fold(text: &str) -> String {
    text.nfkc()
        .flat_map(char::to_lowercase)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

pub(super) async fn read(state: &AppState, value: &str) -> Result<Source> {
    let url = Url::parse(value).map_err(|_| AppError::Internal("作品资料 URL 无效".into()))?;
    if let Some(id) = reads::x_status(&url) {
        let post = crate::x::read_post(state, &id).await?;
        return Ok(Source {
            title: post.user.name.clone(),
            url: value.into(),
            text: post.words(),
            links: Vec::new(),
        });
    }
    let (url, bytes) = reads::download(state, url, reads::Content::Html).await?;
    let html = std::str::from_utf8(&bytes)
        .map_err(|_| AppError::Internal("作品资料不是 UTF-8 网页".into()))?;
    let source = extract(&url, html);
    if source.text.chars().count() < 200 {
        return Err(AppError::Internal("作品资料正文不足".into()));
    }
    Ok(source)
}

pub(super) fn extract(url: &Url, text: &str) -> Source {
    let html = Html::parse_document(text);
    let select = |query| Selector::parse(query).expect("static selector");
    let title = html
        .select(&select("title"))
        .next()
        .map(|n| n.text().collect::<String>())
        .unwrap_or_default();
    let body = ["article", "main", "body"]
        .iter()
        .find_map(|query| html.select(&select(query)).next())
        .unwrap_or_else(|| html.root_element());
    let mut words = Vec::new();
    for node in body.descendants() {
        if let Some(text) = node.value().as_text() {
            let skipped = node
                .ancestors()
                .filter_map(ElementRef::wrap)
                .any(|ancestor| {
                    matches!(
                        ancestor.value().name(),
                        "script"
                            | "style"
                            | "noscript"
                            | "nav"
                            | "footer"
                            | "aside"
                            | "svg"
                            | "template"
                    )
                });
            if !skipped && !text.trim().is_empty() {
                words.push(text.trim());
            }
        }
    }
    let description = html
        .select(&select(
            "meta[name=description], meta[property='og:description']",
        ))
        .filter_map(|n| n.attr("content"))
        .take(1)
        .collect::<String>();
    let text = format!("{title}\n{description}\n{}", words.join(" "));
    let mut links = Vec::new();
    for anchor in body.select(&select("a[href]")) {
        let Some(target) = anchor
            .attr("href")
            .and_then(|href| url.join(href).ok())
            .and_then(|url| public_url(url.as_str()))
        else {
            continue;
        };
        let label = anchor
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_owned();
        if !label.is_empty() && !links.iter().any(|link: &Link| link.url == target) {
            links.push(Link {
                label: label.chars().take(100).collect(),
                url: target,
            });
        }
        if links.len() == 60 {
            break;
        }
    }
    Source {
        title,
        url: url.to_string(),
        text: text.chars().take(18000).collect(),
        links,
    }
}
