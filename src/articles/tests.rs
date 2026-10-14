use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A JPEG of noise, which compresses badly: at 400×300 it is larger than the 64 KiB that other
/// requests may carry.
fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let mut seed = 0x2545_f491_u32;
    let pixels = image::RgbImage::from_fn(width, height, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let [r, g, b, _] = seed.to_le_bytes();
        image::Rgb([r, g, b])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(pixels)
        .write_to(&mut bytes, image::ImageFormat::Jpeg)
        .unwrap();
    bytes.into_inner()
}

#[test]
fn excerpts_are_one_line_of_the_body_without_markup_or_picture_descriptions() {
    let id = Uuid::from_u128(7);
    assert_eq!(
        excerpt(&format!(
            "## 开头\n\nHello **world**, `code` and [a link](https://example.com).\n\n![一只猫](image:{id})\n\n- one\n- two"
        )),
        "开头 Hello world, code and a link. one two"
    );
    let long = "字".repeat(EXCERPT_CHARS + 30);
    let cut = excerpt(&long);
    assert_eq!(cut.chars().count(), EXCERPT_CHARS + 1);
    assert!(cut.ends_with('…'));
    assert_eq!(excerpt("  \n\n "), "");
}

#[test]
fn pen_names_are_trimmed_and_bounded() {
    assert_eq!(pen_name("  小明  ").unwrap(), "小明");
    assert_eq!(pen_name("Ada Lovelace").unwrap(), "Ada Lovelace");
    assert!(pen_name("小").is_err());
    assert!(pen_name(&"名".repeat(25)).is_err());
    assert!(pen_name("小\n明").is_err());
}

#[test]
fn an_article_shows_at_most_its_allowance_of_pictures_with_the_cover_counted_once() {
    let ids: Vec<Uuid> = (1..=33).map(Uuid::from_u128).collect();
    let body = |count: usize| {
        ids[..count]
            .iter()
            .map(|id| format!("![](image:{id}) ![again](image:{id})"))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let shown = checked("Title", &body(32), Some(ids[0])).unwrap();
    assert_eq!(pictures(&shown.body).len(), 32);
    assert!(checked("Title", &body(32), Some(ids[32])).is_err());
    assert!(checked("Title", &body(31), Some(ids[32])).is_ok());
    // Addresses that aren't the article's own pictures don't count.
    assert!(pictures("![](https://example.com/a.png) ![](image:nope)").is_empty());
    assert_eq!(checked("  标题  ", "", None).unwrap().title, "标题");
    assert!(checked(&"题".repeat(TITLE_CHARS + 1), "", None).is_err());
    assert!(checked("", &"字".repeat(BODY_CHARS + 1), None).is_err());
}

#[test]
fn only_jpeg_png_webp_and_gif_pictures_are_taken_and_their_size_is_read() {
    assert_eq!(kind(&jpeg(64, 48)).unwrap(), ("image/jpeg", (64, 48)));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(3, 2))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    assert_eq!(kind(png.get_ref()).unwrap(), ("image/png", (3, 2)));
    let mut gif = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(5, 4))
        .write_to(&mut gif, image::ImageFormat::Gif)
        .unwrap();
    assert_eq!(kind(gif.get_ref()).unwrap(), ("image/gif", (5, 4)));
    for refused in [
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".as_slice(),
        b"<html><script>alert(1)</script></html>",
        b"",
    ] {
        assert!(matches!(
            kind(refused),
            Err(AppError::UnsupportedMediaType(_))
        ));
    }
    // A picture's signature without a readable picture behind it.
    assert!(matches!(
        kind(b"\xFF\xD8\xFF\xE0broken"),
        Err(AppError::BadRequest(_))
    ));
}

#[test]
fn an_article_renders_only_its_own_pictures_and_no_markup() {
    let own = Uuid::from_u128(1);
    let foreign = Uuid::from_u128(2);
    let html = crate::site::markdown(
        Parser::new_ext(
            &format!(
                "![猫](image:{own}) ![狗](image:{foreign}) ![外链](https://tracker.example/p.png)\n\n<script>alert(1)</script> [bad](javascript:alert%281%29)"
            ),
            MARKDOWN,
        ),
        |destination| {
            (image_id(destination) == Some(own)).then(|| crate::site::Picture {
                src: "https://media.example/files/cat.jpg".into(),
                size: Some((640, 480)),
            })
        },
    );
    assert!(html.contains(
        "<img src=\"https://media.example/files/cat.jpg\" alt=\"猫\" width=640 height=480 loading=lazy"
    ));
    assert_eq!(html.matches("<img ").count(), 1);
    assert!(html.contains("狗") && html.contains("外链"));
    for bad in ["<script", "javascript:", "tracker.example"] {
        assert!(!html.contains(bad), "{html}");
    }
}

/// A local tmpfile over HTTPS: uploads get a link under its origin, and every file is `picture`.
/// While the returned switch is on, it refuses uploads.
async fn tmpfile(picture: Vec<u8>) -> (String, reqwest::Client, Arc<AtomicBool>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, rustls};
    let cert = rcgen::generate_simple_self_signed(vec!["media.example".into()]).unwrap();
    let root = reqwest::Certificate::from_der(cert.cert.der()).unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = "https://media.example".to_owned();
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::https(format!("http://{addr}")).unwrap())
        .tls_certs_only([root])
        .build()
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let refuse = Arc::new(AtomicBool::new(false));
    let refusing = refuse.clone();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (acceptor, picture) = (acceptor.clone(), picture.clone());
            let refused = refusing.load(Ordering::SeqCst);
            tokio::spawn(async move {
                let mut connect = Vec::new();
                while !connect.ends_with(b"\r\n\r\n") {
                    let Ok(byte) = socket.read_u8().await else {
                        return;
                    };
                    connect.push(byte);
                }
                socket
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await
                    .unwrap();
                let Ok(mut stream) = acceptor.accept(socket).await else {
                    return;
                };
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    let Ok(byte) = stream.read_u8().await else {
                        return;
                    };
                    head.push(byte);
                }
                let head = String::from_utf8(head).unwrap();
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let length: usize = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|n| n.parse().ok())
                    })
                    .unwrap_or(0);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                let refused = refused && path == "/upload";
                let (kind, body) = if refused {
                    ("text/plain", b"full".to_vec())
                } else if path == "/upload" {
                    let link = json!({
                        "direct_link": format!("https://media.example/files/{}.jpg", Uuid::new_v4()),
                        "expires_at": Utc::now() + chrono::Duration::days(30),
                    });
                    ("application/json", link.to_string().into_bytes())
                } else {
                    ("image/jpeg", picture)
                };
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    if refused {
                        "507 Insufficient Storage"
                    } else {
                        "200 OK"
                    },
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
                stream.shutdown().await.ok();
            });
        }
    });
    (base, client, refuse)
}

/// A signed-in account: its id and the cookies a browser would send.
async fn account(state: &AppState, role: &str) -> (i64, String) {
    use sha2::{Digest, Sha256};
    let token = Uuid::new_v4().to_string();
    let id: i64 =
        sqlx::query_scalar("INSERT INTO users (email, role) VALUES ($1, $2) RETURNING id")
            .bind(format!("{token}@articles.example.test"))
            .bind(role)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, NOW() + INTERVAL '1 day')",
    )
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .bind(id)
    .execute(&state.pool)
    .await
    .unwrap();
    (
        id,
        format!(
            "{}={token}; news_role={role}",
            state.config.session_cookie_name
        ),
    )
}

#[tokio::test]
#[ignore = "requires NEWS_ARTICLES_TEST_DATABASE_URL and local test configuration"]
async fn postgres_articles_are_written_reviewed_published_and_counted() {
    use crate::config::Config;
    let database_url =
        std::env::var("NEWS_ARTICLES_TEST_DATABASE_URL").expect("disposable database URL");
    let mut config = Config::from_env().unwrap();
    config.database_url = database_url;
    config.redis_addr = None;
    config.scheduler_enabled = false;
    let picture = jpeg(400, 300);
    assert!(picture.len() > 64 * 1024);
    let (media, client, refuse) = tmpfile(picture.clone()).await;
    config.tmpfile_base_url = media.parse().unwrap();
    config.tmpfile_api_key = "fixture".into();
    let mut state = AppState::new(config).await.unwrap();
    state.http = client;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_articles_test"));
    sqlx::query("TRUNCATE articles, article_images, authors CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::web::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let origin = state.config.public_base_url.origin().ascii_serialization();
    let (writer, author) = account(&state, "user").await;
    let (_, reader) = account(&state, "user").await;
    let (_, admin) = account(&state, "admin").await;
    let send = |method: reqwest::Method, path: &str, cookie: &str| {
        http.request(method, format!("{base}{path}"))
            .header("cookie", cookie)
            .header("origin", &origin)
    };
    let get = |path: &str, cookie: &str| send(reqwest::Method::GET, path, cookie);
    let anonymous = |path: &str| http.get(format!("{base}{path}"));
    let status = |response: reqwest::Response| response.status().as_u16();

    // Writing starts with a pen name, which is the author's alone.
    let mine: Value = get("/api/articles", &author)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mine["author"], Value::Null);
    let first = json!({ "author": writer, "title": "", "body": "" });
    let response = send(reqwest::Method::POST, "/api/articles", &author)
        .json(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(status(response), 409);
    let response = send(reqwest::Method::PUT, "/api/author", &author)
        .json(&json!({ "name": "  小明 " }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.json::<Value>().await.unwrap()["name"], "小明");
    for (name, expected) in [("小明", 409), ("小", 400)] {
        let response = send(reqwest::Method::PUT, "/api/author", &reader)
            .json(&json!({ "name": name }))
            .send()
            .await
            .unwrap();
        assert_eq!(status(response), expected, "{name}");
    }
    let created: Value = send(reqwest::Method::POST, "/api/articles", &author)
        .json(&first)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (created["status"].as_str(), created["revision"].as_i64()),
        (Some("draft"), Some(1))
    );
    // A writing page opened for another account doesn't write under this one.
    let response = send(reqwest::Method::POST, "/api/articles", &author)
        .json(&json!({ "author": writer + 1, "title": "", "body": "" }))
        .send()
        .await
        .unwrap();
    assert_eq!(status(response), 409);

    // Pictures are uploaded as they are, and only pictures.
    let upload = |cookie: &str, bytes: Vec<u8>| {
        send(
            reqwest::Method::POST,
            &format!("/api/articles/{id}/images"),
            cookie,
        )
        .header("content-type", "image/jpeg")
        .body(bytes)
    };
    let response = upload(&author, picture.clone()).send().await.unwrap();
    assert_eq!(response.status(), 201);
    let image: Value = response.json().await.unwrap();
    assert_eq!(
        (image["width"].as_i64(), image["height"].as_i64()),
        (Some(400), Some(300))
    );
    let image = image["id"].as_str().unwrap().to_owned();
    assert_eq!(
        status(upload(&author, b"<svg/>".to_vec()).send().await.unwrap()),
        415
    );
    assert_eq!(
        status(upload(&reader, picture.clone()).send().await.unwrap()),
        404
    );
    // A picture tmpfile didn't keep doesn't count against the day.
    let uploaded = || async {
        sqlx::query_scalar::<_, i16>("SELECT images FROM authors WHERE user_id = $1")
            .bind(writer)
            .fetch_one(&state.pool)
            .await
            .unwrap()
    };
    assert_eq!(uploaded().await, 1);
    refuse.store(true, Ordering::SeqCst);
    assert_eq!(
        status(upload(&author, picture.clone()).send().await.unwrap()),
        503
    );
    refuse.store(false, Ordering::SeqCst);
    assert_eq!(uploaded().await, 1);

    // Saves follow the revision the editor holds.
    let body = format!("Hello **world**\n\n![猫](image:{image})\n\n<script>alert(1)</script>");
    let save = |cookie: &str, revision: i64, body: &str| {
        send(
            reqwest::Method::PATCH,
            &format!("/api/articles/{id}"),
            cookie,
        )
        .json(&json!({
            "revision": revision, "title": "第一篇", "body": body, "cover": image,
        }))
    };
    let saved: Value = save(&author, 1, &body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (saved["status"].as_str(), saved["revision"].as_i64()),
        (Some("draft"), Some(2))
    );
    assert_eq!(status(save(&author, 1, &body).send().await.unwrap()), 409);
    assert_eq!(status(save(&reader, 2, &body).send().await.unwrap()), 404);
    // A cover is one of the article's own pictures, not another article's.
    let other: Value = send(reqwest::Method::POST, "/api/articles", &author)
        .json(&first)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let elsewhere: Value = send(
        reqwest::Method::POST,
        &format!("/api/articles/{}/images", other["id"].as_str().unwrap()),
        &author,
    )
    .body(picture.clone())
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    for cover in [json!(Uuid::new_v4()), elsewhere["id"].clone()] {
        let response = send(
            reqwest::Method::PATCH,
            &format!("/api/articles/{id}"),
            &author,
        )
        .json(&json!({ "revision": 2, "title": "", "body": "", "cover": cover }))
        .send()
        .await
        .unwrap();
        assert_eq!(status(response), 400, "{cover}");
    }
    for path in [
        format!("/api/articles/{id}"),
        format!("/api/articles/{id}/preview"),
        format!("/api/articles/{id}/images/{image}"),
    ] {
        assert_eq!(
            status(get(&path, &reader).send().await.unwrap()),
            404,
            "{path}"
        );
    }

    // The author previews it with pictures read through this server.
    let response = get(&format!("/api/articles/{id}/preview"), &author)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    let page = response.text().await.unwrap();
    assert!(page.contains(&format!("/api/articles/{id}/images/{image}")));
    assert!(!page.contains("media.example") && !page.contains("<script>alert"));
    let response = get(&format!("/api/articles/{id}/images/{image}"), &author)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "private, no-cache");
    let tag = response.headers()["etag"].to_str().unwrap().to_owned();
    assert_eq!(response.bytes().await.unwrap(), picture);
    let response = get(&format!("/api/articles/{id}/images/{image}"), &author)
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(status(response), 304);
    // Another account can't use a tag the browser kept.
    let response = get(&format!("/api/articles/{id}/images/{image}"), &reader)
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(status(response), 404);

    // Submitted, it waits for a ❤️ for exactly the revision the administrator read.
    assert_eq!(
        status(anonymous(&format!("/articles/{id}")).send().await.unwrap()),
        404
    );
    let submit = |revision: i64| {
        send(
            reqwest::Method::POST,
            &format!("/api/articles/{id}/submit"),
            &author,
        )
        .json(&json!({ "revision": revision }))
    };
    assert_eq!(status(submit(1).send().await.unwrap()), 409);
    let submitted: Value = submit(2).send().await.unwrap().json().await.unwrap();
    assert_eq!(submitted["status"], "pending");
    assert_eq!(
        status(anonymous(&format!("/articles/{id}")).send().await.unwrap()),
        404
    );
    let review = get(&format!("/api/admin/preview/articles/{id}"), &admin)
        .send()
        .await
        .unwrap();
    assert_eq!(review.headers()["cache-control"], "private, no-store");
    let review = review.text().await.unwrap();
    assert!(review.contains("data-article-review") && review.contains("data-revision=\"2\""));
    assert_eq!(
        status(
            get(&format!("/api/admin/preview/articles/{id}"), &reader)
                .send()
                .await
                .unwrap()
        ),
        403
    );
    let heart = |cookie: &str, published: bool, revision: i64| {
        send(
            reqwest::Method::PATCH,
            &format!("/api/admin/articles/{id}"),
            cookie,
        )
        .json(&json!({ "published": published, "revision": revision }))
    };
    assert_eq!(status(heart(&reader, true, 2).send().await.unwrap()), 403);
    assert_eq!(status(heart(&admin, true, 1).send().await.unwrap()), 409);
    let queue: Value = get("/api/admin/articles", &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queue["pending"][0]["id"], id.as_str());
    assert_eq!(queue["pending"][0]["author"], "小明");
    assert_eq!(status(heart(&admin, true, 2).send().await.unwrap()), 200);

    // Readers see it under the pen name, with its tmpfile pictures, and never the email.
    let list = anonymous("/articles")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(list.contains("第一篇") && list.contains("小明") && list.contains("Hello world"));
    assert!(!list.contains("articles.example.test"));
    // Its tab follows 精读's on every page, and the sitemap lists it.
    assert!(list.contains("<a class=tab href=\"/articles\" aria-current=\"true\">分享</a>"));
    let reads = anonymous("/reads")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(reads.contains(
        "<a class=tab href=\"/reads\" aria-current=\"true\">精读</a><a class=tab href=\"/articles\">分享</a>"
    ));
    let sitemaps = anonymous("/sitemap.xml")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(sitemaps.contains("/articles/sitemap.xml</loc>"));
    let listed = || async {
        anonymous("/articles/sitemap.xml")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    };
    assert!(listed().await.contains(&format!("/articles/{id}</loc>")));
    let page = anonymous(&format!("/articles/{id}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("https://media.example/files/") && page.contains("og:image"));
    assert!(page.contains(&format!("data-article-views=\"{id}\"")));
    assert!(!page.contains("/api/articles/") && !page.contains("<script>alert"));
    let by = anonymous(&format!("/articles?by={writer}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(by.contains("小明 的文章") && by.contains("第一篇"));
    assert_eq!(
        status(anonymous("/articles?by=0").send().await.unwrap()),
        404
    );
    let published_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT published_at FROM articles WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .unwrap();

    // A read counts, the author's own doesn't.
    let views = |cookie: Option<&str>| {
        let request = http
            .post(format!("{base}/api/articles/{id}/views"))
            .header("origin", &origin);
        match cookie {
            Some(cookie) => request.header("cookie", cookie),
            None => request,
        }
    };
    let counted = |response: reqwest::Response| async move {
        response.json::<Value>().await.unwrap()["views"].as_i64()
    };
    assert_eq!(counted(views(None).send().await.unwrap()).await, Some(1));
    assert_eq!(
        counted(views(Some(&author)).send().await.unwrap()).await,
        Some(1)
    );
    assert_eq!(
        counted(views(Some(&reader)).send().await.unwrap()).await,
        Some(2)
    );

    // An edit takes it back to review; a new ❤️ keeps its first publication time. A long body is
    // well past the 64 KiB other requests may carry.
    let long = format!("{body}\n\n{}", "长".repeat(BODY_CHARS - 200));
    let saved: Value = save(&author, 2, &long)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (saved["status"].as_str(), saved["revision"].as_i64()),
        (Some("pending"), Some(3))
    );
    assert_eq!(
        status(anonymous(&format!("/articles/{id}")).send().await.unwrap()),
        404
    );
    assert!(!listed().await.contains(&format!("/articles/{id}</loc>")));
    assert_eq!(status(views(None).send().await.unwrap()), 404);
    assert_eq!(status(heart(&admin, true, 3).send().await.unwrap()), 200);
    let again: DateTime<Utc> =
        sqlx::query_scalar("SELECT published_at FROM articles WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(again, published_at);

    // A disabled account's articles leave every public page.
    sqlx::query("UPDATE users SET status = false WHERE id = $1")
        .bind(writer)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        status(anonymous(&format!("/articles/{id}")).send().await.unwrap()),
        404
    );
    let list = anonymous("/articles")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!list.contains(&format!("/articles/{id}")) && list.contains("来写第一篇"));
    assert_eq!(status(views(None).send().await.unwrap()), 404);
    let response = get(&format!("/api/articles/{id}/images/{image}"), &author)
        .header("if-none-match", &tag)
        .send()
        .await
        .unwrap();
    assert_eq!(status(response), 403);
    sqlx::query("UPDATE users SET status = true WHERE id = $1")
        .bind(writer)
        .execute(&state.pool)
        .await
        .unwrap();

    // Ten articles and 64 pictures a day; deleting gives none back. Two are written already.
    for _ in 2..DAILY_ARTICLES {
        let response = send(reqwest::Method::POST, "/api/articles", &author)
            .json(&first)
            .send()
            .await
            .unwrap();
        assert_eq!(status(response), 201);
    }
    let response = send(reqwest::Method::POST, "/api/articles", &author)
        .json(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(status(response), 429);
    sqlx::query("UPDATE authors SET images = $2 WHERE user_id = $1")
        .bind(writer)
        .bind(DAILY_IMAGES)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        status(upload(&author, picture.clone()).send().await.unwrap()),
        429
    );
    assert_eq!(
        status(
            send(
                reqwest::Method::DELETE,
                &format!("/api/articles/{id}"),
                &author
            )
            .send()
            .await
            .unwrap()
        ),
        204
    );
    let left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM article_images WHERE article_id = $1::uuid")
            .bind(&id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(left, 0);
    let response = send(reqwest::Method::POST, "/api/articles", &author)
        .json(&first)
        .send()
        .await
        .unwrap();
    assert_eq!(status(response), 429);

    // The day only moves forward: a request that read the date before midnight counts toward
    // the new day, and the next day starts again.
    let today = state.digest.today();
    let (yesterday, tomorrow) = (today.pred_opt().unwrap(), today.succ_opt().unwrap());
    let counts = || async {
        sqlx::query_as::<_, (NaiveDate, i16, i16)>(
            "SELECT day, articles, images FROM authors WHERE user_id = $1",
        )
        .bind(writer)
        .fetch_one(&state.pool)
        .await
        .unwrap()
    };
    sqlx::query("UPDATE authors SET articles = 3, images = 5 WHERE user_id = $1")
        .bind(writer)
        .execute(&state.pool)
        .await
        .unwrap();
    take(&state.pool, writer, yesterday, Quota::Articles)
        .await
        .unwrap();
    assert_eq!(counts().await, (today, 4, 5));
    take(&state.pool, writer, yesterday, Quota::Images)
        .await
        .unwrap();
    assert_eq!(counts().await, (today, 4, 6));
    take(&state.pool, writer, tomorrow, Quota::Images)
        .await
        .unwrap();
    assert_eq!(counts().await, (tomorrow, 0, 1));
    // A picture tmpfile refuses is given back to the day it was counted on, here the newer one.
    refuse.store(true, Ordering::SeqCst);
    let response = send(
        reqwest::Method::POST,
        &format!("/api/articles/{}/images", other["id"].as_str().unwrap()),
        &author,
    )
    .body(picture.clone())
    .send()
    .await
    .unwrap();
    assert_eq!(status(response), 503);
    refuse.store(false, Ordering::SeqCst);
    assert_eq!(counts().await, (tomorrow, 0, 1));
}
