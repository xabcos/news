use super::invalid_source;
use crate::{error::Result, media::is_public_origin, state::AppState};
use reqwest::header::USER_AGENT;
use scraper::{ElementRef, Html, Node, Selector};
use serde::Deserialize;
use std::time::Duration;
use url::Url;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct Source {
    pub url: Url,
    pub title: String,
    pub author: String,
    pub language: String,
    pub body: String,
    pub images: Vec<super::full_text::Image>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(super) async fn fetch(state: &AppState, value: &str) -> Result<Source> {
    let url = Url::parse(value).map_err(|_| invalid_source("原文 URL 无效"))?;
    if let Some(id) = x_status(&url) {
        return fetch_post(state, &id).await;
    }
    let (url, bytes) = download(state, url, Content::Html).await?;
    let html = std::str::from_utf8(&bytes).map_err(|_| invalid_source("原文不是 UTF-8 文本"))?;
    extract(url, html)
}

#[derive(Clone, Copy)]
pub(crate) enum Content {
    Html,
    Json,
    Feed,
    Image(usize),
}

/// `origin_http` resolves every hop to public addresses and keeps redirects on public
/// HTTPS; this bounds time, content type and size, and sends no credentials or cookies.
pub(crate) async fn download(
    state: &AppState,
    url: Url,
    content: Content,
) -> Result<(Url, Vec<u8>)> {
    if !is_public_origin(&url) {
        return Err(invalid_source("原文必须是公开 HTTPS 地址"));
    }
    let max_bytes = match content {
        Content::Image(limit) => limit,
        _ => MAX_BYTES,
    };
    let mut response = state
        .origin_http
        .get(url)
        .timeout(TIMEOUT)
        .header(
            USER_AGENT,
            "XABCNews-Reads/1.0 (+https://xabcnews.com/reads)",
        )
        .send()
        .await?
        .error_for_status()?;
    let url = response.url().clone();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let accepted = match content {
        Content::Image(_) => [
            "image/png",
            "image/jpeg",
            "image/webp",
            "image/gif",
            "image/avif",
            "image/bmp",
        ]
        .contains(&content_type.split(';').next().unwrap_or("").trim()),
        Content::Json => content_type.starts_with("application/json"),
        Content::Html => {
            content_type.starts_with("text/html")
                || content_type.starts_with("application/xhtml+xml")
        }
        Content::Feed => [
            "application/rss+xml",
            "application/atom+xml",
            "application/xml",
            "text/xml",
            "application/feed+json",
            "application/json",
        ]
        .iter()
        .any(|kind| content_type.starts_with(kind)),
    };
    if !accepted {
        return Err(invalid_source("原文不是可读取的网页"));
    }
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes as u64)
    {
        return Err(invalid_source("原文网页过大"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > max_bytes {
            return Err(invalid_source("原文网页过大"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((url, bytes))
}

pub(crate) fn x_status(url: &Url) -> Option<String> {
    if !is_public_origin(url)
        || !matches!(
            url.host_str()?,
            "x.com" | "www.x.com" | "twitter.com" | "www.twitter.com" | "mobile.twitter.com"
        )
    {
        return None;
    }
    let parts: Vec<_> = url.path_segments()?.collect();
    let index = parts.iter().position(|part| *part == "status")?;
    let id = *parts.get(index + 1)?;
    (id.len() >= 5 && id.len() <= 25 && id.bytes().all(|c| c.is_ascii_digit()))
        .then(|| id.to_owned())
}

async fn fetch_post(state: &AppState, id: &str) -> Result<Source> {
    let url = Url::parse(&format!("https://x.com/i/status/{id}")).expect("fixed X URL");
    let mut endpoint = Url::parse("https://publish.twitter.com/oembed").expect("fixed oEmbed URL");
    endpoint
        .query_pairs_mut()
        .append_pair("url", url.as_str())
        .append_pair("omit_script", "true")
        .append_pair("dnt", "true");
    let (_, bytes) = download(state, endpoint, Content::Json).await?;
    #[derive(Deserialize)]
    struct Embed {
        html: String,
        author_name: String,
    }
    let post: Embed = serde_json::from_slice(&bytes)?;
    let html = Html::parse_fragment(&post.html);
    let paragraph = html
        .select(&selector("blockquote.twitter-tweet p"))
        .next()
        .ok_or_else(|| invalid_source("X 帖子缺少可读取正文"))?;
    let (body, images) = markdown(paragraph, &url)?;
    if body.trim_end().ends_with('…')
        || body.contains("Show more")
        || body.contains("Read more")
        || !(600..=80000).contains(&body.chars().count())
    {
        return Err(invalid_source("X 帖子过短或正文不完整"));
    }
    Ok(Source {
        url,
        title: format!("{} 的公开帖子", post.author_name),
        author: post.author_name,
        language: paragraph.attr("lang").unwrap_or("und").into(),
        body,
        images,
        published_at: None,
    })
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("validated selector")
}

fn meta(html: &Html, query: &str) -> String {
    html.select(&selector(query))
        .find_map(|node| node.attr("content"))
        .unwrap_or("")
        .trim()
        .into()
}

fn extract(url: Url, text: &str) -> Result<Source> {
    let html = Html::parse_document(text);
    if html
        .select(&selector("script[type='application/ld+json']"))
        .any(|script| {
            let json = script
                .text()
                .collect::<String>()
                .replace(char::is_whitespace, "");
            json.contains("\"isAccessibleForFree\":false")
                || json.contains("\"isAccessibleForFree\":\"False\"")
        })
    {
        return Err(invalid_source("原文标注为非公开全文"));
    }
    let article_meta = meta(&html, "meta[property='og:type']") == "article";
    let body_selector = article_selector(&url);
    if url.path() == "/"
        || (body_selector.is_none()
            && !article_meta
            && html.select(&selector("article")).count() > 1)
    {
        return Err(invalid_source("原文链接不是独立文章页"));
    }
    let mut selectors = vec![
        "[itemprop='articleBody']",
        ".entry-content",
        ".post-content",
        ".article-body",
        ".post-body",
        ".body.markup",
        ".rich-editor",
        ".markdown-body",
        "#article_body",
        "article",
        "main",
    ];
    if let Some(query) = body_selector {
        selectors.insert(0, query);
    }
    let mut extracted = None;
    for query in selectors {
        if query == "main" && !article_meta {
            continue;
        }
        let Some(root) = html.select(&selector(query)).next() else {
            continue;
        };
        let (body, images) = markdown(root, &url)?;
        // A short or broken body must not fall back to a larger page full of widgets.
        if !(600..=80000).contains(&body.chars().count()) {
            return Err(invalid_source("原文正文过短或超出阅读长度限制"));
        }
        extracted = Some((root, body, images));
        break;
    }
    let (root, body, images) = extracted.ok_or_else(|| invalid_source("未找到完整的文章正文"))?;
    let lower = body.to_lowercase();
    if root
        .select(&selector(
            "a.more-link, a.read-more, .read-more, .paywall, .paywall-content",
        ))
        .next()
        .is_some()
    {
        return Err(invalid_source("原文只有节选或需要解锁"));
    }
    if [
        "subscribe to continue",
        "sign in to continue",
        "subscribe to read the rest",
        "订阅后继续阅读",
        "登录后阅读全文",
        "nur für abonnenten",
        "weiterlesen mit",
        "réservé aux abonnés",
        "abonnez-vous pour lire",
        "続きを読むにはログイン",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
    {
        return Err(invalid_source("原文需要订阅或登录"));
    }
    let mut title = meta(&html, "meta[property='og:title']");
    if title.is_empty() {
        title = html
            .select(&selector("h1, title"))
            .next()
            .map(|node| node.text().collect::<String>())
            .unwrap_or_default();
    }
    let mut author = meta(
        &html,
        "meta[name='author'], meta[property='article:author']",
    );
    if author.is_empty() {
        author = root
            .select(&selector("[rel='author'], [itemprop='author']"))
            .next()
            .map(|node| node.text().collect::<String>())
            .unwrap_or_default();
    }
    let published_at = crate::fetch::parse_time(&meta(
        &html,
        "meta[property='article:published_time'], meta[name='date'], meta[itemprop='datePublished']",
    ));
    Ok(Source {
        url,
        title: title.trim().chars().take(300).collect(),
        author: author.trim().chars().take(160).collect(),
        language: html
            .select(&selector("html"))
            .next()
            .and_then(|node| node.attr("lang"))
            .unwrap_or("und")
            .chars()
            .take(32)
            .collect(),
        body,
        images,
        published_at,
    })
}

// Publisher-specific article containers exclude recommendations and navigation.
fn article_selector(url: &Url) -> Option<&'static str> {
    match url.host_str()?.trim_start_matches("www.") {
        "blog.cloudflare.com" | "shopify.engineering" | "heise.de" => Some(".article-content"),
        "github.blog" => Some(".post__content"),
        "stackoverflow.blog" => Some(".s-prose"),
        "theregister.com" => Some("#main .bodytext"),
        "zenn.dev" => Some(".znc"),
        "qiita.com" => Some(".mdContent-inner"),
        "huggingface.co" => Some(".blog-content > div.relative"),
        "infoq.com" => Some(".article__data"),
        "quantamagazine.org" => Some("#postBody"),
        "blog.octo.com" => Some(".articleContent"),
        "t3n.de" => Some("#article"),
        _ => None,
    }
}

fn excluded(element: ElementRef<'_>, base: &Url) -> bool {
    matches!(
        element.value().name(),
        "script"
            | "style"
            | "nav"
            | "aside"
            | "footer"
            | "form"
            | "button"
            | "noscript"
            | "svg"
            | "input"
            | "select"
            | "textarea"
            | "dialog"
            | "audio"
            | "iframe"
    ) || element.attr("hidden").is_some()
        || element.attr("aria-hidden") == Some("true")
        || matches!(
            element.attr("role"),
            Some("navigation" | "complementary" | "contentinfo" | "dialog" | "toolbar")
        )
        || (matches!(element.value().name(), "div" | "section")
            && matches!(
                element.attr("id"),
                Some("comments" | "respond" | "disqus_thread" | "newsletter")
            ))
        || element.value().classes().any(|class| {
            matches!(
                class,
                "sharedaddy"
                    | "a2a_kit"
                    | "addthis_toolbox"
                    | "addthis_inline_share_toolbox"
                    | "share-buttons"
                    | "sharing-buttons"
                    | "social-share"
                    | "social-sharing"
                    | "related-posts"
                    | "related-articles"
                    | "related-content"
                    | "post-navigation"
                    | "comments-area"
                    | "comment-list"
                    | "comment-respond"
                    | "newsletter-signup"
                    | "subscription-widget-wrap"
                    | "subscribe-widget"
                    | "button-wrapper"
                    | "author-bio"
                    | "author-box"
                    | "author-section-full"
            ) || (base.host_str() == Some("huggingface.co") && class == "not-prose")
                || (base
                    .host_str()
                    .is_some_and(|host| host.trim_start_matches("www.") == "quantamagazine.org")
                    && matches!(
                        class,
                        "post__title__kicker"
                            | "post__title__title"
                            | "post__title__author-date"
                            | "post__title__actions"
                            | "post__footer"
                    ))
        })
}

fn markdown(root: ElementRef<'_>, base: &Url) -> Result<(String, Vec<super::full_text::Image>)> {
    struct Writer<'a> {
        base: &'a Url,
        images: Vec<super::full_text::Image>,
    }
    impl Writer<'_> {
        fn children(&mut self, element: ElementRef<'_>, depth: usize) -> Result<String> {
            let mut output = String::new();
            for child in element.children() {
                match child.value() {
                    Node::Text(text) => {
                        let mut space = false;
                        for c in text.chars() {
                            if c.is_whitespace() {
                                if !space {
                                    output.push(' ');
                                }
                                space = true;
                            } else {
                                if "\\`*_{}[]<>#!|".contains(c) {
                                    output.push('\\');
                                }
                                output.push(c);
                                space = false;
                            }
                        }
                    }
                    Node::Element(_) => {
                        if let Some(child) = ElementRef::wrap(child) {
                            output.push_str(&self.walk(child, depth + 1)?);
                        }
                    }
                    _ => {}
                }
            }
            Ok(output)
        }

        fn walk(&mut self, element: ElementRef<'_>, depth: usize) -> Result<String> {
            if depth > 128 {
                return Err(invalid_source("原文结构过于复杂"));
            }
            let tag = element.value().name();
            if excluded(element, self.base) {
                return Ok(String::new());
            }
            if tag == "pre" || tag == "code" {
                let code = element.text().collect::<String>();
                let ticks = code.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1;
                if tag == "pre" {
                    let fence = "`".repeat(ticks.max(3));
                    let language = element
                        .select(&selector("code"))
                        .next()
                        .and_then(|code| code.attr("class"))
                        .unwrap_or("")
                        .split_whitespace()
                        .find_map(|class| class.strip_prefix("language-"))
                        .filter(|language| {
                            language
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                        })
                        .unwrap_or("");
                    return Ok(format!(
                        "\n\n{fence}{language}\n{code}{}{fence}\n\n",
                        if code.ends_with('\n') { "" } else { "\n" }
                    ));
                }
                let fence = "`".repeat(ticks);
                return Ok(format!("{fence} {code} {fence}"));
            }
            if tag == "img" {
                let alt = element.attr("alt").unwrap_or("");
                let origin = [element.attr("data-src"), element.attr("src")]
                    .into_iter()
                    .flatten()
                    .find_map(|src| self.base.join(src).ok().filter(is_public_origin));
                if let Some(origin) = origin {
                    let index = if let Some(index) = self
                        .images
                        .iter()
                        .position(|image| image.origin == origin.as_str())
                    {
                        index
                    } else if self.images.len() < 64 {
                        let index = self.images.len();
                        self.images.push(super::full_text::Image {
                            origin: origin.to_string(),
                            stored: None,
                            failures: 0,
                            next_retry_at: None,
                        });
                        index
                    } else {
                        return Err(invalid_source("原文图片过多"));
                    };
                    return Ok(format!("![{}](reads-image:{index})", escape_markdown(alt)));
                }
                return Ok(if alt.is_empty() {
                    String::new()
                } else {
                    format!(" [Image: {}] ", escape_markdown(alt))
                });
            }
            if matches!(tag, "ul" | "ol") {
                let mut output = String::from("\n\n");
                let start = element
                    .attr("start")
                    .and_then(|value| value.parse::<u32>().ok())
                    .unwrap_or(1);
                for (index, child) in element
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|child| child.value().name() == "li")
                    .enumerate()
                {
                    let text = self.children(child, depth + 1)?;
                    let marker = if tag == "ol" {
                        format!("{}. ", start.saturating_add(index as u32))
                    } else {
                        "- ".into()
                    };
                    let indent = " ".repeat(marker.len());
                    output.push_str(&marker);
                    output.push_str(&text.trim().replace('\n', &format!("\n{indent}")));
                    output.push('\n');
                }
                output.push('\n');
                return Ok(output);
            }
            if tag == "table" {
                let mut rows = Vec::new();
                for row in element.select(&selector("tr")) {
                    let mut cells = Vec::new();
                    for cell in row
                        .children()
                        .filter_map(ElementRef::wrap)
                        .filter(|cell| matches!(cell.value().name(), "td" | "th"))
                    {
                        cells.push(self.children(cell, depth + 1)?.trim().replace('\n', " "));
                    }
                    if !cells.is_empty() {
                        rows.push(cells);
                    }
                }
                let width = rows.iter().map(Vec::len).max().unwrap_or(0);
                let mut output = String::from("\n\n");
                for (index, mut cells) in rows.into_iter().enumerate() {
                    cells.resize(width, String::new());
                    output.push_str(&format!("| {} |\n", cells.join(" | ")));
                    if index == 0 {
                        output.push_str(&format!("|{}\n", " --- |".repeat(width)));
                    }
                }
                output.push('\n');
                return Ok(output);
            }
            let text = self.children(element, depth)?;
            Ok(match tag {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => format!(
                    "\n\n{} {}\n\n",
                    "#".repeat(tag[1..].parse::<usize>().unwrap().max(2)),
                    text.trim()
                ),
                "a" => {
                    if let Some(url) = element
                        .attr("href")
                        .and_then(|href| self.base.join(href).ok())
                        .filter(|url| {
                            matches!(url.scheme(), "https" | "http")
                                && url.username().is_empty()
                                && url.password().is_none()
                        })
                    {
                        format!(
                            "[{}](<{}>)",
                            text.trim(),
                            url.as_str().replace('>', "%3E").replace('<', "%3C")
                        )
                    } else {
                        text
                    }
                }
                "strong" | "b" => format!("**{text}**"),
                "em" | "i" => format!("*{text}*"),
                "del" | "s" => format!("~~{text}~~"),
                "blockquote" => format!("\n\n> {}\n\n", text.trim().replace('\n', "\n> ")),
                "br" => "  \n".into(),
                "hr" => "\n\n---\n\n".into(),
                "p" | "div" | "section" | "article" | "main" | "figure" | "figcaption" => {
                    format!("\n\n{}\n\n", text.trim())
                }
                _ => text,
            })
        }
    }
    let mut writer = Writer {
        base,
        images: Vec::new(),
    };
    let body = writer.walk(root, 0)?;
    Ok((body.trim().to_owned(), writer.images))
}

fn escape_markdown(value: &str) -> String {
    let mut output = String::new();
    for c in value.chars() {
        if "\\`*_{}[]<>#!|".contains(c) {
            output.push('\\');
        }
        output.push(c);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_text_preserves_code_lists_tables_images_and_captions() {
        let url = Url::parse("https://example.com/posts/one").unwrap();
        let prose =
            "A detailed observation describing the experiment and its limitations. ".repeat(15);
        let text = format!(
            r#"<html lang="en"><article><h2>Experiment</h2><p>{prose}</p><p>Use <code>a &lt; b</code> and <strong>inspect</strong> the result.</p><pre><code class="language-rust">let text = "```";
  work();
</code></pre><ol start="3"><li>First<ul><li>Nested</li></ul></li><li>Second</li></ol><blockquote><p>Preserve this quotation.</p></blockquote><table><tr><th>Input</th><th>Output</th></tr><tr><td>2</td><td>4</td></tr></table><figure><img data-src="/plot.png" src="data:image/png;base64,x" alt="The result"><figcaption>Diagram by the author</figcaption></figure><img src="/plot.png" alt="Repeated"><figure><img src="/restricted.png" alt="Third party"><figcaption><a rel="license" href="/all-rights-reserved">Separate rights</a></figcaption></figure><img src="http://127.0.0.1/internal" alt="No private images"></article></html>"#
        );
        let source = extract(url.clone(), &text).unwrap();
        assert_eq!(source.images.len(), 2);
        assert_eq!(source.images[0].origin, "https://example.com/plot.png");
        assert_eq!(source.body.matches("reads-image:0").count(), 2);
        assert_eq!(
            source.images[1].origin,
            "https://example.com/restricted.png"
        );
        assert!(source.body.contains("Diagram by the author"));
        assert!(source.body.contains("Separate rights"));
        assert!(!source.body.contains("127.0.0.1"));
        let mut html = String::new();
        pulldown_cmark::html::push_html(
            &mut html,
            pulldown_cmark::Parser::new_ext(&source.body, super::super::full_text::OPTIONS),
        );
        let html = Html::parse_fragment(&html);
        assert_eq!(
            html.select(&selector("pre code"))
                .next()
                .unwrap()
                .text()
                .collect::<String>(),
            "let text = \"```\";\n  work();\n"
        );
        assert_eq!(html.select(&selector("ol[start='3'] > li")).count(), 2);
        assert_eq!(html.select(&selector("ol ul li")).count(), 1);
        assert_eq!(html.select(&selector("table tbody td")).count(), 2);
        assert_eq!(html.select(&selector("blockquote")).count(), 1);
    }

    #[test]
    fn full_text_does_not_depend_on_license_markup() {
        let url = Url::parse("https://example.com/posts/one").unwrap();
        let text = "An original paragraph with details and useful experience. ".repeat(20);
        for license in [
            "",
            "<a href='https://creativecommons.org/licenses/by/4.0/'>CC BY</a>",
            "All rights reserved",
        ] {
            let html = format!(
                "<html lang='en'><head><meta name='author' content='Ada'></head><body><article><h1>Real title</h1><p>{text}</p><img src='/diagram.png' alt='Experiment'><p>{license}</p><script>steal()</script></article><footer>Site navigation</footer></body></html>"
            );
            let source = extract(url.clone(), &html).unwrap();
            assert_eq!(source.author, "Ada");
            assert!(source.body.contains("original paragraph"));
            assert_eq!(source.images[0].origin, "https://example.com/diagram.png");
            assert!(!source.body.contains("steal()"));
            assert!(!source.body.contains("Site navigation"));
        }
        let paywall = format!("<article><p>{text}</p><p>Subscribe to continue</p></article>");
        assert!(extract(url, &paywall).is_err());
    }

    #[test]
    fn article_body_hints_exclude_recommendations() {
        let text = "A useful detailed original observation about how a team evaluates its work. "
            .repeat(20);
        for (url, class) in [
            ("https://zenn.dev/author/articles/example", "znc"),
            ("https://github.blog/engineering/example", "post__content"),
            ("https://blog.octo.com/example", "articleContent"),
        ] {
            let html = format!(
                "<html lang='en'><head><meta property='article:published_time' content='2026-09-22T10:00:00Z'></head><body><div class='{class}'><h1>Real article</h1><p>{text}</p></div><aside><article>Unrelated recommendation</article><article>Another recommendation</article></aside><footer><a rel='license' href='https://creativecommons.org/licenses/by/4.0/'>Site assets</a></footer></body></html>"
            );
            let source = extract(Url::parse(url).unwrap(), &html).unwrap();
            assert!(source.body.contains("original observation"));
            assert!(!source.body.contains("recommendation"));
            assert_eq!(
                source.published_at.unwrap().to_rfc3339(),
                "2026-09-22T10:00:00+00:00"
            );
        }
    }

    #[test]
    fn article_body_excludes_embedded_sharing_widgets() {
        let prose =
            "Share this approach with your team: verify the data before making a decision. "
                .repeat(15);
        let html = format!(
            "<article><div class='entry-content'><h2>Sharing data safely</h2><p>{prose}</p>\
             <figure><img src='/diagram.png'><figcaption>How data is shared</figcaption></figure>\
             <pre><code>share(data);</code></pre>\
             <div class='sharedaddy sd-sharing-enabled'><div class='sd-sharing'><h3>Share this:</h3>\
             <ul><li><a href='?share=facebook'>Facebook</a></li><li><a href='?share=x'>X</a></li></ul></div></div>\
             <div class='share-buttons'><a href='/share'><img src='/share-icon.png'>Share on social media</a></div>\
             <p>The final observation belongs to the article.</p></div></article>"
        );
        let source = extract(Url::parse("https://example.com/post").unwrap(), &html).unwrap();
        assert!(source.body.contains(prose.trim()));
        assert!(source.body.contains("Sharing data safely"));
        assert!(source.body.contains("How data is shared"));
        assert!(source.body.contains("share(data);"));
        assert!(source.body.contains("The final observation"));
        assert!(!source.body.contains("Share this:"));
        assert!(!source.body.contains("?share="));
        assert!(!source.body.contains("Share on social media"));
        assert_eq!(source.images.len(), 1);
        assert_eq!(source.images[0].origin, "https://example.com/diagram.png");
    }

    #[test]
    fn publisher_body_containers_keep_article_content_and_exclude_page_widgets() {
        let prose =
            "This experiment examines comments and subscriptions as real product features. "
                .repeat(15);
        for (url, open, close) in [
            (
                "https://www.infoq.com/news/example",
                "<div class='article__data'>",
                "</div>",
            ),
            (
                "https://www.quantamagazine.org/example",
                "<div id='postBody'>",
                "</div>",
            ),
            (
                "https://huggingface.co/blog/example",
                "<div class='blog-content'><div class='relative'>",
                "</div></div>",
            ),
            (
                "https://example.substack.com/p/example",
                "<div class='body markup'>",
                "</div>",
            ),
        ] {
            let header = if url.contains("quantamagazine.org") {
                "<div class='post__title__wrapper'><h1 class='post__title__title'>Unrelated repeated title</h1><div class='post__title__author-date'>Unrelated date</div><div class='post__title__excerpt'>The article's introductory standfirst.</div><div class='post__title__actions'>Unrelated controls</div></div>"
            } else {
                ""
            };
            let html = format!(
                "<main><article><p>Page navigation</p>{open}{header}<h2 id='comments'>Experiment</h2><p>{prose}</p>\
                 <figure><img src='/plot.png'><figcaption>A caption with attribution</figcaption></figure>\
                 <div class='subscription-widget-wrap'><p>Unrelated subscription offer</p></div>\
                 <div role='toolbar'>Unrelated controls</div><section id='comments'>Unrelated comments</section>\
                 <div class='related-posts'><img src='/advert.png'>Unrelated recommendations</div>\
                 <div class='author-section-full'>Unrelated author panel</div>\
                 <p>Final observation.</p><h2>References</h2><ol><li><a href='/paper'>Research paper</a></li></ol>\
                 {close}<section>Unrelated footer</section></article></main>"
            );
            let source = extract(Url::parse(url).unwrap(), &html).unwrap();
            assert!(source.body.contains(prose.trim()), "{url}");
            assert!(source.body.contains("A caption with attribution"));
            assert!(source.body.contains("Final observation"));
            assert!(source.body.contains("Research paper"));
            if !header.is_empty() {
                assert!(source.body.contains("introductory standfirst"));
            }
            assert!(!source.body.contains("Page navigation"));
            assert!(!source.body.contains("Unrelated"), "{url}: {}", source.body);
            assert_eq!(source.images.len(), 1);
        }
    }

    #[test]
    fn short_article_body_never_falls_back_to_surrounding_page_text() {
        let clutter = "Unrelated navigation and recommendation text. ".repeat(40);
        for (url, body) in [
            (
                "https://www.infoq.com/news/example",
                "<div class='article__data'><p>Only an excerpt.</p></div>",
            ),
            (
                "https://example.com/post",
                "<div class='entry-content'><p>Only an excerpt.</p></div>",
            ),
        ] {
            let html = format!("<article>{body}<p>{clutter}</p></article>");
            assert!(extract(Url::parse(url).unwrap(), &html).is_err());
        }
        assert!(
            extract(
                Url::parse("https://example.com/page").unwrap(),
                &format!("<main>{clutter}</main>")
            )
            .is_err()
        );
    }

    #[test]
    fn excerpts_and_complex_markup_never_become_full_articles() {
        let url = Url::parse("https://example.com/post").unwrap();
        let text = "A detailed paragraph about a useful experiment and what changed. ".repeat(20);
        let excerpt = format!(
            "<article><p>{text}</p><a class='read-more' href='/subscribe'>Continue</a></article>"
        );
        assert!(extract(url.clone(), &excerpt).is_err());
        let nested = format!(
            "<article>{}<p>{text}</p>{}</article>",
            "<div>".repeat(150),
            "</div>".repeat(150)
        );
        assert!(extract(url, &nested).is_err());
    }
}
