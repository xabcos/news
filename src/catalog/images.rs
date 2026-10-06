//! Resolve missing pictures from known pages; use the existing image transport and budget.
use super::{
    Topic,
    source::{public_url, same_page},
};
use crate::{
    error::Result,
    fetch::{NewItem, NewsImage},
    media, reads,
    state::AppState,
    x,
};
use scraper::{Html, Selector};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use tokio::{
    task::JoinSet,
    time::{Instant, timeout_at},
};
use url::Url;

pub(crate) async fn prepare(
    state: &AppState,
    topic: &Topic,
    items: &mut [NewItem],
    payload: &Value,
    deadline: Instant,
) {
    let candidates: HashMap<_, _> = payload["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| Some((public_url(item["url"].as_str()?)?, item)))
        .collect();
    let book = topic.id == "book";
    let pending: Vec<_> = items
        .iter_mut()
        .enumerate()
        .filter(|(_, item)| item.image.is_none())
        .map(|(index, item)| {
            let candidate = public_url(&item.url)
                .and_then(|url| candidates.get(&url))
                .copied();
            let mut pages: Vec<String> = Vec::new();
            let hint = candidate.and_then(|c| c["image_page_url"].as_str());
            let work = candidate.and_then(|c| c["work"]["source_urls"][0].as_str());
            let ordered = if book {
                [hint, work, Some(item.url.as_str())]
            } else {
                [hint, Some(item.url.as_str()), work]
            };
            for page in ordered.into_iter().flatten().filter_map(public_url) {
                if !pages.iter().any(|known| same_page(known, &page)) {
                    pages.push(page);
                }
            }
            pages.truncate(2);
            let template = NewsImage {
                url: String::new(),
                source_url: item.url.clone(),
                caption: item.title.clone(),
                caption_en: if item.title_en.is_empty() {
                    &item.title
                } else {
                    &item.title_en
                }
                .clone(),
                credit: item.source.clone(),
                origin_url: None,
                expires_at: None,
                content_hash: None,
                width: None,
                height: None,
            };
            let state = state.clone();
            async move {
                let limit = deadline.min(Instant::now() + Duration::from_secs(60));
                let image = timeout_at(limit, resolve(&state, template, pages, book))
                    .await
                    .ok()
                    .flatten();
                (index, image)
            }
        })
        .collect();
    let mut pending = pending.into_iter();
    let mut jobs = JoinSet::new();
    loop {
        while jobs.len() < 3 && Instant::now() < deadline {
            let Some(job) = pending.next() else { break };
            jobs.spawn(job);
        }
        match jobs.join_next().await {
            Some(Ok((index, image))) => items[index].image = image,
            Some(Err(error)) => tracing::debug!(%error, "catalogue image deferred"),
            None => break,
        }
    }
}

async fn mirrored(state: &AppState, image: NewsImage) -> Option<NewsImage> {
    let mut images = [Some(image)];
    media::mirror_images(state, &mut images).await;
    images[0].take()
}

async fn resolve(
    state: &AppState,
    template: NewsImage,
    pages: Vec<String>,
    book: bool,
) -> Option<NewsImage> {
    let mut tried = Vec::new();
    for page in pages {
        let images = match from_page(state, &page, &template, book).await {
            Ok(images) => images,
            Err(error) => {
                tracing::debug!(%error, %page, "catalogue image source unavailable");
                continue;
            }
        };
        for image in images {
            if tried.contains(&image.url) {
                continue;
            }
            tried.push(image.url.clone());
            if let Some(image) = mirrored(state, image).await {
                return Some(image);
            }
        }
    }
    None
}

async fn from_page(
    state: &AppState,
    page: &str,
    template: &NewsImage,
    book: bool,
) -> Result<Vec<NewsImage>> {
    let url = Url::parse(page).expect("validated image page");
    if let Some(id) = reads::x_status(&url) {
        let post = x::read_post(state, &id).await?;
        if post.sensitive() {
            return Ok(Vec::new());
        }
        return Ok(post
            .photos()
            .filter(|(_, size)| {
                size.width >= media::MIN_IMAGE_DIMENSION
                    && size.height >= media::MIN_IMAGE_DIMENSION
            })
            .take(2)
            .map(|(key, size)| NewsImage {
                url: x::photo_url(&key),
                source_url: page.into(),
                credit: post.user.name.clone(),
                width: Some(size.width),
                height: Some(size.height),
                ..template.clone()
            })
            .collect());
    }
    let (url, bytes) = reads::download(state, url, reads::Content::Html).await?;
    let html = String::from_utf8_lossy(&bytes);
    Ok(page_images(&url, &html, book)
        .into_iter()
        .map(|image| NewsImage {
            url: image,
            source_url: url.to_string(),
            credit: url.host_str().unwrap_or_default().into(),
            ..template.clone()
        })
        .collect())
}

fn page_images(base: &Url, text: &str, book: bool) -> Vec<String> {
    let html = Html::parse_document(text);
    let select = |query| Selector::parse(query).expect("static selector");
    let mut images: Vec<String> = Vec::new();
    let mut add = |value: &str| {
        if let Ok(url) = base.join(value.trim())
            && media::is_public_origin(&url)
            && !images.iter().any(|known| known == url.as_str())
        {
            images.push(url.to_string());
        }
    };
    // Publishers sometimes expose a downloadable cover while their social thumbnail is blocked.
    if book {
        for anchor in html.select(&select("a[href]")) {
            let label = anchor.text().collect::<String>().to_lowercase();
            let href = anchor.attr("href").unwrap_or_default();
            let path = href.split('?').next().unwrap_or(href).to_lowercase();
            if [
                "cover",
                "couverture",
                "封面",
                "表紙",
                "copertina",
                "portada",
            ]
            .iter()
            .any(|word| label.contains(word))
                && [".jpg", ".jpeg", ".png", ".webp"]
                    .iter()
                    .any(|ext| path.ends_with(ext))
            {
                add(href);
            }
        }
    }
    for image in super::source::meta_images(&html, base) {
        add(&image);
    }
    images.truncate(2);
    images
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_real_page_images_and_prefers_the_publishers_book_cover() {
        let base = Url::parse("https://publisher.example/books/shore").unwrap();
        let html = r#"<meta property="og:image" content="/social.jpg"><meta name="twitter:image" content="/social.jpg">
            <meta property="og:image" content="http://127.0.0.1/private.jpg">
            <a href="https://media.example/cover.jpg">Télécharger la couverture</a><img src="/unrelated-logo.jpg">"#;
        assert_eq!(
            page_images(&base, html, false),
            ["https://publisher.example/social.jpg"]
        );
        assert_eq!(
            page_images(&base, html, true),
            [
                "https://media.example/cover.jpg",
                "https://publisher.example/social.jpg"
            ]
        );
    }
}
