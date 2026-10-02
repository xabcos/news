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
    /// Pictures the page shares or the post carries; neither shown to the model nor hashed.
    #[serde(skip)]
    pub images: Vec<String>,
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

/// Text as an excerpt is matched: folded, but keeping the punctuation between two digits, or
/// a space where there is none, so 6.8, 6–8 and 68 stay apart.
pub(super) fn excerpt(text: &str) -> String {
    let mut folded = String::new();
    let mut gap: Option<String> = None;
    for c in text.nfkc().flat_map(char::to_lowercase) {
        if !c.is_alphanumeric() {
            let gap = gap.get_or_insert_default();
            if !c.is_whitespace() {
                gap.push(c);
            }
            continue;
        }
        if let Some(gap) = gap.take()
            && c.is_ascii_digit()
            && folded.ends_with(|last: char| last.is_ascii_digit())
        {
            folded.push_str(if gap.is_empty() { " " } else { &gap });
        }
        folded.push(c);
    }
    folded
}

pub(super) async fn read(state: &AppState, value: &str, limit: usize) -> Result<Source> {
    let url = Url::parse(value).map_err(|_| AppError::Internal("资料 URL 无效".into()))?;
    if let Some(id) = reads::x_status(&url) {
        let post = crate::x::read_post(state, &id).await?;
        // Known by its real author, whom an official account check then sees.
        return Ok(Source {
            title: post.user.name.clone(),
            url: format!(
                "https://x.com/{}/status/{id}",
                post.account().unwrap_or("i")
            ),
            text: post.words(),
            links: Vec::new(),
            images: post
                .photos()
                .filter(|(_, size)| size.width.max(size.height) >= crate::x::MIN_LONGER_SIDE)
                .map(|(key, _)| crate::x::photo_url(&key))
                .collect(),
        });
    }
    let (url, bytes) = reads::download(state, url, reads::Content::Html).await?;
    let html = std::str::from_utf8(&bytes)
        .map_err(|_| AppError::Internal("作品资料不是 UTF-8 网页".into()))?;
    let source = extract(&url, html, limit);
    if source.text.chars().count() < 200 {
        return Err(AppError::Internal("资料正文不足".into()));
    }
    Ok(source)
}

/// The pictures a page shares in its metadata, absolute and public.
pub(super) fn meta_images(html: &Html, base: &Url) -> Vec<String> {
    let select = Selector::parse("meta[property], meta[name]").expect("static selector");
    let mut images: Vec<String> = Vec::new();
    for meta in html.select(&select) {
        let key = meta
            .attr("property")
            .or_else(|| meta.attr("name"))
            .unwrap_or_default();
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "og:image"
                | "og:image:url"
                | "og:image:secure_url"
                | "twitter:image"
                | "twitter:image:src"
        ) && let Some(url) = meta
            .attr("content")
            .and_then(|value| base.join(value.trim()).ok())
            && crate::media::is_public_origin(&url)
            && !images.iter().any(|known| known == url.as_str())
        {
            images.push(url.to_string());
        }
    }
    images
}

pub(super) fn extract(url: &Url, text: &str, limit: usize) -> Source {
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
    // The pictures it shares, then the large ones it shows.
    let mut images = meta_images(&html, url);
    for image in body.select(&select("img[src][width]")) {
        let wide = image
            .attr("width")
            .and_then(|width| width.trim_end_matches("px").parse::<u32>().ok())
            .is_some_and(|width| width >= 600);
        if let Some(src) = image.attr("src").filter(|_| wide)
            && let Ok(src) = url.join(src.trim())
            && crate::media::is_public_origin(&src)
            && !images.iter().any(|known| known == src.as_str())
        {
            images.push(src.to_string());
        }
    }
    // A site's logo, often its default social picture, shows no product.
    images.retain(|image| {
        !Url::parse(image).is_ok_and(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .is_some_and(|name| {
                    name.split(|c: char| !c.is_ascii_alphanumeric())
                        .any(|word| word.eq_ignore_ascii_case("logo"))
                })
        })
    });
    Source {
        title,
        url: url.to_string(),
        text: text.chars().take(limit).collect(),
        links,
        images,
    }
}
