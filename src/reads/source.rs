use super::invalid_source;
use crate::error::{AppError, Result};
use reqwest::{Client, redirect::Policy};
use scraper::{ElementRef, Html, Node, Selector};
use serde::Deserialize;
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;

const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LicenseGrant {
    prefix: Url,
    name: String,
    license_url: Url,
    /// A selector identifying the complete article, excluding site navigation.
    selector: String,
}

#[derive(Debug)]
pub(super) struct License {
    pub name: String,
    pub url: String,
}

pub(super) struct Source {
    pub url: Url,
    pub title: String,
    pub author: String,
    pub language: String,
    pub body: String,
    pub license: Option<License>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub(crate) fn license_grants(value: &str) -> Result<Vec<LicenseGrant>> {
    let grants: Vec<LicenseGrant> = serde_json::from_str(value).map_err(|_| {
        AppError::Config("NEWS_READS_LICENSES 必须是有效的授权配置 JSON 数组".into())
    })?;
    for grant in &grants {
        if !public_url(&grant.prefix)
            || !public_url(&grant.license_url)
            || grant.prefix.query().is_some()
            || grant.prefix.fragment().is_some()
            || grant.name.trim().is_empty()
            || grant.name.len() > 160
            || grant.selector.trim().is_empty()
            || Selector::parse(&grant.selector).is_err()
        {
            return Err(AppError::Config(
                "NEWS_READS_LICENSES 的 URL、授权名称或正文选择器无效".into(),
            ));
        }
    }
    Ok(grants)
}

impl LicenseGrant {
    fn matches(&self, url: &Url) -> bool {
        self.prefix.origin() == url.origin()
            && (self.prefix.path() == url.path()
                || (self.prefix.path().ends_with('/')
                    && url.path().starts_with(self.prefix.path())))
    }
}

pub(super) async fn fetch(value: &str, grants: &[LicenseGrant]) -> Result<Source> {
    tokio::time::timeout(Duration::from_secs(30), async {
        let url = Url::parse(value).map_err(|_| invalid_source("原文 URL 无效"))?;
        if let Some(id) = x_status(&url) {
            return fetch_post(&id).await;
        }
        let (url, bytes) = download(url, Content::Html).await?;
        let html =
            std::str::from_utf8(&bytes).map_err(|_| invalid_source("原文不是 UTF-8 文本"))?;
        extract(url, html, grants)
    })
    .await
    .map_err(|_| invalid_source("读取原文超时"))?
}

#[derive(Clone, Copy)]
pub(super) enum Content {
    Html,
    Json,
    Feed,
}

pub(super) async fn download(url: Url, content: Content) -> Result<(Url, Vec<u8>)> {
    tokio::time::timeout(Duration::from_secs(30), download_inner(url, content))
        .await
        .map_err(|_| invalid_source("读取来源超时"))?
}

async fn download_inner(mut url: Url, content: Content) -> Result<(Url, Vec<u8>)> {
    for _ in 0..5 {
        if !public_url(&url) {
            return Err(invalid_source("原文必须是公开 HTTPS 地址"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| invalid_source("原文缺少域名"))?;
        let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, 443))
            .await
            .map_err(|_| invalid_source("原文域名无法解析"))?
            .collect();
        if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
            return Err(invalid_source("原文地址不可公开访问"));
        }
        // Pin validated addresses, including each redirect, so a DNS rebind cannot
        // reach internal services. Never forward application credentials or cookies.
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .resolve_to_addrs(host, &addresses)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .user_agent("XABCNews-Reads/1.0 (+https://xabcnews.com/reads)")
            .build()?;
        let mut response = client.get(url.clone()).send().await?;
        if response.status().is_redirection() {
            url = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|header| header.to_str().ok())
                .and_then(|location| url.join(location).ok())
                .ok_or_else(|| invalid_source("原文重定向无效"))?;
            continue;
        }
        response = response.error_for_status()?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        let accepted = match content {
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
            .is_some_and(|size| size > MAX_BYTES as u64)
        {
            return Err(invalid_source("原文网页过大"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > MAX_BYTES {
                return Err(invalid_source("原文网页过大"));
            }
            bytes.extend_from_slice(&chunk);
        }
        return Ok((url, bytes));
    }
    Err(invalid_source("原文重定向过多"))
}

fn public_url(url: &Url) -> bool {
    crate::media::is_public_https_url(url) && url.port_or_known_default() == Some(443)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || a == 0
                || a >= 224
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0))
        }
        IpAddr::V6(ip) => {
            let [a, b, ..] = ip.segments();
            // Global unicast only; exclude transition, documentation and protocol ranges.
            (0x2000..0x4000).contains(&a)
                && a != 0x2002
                && !(a == 0x2001 && (b < 0x200 || b == 0xdb8))
                && !(a == 0x3fff && b < 0x1000)
        }
    }
}

pub(super) fn x_status(url: &Url) -> Option<String> {
    if !public_url(url)
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

async fn fetch_post(id: &str) -> Result<Source> {
    let url = Url::parse(&format!("https://x.com/i/status/{id}")).expect("fixed X URL");
    let mut endpoint = Url::parse("https://publish.twitter.com/oembed").expect("fixed oEmbed URL");
    endpoint
        .query_pairs_mut()
        .append_pair("url", url.as_str())
        .append_pair("omit_script", "true")
        .append_pair("dnt", "true");
    let (_, bytes) = download(endpoint, Content::Json).await?;
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
    let body = markdown(paragraph, &url)?;
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
        license: None,
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

fn extract(url: Url, text: &str, grants: &[LicenseGrant]) -> Result<Source> {
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
    let grant = grants.iter().find(|grant| grant.matches(&url));
    let article_meta = meta(&html, "meta[property='og:type']") == "article";
    let body_selector = article_selector(&url);
    if grant.is_none()
        && (url.path() == "/"
            || (body_selector.is_none()
                && !article_meta
                && html.select(&selector("article")).count() > 1))
    {
        return Err(invalid_source("原文链接不是独立文章页"));
    }
    let mut selectors = grant
        .map(|grant| vec![grant.selector.as_str()])
        .unwrap_or_else(|| {
            vec![
                "[itemprop='articleBody']",
                ".entry-content",
                ".post-content",
                ".article-body",
                ".post-body",
                ".rich-editor",
                ".markdown-body",
                "#article_body",
                "article",
                "main",
            ]
        });
    if grant.is_none()
        && let Some(query) = body_selector
    {
        selectors.insert(0, query);
    }
    let (root, body, semantic) = selectors
        .iter()
        .find_map(|query| {
            html.select(&selector(query)).find_map(|root| {
                let body = markdown(root, &url).ok()?;
                (body.chars().count() >= 600).then_some((root, body, *query != "main"))
            })
        })
        .ok_or_else(|| invalid_source("未找到完整的文章正文"))?;
    if body.chars().count() > 80000 {
        return Err(invalid_source("原文超出阅读长度限制"));
    }
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
    let license = if let Some(grant) = grant {
        Some(License {
            name: grant.name.clone(),
            url: grant.license_url.to_string(),
        })
    } else if semantic {
        // Only article-scoped or document metadata licenses count. Footer links often
        // apply to the website's code/assets, not to the article being republished.
        root.select(&selector("a[rel~='license']"))
            .chain(html.select(&selector("head link[rel~='license']")))
            .filter_map(|node| node.attr("href"))
            .filter_map(|href| url.join(href).ok())
            .find_map(open_license)
    } else {
        None
    };
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
        license,
        published_at,
    })
}

// Extraction hints only; CSS selectors do not grant republication rights.
fn article_selector(url: &Url) -> Option<&'static str> {
    match url.host_str()?.trim_start_matches("www.") {
        "blog.cloudflare.com" | "shopify.engineering" | "heise.de" => Some(".article-content"),
        "github.blog" => Some(".post__content"),
        "stackoverflow.blog" => Some(".s-prose"),
        "theregister.com" => Some("#main .bodytext"),
        "zenn.dev" => Some(".znc"),
        "qiita.com" => Some(".mdContent-inner"),
        "huggingface.co" => Some(".blog-content"),
        "blog.octo.com" => Some(".articleContent"),
        "t3n.de" => Some("#article"),
        _ => None,
    }
}

fn open_license(url: Url) -> Option<License> {
    if url.host_str()? != "creativecommons.org" || !matches!(url.scheme(), "https" | "http") {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    let name = match path {
        "/licenses/by/4.0" => "CC BY 4.0",
        "/licenses/by-sa/4.0" => "CC BY-SA 4.0",
        "/publicdomain/zero/1.0" => "CC0 1.0",
        _ => return None,
    };
    Some(License {
        name: name.into(),
        url: format!("https://creativecommons.org{path}/"),
    })
}

fn markdown(root: ElementRef<'_>, base: &Url) -> Result<String> {
    fn walk(
        element: ElementRef<'_>,
        base: &Url,
        output: &mut String,
        verbatim: bool,
        depth: usize,
    ) -> Result<()> {
        if depth > 128 {
            return Err(invalid_source("原文结构过于复杂"));
        }
        let tag = element.value().name();
        if matches!(
            tag,
            "script"
                | "style"
                | "nav"
                | "aside"
                | "footer"
                | "form"
                | "button"
                | "noscript"
                | "svg"
        ) || element.attr("hidden").is_some()
            || element.attr("aria-hidden") == Some("true")
        {
            return Ok(());
        }
        let block = matches!(
            tag,
            "p" | "div"
                | "section"
                | "article"
                | "main"
                | "ul"
                | "ol"
                | "blockquote"
                | "table"
                | "tr"
                | "figure"
                | "figcaption"
        );
        if block || tag.starts_with('h') && tag.len() == 2 {
            output.push_str("\n\n");
        }
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => output.push_str("## "),
            "li" => output.push_str("\n- "),
            "br" => output.push_str("  \n"),
            "pre" => output.push_str("\n\n````````\n"),
            "a" => output.push('['),
            "img" => {
                if let Some(alt) = element.attr("alt").filter(|alt| !alt.is_empty()) {
                    output.push_str(&format!(" [图片：{}] ", escape_markdown(alt)));
                }
            }
            _ => {}
        }
        for child in element.children() {
            match child.value() {
                Node::Text(text) => {
                    if verbatim || tag == "pre" {
                        output.push_str(text);
                    } else {
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
                }
                Node::Element(_) => {
                    if let Some(child) = ElementRef::wrap(child) {
                        walk(child, base, output, verbatim || tag == "pre", depth + 1)?;
                    }
                }
                _ => {}
            }
        }
        if tag == "a" {
            output.push(']');
            if let Some(url) = element
                .attr("href")
                .and_then(|href| base.join(href).ok())
                .filter(|url| matches!(url.scheme(), "https" | "http"))
            {
                output.push_str(&format!(
                    "(<{}>)",
                    url.as_str().replace('>', "%3E").replace('<', "%3C")
                ));
            }
        }
        if tag == "pre" {
            output.push_str("\n````````\n\n");
        }
        if matches!(tag, "td" | "th") {
            output.push_str(" · ");
        }
        if block || tag.starts_with('h') && tag.len() == 2 {
            output.push_str("\n\n");
        }
        Ok(())
    }
    let mut output = String::new();
    walk(root, base, &mut output, false, 0)?;
    Ok(output.trim().to_owned())
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
    fn refuses_private_and_special_networks() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "192.0.2.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(ip.parse().unwrap()));
        }
    }

    #[test]
    fn only_verified_article_licenses_enable_full_text() {
        let url = Url::parse("https://example.com/posts/one").unwrap();
        let text = "An original paragraph with details and useful experience. ".repeat(20);
        let document = |license: &str, footer: &str| {
            format!(
                "<html lang='en'><head><meta name='author' content='Ada'></head><body><article><h1>Real title</h1><p>{text}</p>{license}<script>steal()</script></article><footer>{footer}</footer></body></html>"
            )
        };
        let cc = "<a rel='license' href='https://creativecommons.org/licenses/by/4.0/'>CC BY</a>";
        let source = extract(url.clone(), &document(cc, ""), &[]).unwrap();
        assert_eq!(source.license.unwrap().name, "CC BY 4.0");
        assert_eq!(source.author, "Ada");
        assert!(!source.body.contains("steal()"));
        assert!(
            extract(url.clone(), &document("", cc), &[])
                .unwrap()
                .license
                .is_none()
        );
        let grants = license_grants(r#"[{"prefix":"https://example.com/posts/","name":"Author permission","license_url":"https://example.com/permission","selector":"article"}]"#).unwrap();
        assert!(
            extract(url.clone(), &document("", ""), &grants)
                .unwrap()
                .license
                .is_some()
        );
        assert!(
            !grants[0].matches(&Url::parse("https://example.com/posts-elsewhere/one").unwrap())
        );
        assert!(!grants[0].matches(&Url::parse("https://evil.example/posts/one").unwrap()));
        assert!(extract(url, &document("<p>Subscribe to continue</p>", ""), &[]).is_err());
    }

    #[test]
    fn article_body_hints_exclude_recommendations_and_do_not_grant_a_license() {
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
            let source = extract(Url::parse(url).unwrap(), &html, &[]).unwrap();
            assert!(source.body.contains("original observation"));
            assert!(!source.body.contains("recommendation"));
            assert!(source.license.is_none());
            assert_eq!(
                source.published_at.unwrap().to_rfc3339(),
                "2026-09-22T10:00:00+00:00"
            );
        }
    }

    #[test]
    fn excerpts_and_complex_markup_never_become_full_articles() {
        let url = Url::parse("https://example.com/post").unwrap();
        let text = "A detailed paragraph about a useful experiment and what changed. ".repeat(20);
        let excerpt = format!(
            "<article><p>{text}</p><a class='read-more' href='/subscribe'>Continue</a></article>"
        );
        assert!(extract(url.clone(), &excerpt, &[]).is_err());
        let nested = format!(
            "<article>{}<p>{text}</p>{}</article>",
            "<div>".repeat(150),
            "</div>".repeat(150)
        );
        assert!(extract(url, &nested, &[]).is_err());
    }
}
