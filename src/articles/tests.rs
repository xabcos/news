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
            "## 开头\n\nHello **world**, `code` and [a link](https://example.com).\n\n![一只猫](image:{id})\n\n- **一起玩。**把\n- two"
        )),
        "开头 Hello world, code and a link. 一起玩。把 two"
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
    // The workspace finds it again by the day it went out, by words of its title, or by both.
    let today = Utc::now()
        .with_timezone(&state.config.timezone)
        .date_naive();
    for (find, found) in [
        (format!("day={today}"), 1),
        ("q=第一".to_owned(), 1),
        (format!("day={}&q=第一", today.pred_opt().unwrap()), 0),
        ("q=第一 没有".to_owned(), 0),
    ] {
        let queue: Value = get(&format!("/api/admin/articles?{find}"), &admin)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            queue["published"].as_array().unwrap().len(),
            found,
            "{find}"
        );
        if found > 0 {
            assert_eq!(queue["published"][0]["film"], false);
        }
    }

    // Readers see it under the pen name, with its tmpfile pictures, and never the email.
    let list = anonymous("/articles")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(list.contains("第一篇") && list.contains("小明") && list.contains("Hello world"));
    // Its count has a place on the card even before anyone reads it, for the script to fill.
    assert!(list.contains(&format!("<span data-views=\"{id}\" hidden> · ")));
    assert!(!list.contains("articles.example.test"));
    // Its tab follows 开源's on every page, and the sitemap lists it.
    assert!(list.contains("<a class=tab href=\"/articles\" aria-current=\"true\">分享</a>"));
    let reads = anonymous("/reads")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(reads.contains("开源</a><a class=tab href=\"/articles\">分享</a>"));
    assert!(reads.contains("<a class=tab href=\"/reads\" aria-current=\"true\">精读</a>"));
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
    // Pages that came from a cache read the counts as they are now: public articles only.
    let response = anonymous(&format!(
        "/api/articles/views?ids={id},nonsense,{}",
        Uuid::new_v4()
    ))
    .send()
    .await
    .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let fresh: Value = response.json().await.unwrap();
    assert_eq!(fresh["views"][id.as_str()], 2);
    assert_eq!(fresh["views"].as_object().unwrap().len(), 1);

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

    // An administrator edits anyone's article, XABC's above all, and its status stays: a pending
    // one still waits for the ❤️, a public one stays public. A reader opens no one else's. The
    // administrator's previews lead to the editor and copy the article for X Articles.
    assert_eq!(
        status(
            get(&format!("/api/articles/{id}"), &reader)
                .send()
                .await
                .unwrap()
        ),
        404
    );
    assert_eq!(status(heart(&admin, false, 3).send().await.unwrap()), 200);
    let opened: Value = get(&format!("/api/articles/{id}"), &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(opened["revision"], 3);
    let edit =
        |revision: i64, title: &str| {
            send(reqwest::Method::PATCH, &format!("/api/articles/{id}"), &admin).json(&json!({
            "revision": revision, "title": title, "body": opened["body"], "cover": opened["cover"],
        }))
        };
    let edited: Value = edit(3, "第一篇（改）")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (edited["status"].as_str(), edited["revision"].as_i64()),
        (Some("pending"), Some(4))
    );
    assert_eq!(status(heart(&admin, true, 4).send().await.unwrap()), 200);
    let edited: Value = edit(4, "第一篇（再改）")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (edited["status"].as_str(), edited["revision"].as_i64()),
        (Some("published"), Some(5))
    );
    assert_eq!(
        status(
            get(&format!("/api/articles/{id}/preview"), &admin)
                .send()
                .await
                .unwrap()
        ),
        200
    );
    let reviewed = get(&format!("/api/admin/preview/articles/{id}"), &admin)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(reviewed.contains(&format!("href=\"/me/write.html?id={id}\"")));
    assert!(
        reviewed.contains("data-article-copy=title") && reviewed.contains("data-article-copy=body")
    );
    assert!(reviewed.contains("第一篇（再改）"));

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

/// XABC's two lines from start to finish against stand-ins for the AI service and tmpfile: a 分享
/// article's searches, writing, drawn pictures, publication and pages; then 解说 explaining a
/// work 影视 discusses and then another, revising the first, and listing them under Culture.
#[tokio::test]
#[ignore = "requires NEWS_XABC_TEST_DATABASE_URL and local test configuration"]
async fn postgres_xabc_shares_and_explains_films() {
    use crate::config::Config;
    use axum::{http::StatusCode, routing::post};
    use base64::Engine;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU8},
    };
    let database_url =
        std::env::var("NEWS_XABC_TEST_DATABASE_URL").expect("disposable database URL");
    let asked: Arc<Mutex<Vec<Value>>> = Arc::default();
    let drawings: Arc<Mutex<Vec<Value>>> = Arc::default();
    // 0: both searches answer; 1: grok's fails; 2: both fail.
    let failing = Arc::new(AtomicU8::new(1));
    // Whether the image model fails to draw covers.
    let covers_fail = Arc::new(AtomicBool::new(false));
    let mut seed = 0x9e37_79b9_u32;
    let noise = image::RgbImage::from_fn(640, 400, |_, _| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let [r, g, b, _] = seed.to_le_bytes();
        image::Rgb([r, g, b])
    });
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(noise)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let drawn = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    let reply = |text: Value, cited: &str| {
        json!({
            "id": "resp_test", "status": "completed",
            "output": [{ "type": "message", "content": [{
                "type": "output_text", "text": text.to_string(),
                "annotations": [{ "type": "url_citation", "url": cited }],
            }] }],
            "usage": { "input_tokens": 100, "output_tokens": 50 },
        })
    };
    let body = format!(
        "{}\n\n可以去试试[这个工具](https://www.known.example/tool/)，看看[别处](https://elsewhere.example/x)和[引用](https://cited.example/writer)。([known.example](https://known.example/tool))\n\n![](img:1)\n\n![](img:2)\n\n![](img:3)\n\n最后一段。",
        "有趣的东西，".repeat(80)
    );
    let ai = axum::Router::new()
        .route(
            "/v1/responses",
            post({
                let (asked, failing) = (asked.clone(), failing.clone());
                move |Json(request): Json<Value>| {
                    let (asked, failing, body) = (asked.clone(), failing.clone(), body.clone());
                    async move {
                        asked.lock().unwrap().push(request.clone());
                        let instructions = request["instructions"].as_str().unwrap_or_default();
                        let film = instructions.contains("文娱 · 解说");
                        if request["text"]["format"]["name"] == "finds" && film {
                            return (
                                StatusCode::OK,
                                Json(reply(
                                    json!({ "finds": [{
                                        "title": "结局解析", "why": "讲透了", "points": "结局是这样的",
                                        "sources": ["https://film.example/review"], "pictures": [],
                                    }] }),
                                    "https://cited.example/film",
                                )),
                            );
                        }
                        if request["text"]["format"]["name"] == "article" && film {
                            let revising = instructions.contains("这次是修订");
                            let body = if revising {
                                format!("{}\n\n![](keep:1)\n\n![](img:1)\n\n结局终于清楚了。", "修订后的解说，".repeat(80))
                            } else {
                                format!("{}\n\n![](img:1)\n\n看[影评](https://film.example/review)。", "完整的剧情，".repeat(80))
                            };
                            return (
                                StatusCode::OK,
                                Json(reply(
                                    json!({
                                        "title": if revising { "修订后的解说" } else { "一部片的解说" },
                                        "body": body,
                                        "pictures": [{ "page": "", "prompt": "一幅插图", "caption": if revising { "新图" } else { "旧图" } }],
                                        "cover": "封面插画",
                                    }),
                                    "https://cited.example/film",
                                )),
                            );
                        }
                        if request["text"]["format"]["name"] == "finds" {
                            let grok = request["tools"].as_array().unwrap().len() == 2;
                            match failing.load(Ordering::SeqCst) {
                                2 => return (StatusCode::BAD_REQUEST, Json(json!({}))),
                                1 if grok => return (StatusCode::BAD_REQUEST, Json(json!({}))),
                                _ => {}
                            }
                            return (
                                StatusCode::OK,
                                Json(reply(
                                    json!({ "finds": [{
                                        "title": "一个好玩的工具", "why": "好玩", "points": "能做很多事",
                                        "sources": ["https://known.example/tool"], "pictures": [],
                                    }] }),
                                    "https://cited.example/search",
                                )),
                            );
                        }
                        (
                            StatusCode::OK,
                            Json(reply(
                                json!({
                                    "title": "今天分享一个好玩的工具", "body": body,
                                    "pictures": [
                                        { "page": "", "prompt": "一只猫", "caption": "一只猫" },
                                        { "page": "", "prompt": "", "caption": "不会来" },
                                        // A page whose picture can't be had leaves none, though
                                        // a prompt is there: the caption was written for it.
                                        { "page": "https://unreachable.invalid/post", "prompt": "一只狗", "caption": "出自那个页面的图" },
                                    ],
                                    "cover": "封面插画",
                                }),
                                "https://cited.example/writer",
                            )),
                        )
                    }
                }
            }),
        )
        .route(
            "/v1/images/generations",
            post({
                let (drawings, covers_fail) = (drawings.clone(), covers_fail.clone());
                move |Json(request): Json<Value>| {
                    let (drawings, drawn, covers_fail) =
                        (drawings.clone(), drawn.clone(), covers_fail.clone());
                    async move {
                        let cover = request["prompt"]
                            .as_str()
                            .is_some_and(|prompt| prompt.contains("封面插画"));
                        drawings.lock().unwrap().push(request);
                        if cover && covers_fail.load(Ordering::SeqCst) {
                            return (StatusCode::BAD_GATEWAY, Json(json!({})));
                        }
                        (
                            StatusCode::OK,
                            Json(json!({ "data": [{ "b64_json": drawn }] })),
                        )
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ai_base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, ai).await.unwrap() });
    let mut config = Config::from_env().unwrap();
    config.database_url = database_url;
    config.redis_addr = None;
    config.scheduler_enabled = false;
    config.ai_base_url = ai_base.parse().unwrap();
    config.ai_api_key = "fixture".into();
    config.ai_writing_model = "writer-fixture".into();
    let (media, client, _) = tmpfile(jpeg(400, 300)).await;
    config.tmpfile_base_url = media.parse().unwrap();
    config.tmpfile_api_key = "fixture".into();
    let mut state = AppState::new(config).await.unwrap();
    state.http = client;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_xabc_test"));
    sqlx::query("TRUNCATE articles, article_images CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    // The migration made XABC's account, which takes the pen name before any reader can.
    let author = xabc::author(&state).await.unwrap();
    let since = Utc::now() - chrono::Duration::minutes(1);
    let id = xabc::write(&state, xabc::Line::Share, since).await.unwrap();

    let (title, body, cover, status, name): (String, String, Option<Uuid>, String, String) =
        sqlx::query_as(
            "SELECT a.title, a.body, a.cover, a.status, w.name FROM articles a
             JOIN authors w ON w.user_id = a.author_id WHERE a.id = $1 AND a.author_id = $2",
        )
        .bind(id)
        .bind(author)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        (title.as_str(), status.as_str(), name.as_str()),
        ("今天分享一个好玩的工具", "published", "XABC")
    );
    // A link keeps its address when a model found it, by any of its forms; the others keep
    // their words, and the source mark after the sentence goes.
    assert!(body.contains(
        "试试[这个工具](https://www.known.example/tool/)，看看别处和[引用](https://cited.example/writer)。\n"
    ));
    let pictures: Vec<(Uuid, String, i32)> =
        sqlx::query_as("SELECT id, content_type, width FROM article_images WHERE article_id = $1")
            .bind(id)
            .fetch_all(&state.pool)
            .await
            .unwrap();
    // The drawn picture and the cover, both kept as JPEG at their size; the picture that never
    // came is gone.
    assert_eq!(pictures.len(), 2);
    assert!(
        pictures
            .iter()
            .all(|(_, kind, width)| kind == "image/jpeg" && *width == 640)
    );
    let cover = cover.expect("a cover");
    let shown = pictures
        .iter()
        .find(|(picture, ..)| *picture != cover)
        .unwrap()
        .0;
    assert!(body.contains(&format!("![一只猫](image:{shown})\n*一只猫（AI 生成）*")));
    assert!(!body.contains("img:") && !body.contains("出自那个页面的图"));

    // Grok's search failed, so the writing model wrote from the briefing model's finds alone.
    let asked_log = asked.clone();
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 3);
    let searched = |model: &str| {
        asked.iter().find(|request| {
            request["model"] == model && request["text"]["format"]["name"] == "finds"
        })
    };
    let x = searched(&state.config.ai_default_model).expect("grok searched");
    assert_eq!(
        x["tools"],
        json!([{ "type": "web_search" }, { "type": "x_search" }])
    );
    let web = searched(&state.config.ai_insight_model).expect("the briefing model searched");
    assert_eq!(web["tools"], json!([{ "type": "web_search" }]));
    let writing = asked
        .iter()
        .find(|request| request["text"]["format"]["name"] == "article")
        .unwrap();
    assert_eq!(writing["model"], state.config.ai_writing_model.as_str());
    let material = writing["input"][0]["content"].as_str().unwrap();
    assert!(material.contains("1.【全网】一个好玩的工具"));
    assert!(!material.contains("【X 与全网】"));
    let drawings_log = drawings.clone();
    let drawings = drawings.lock().unwrap().clone();
    assert_eq!(drawings.len(), 2);
    assert!(
        drawings
            .iter()
            .all(|drawing| drawing["model"] == state.config.ai_image_model.as_str())
    );

    // The last article tells when the next is due. Without finds nothing is written, and finds
    // about a page an article of the last week linked to are left out.
    assert!(
        xabc::last(&state, xabc::Line::Share)
            .await
            .unwrap()
            .is_some_and(|at| at >= since)
    );
    failing.store(2, Ordering::SeqCst);
    assert!(matches!(
        xabc::write(&state, xabc::Line::Share, since).await,
        Err(AppError::Internal(message)) if message.contains("没有找到线索")
    ));
    failing.store(0, Ordering::SeqCst);
    assert!(matches!(
        xabc::write(&state, xabc::Line::Share, since).await,
        Err(AppError::Internal(message)) if message.contains("最近写过")
    ));
    // Two writers finishing together publish once.
    let later = Utc::now();
    let body = "字".repeat(500);
    let article = |title: &str| xabc::Finished {
        title: title.into(),
        body: body.clone(),
        pictures: Vec::new(),
        cover: None,
    };
    let (one, other) = (article("甲"), article("乙"));
    let (first, second) = tokio::join!(
        xabc::publish(&state, author, later, None, &one),
        xabc::publish(&state, author, later, None, &other),
    );
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .iter()
            .filter(|ok| **ok)
            .count(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM articles")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        2
    );

    // 热读 ranks reads fading with age; articles nobody read stay out, and it has one page.
    let race = if first.is_ok() { "甲" } else { "乙" };
    sqlx::query("UPDATE articles SET views = CASE WHEN id = $1 THEN 3 ELSE 9 END")
        .bind(id)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE articles SET published_at = NOW() - INTERVAL '10 days' WHERE title = $1")
        .bind(race)
        .execute(&state.pool)
        .await
        .unwrap();

    // Readers find it under XABC, with its cover and the picture's caption line.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::web::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let page = http
        .get(format!("{base}/articles/{id}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("class=article-cover"));
    assert!(page.contains("<em>一只猫（AI 生成）</em>"));
    assert!(page.contains(&format!("/articles?by={author}\">XABC</a>")));
    let list = http
        .get(format!("{base}/articles?by={author}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(list.contains("今天分享一个好玩的工具"));
    // 分享 tiles picture cards across the window as 解说 does, each with its cover and author;
    // its articles keep the reading column.
    assert!(list.contains("<body class=\"public reads articles article-cards\">"));
    assert!(list.contains(&format!(
        "<article class=\"item image-news-card catalog-card\"><a class=image-news-media href=\"/articles/{id}\""
    )));
    assert!(list.contains(&format!("/articles?by={author}\">XABC</a> · <time")));
    assert!(page.contains("<body class=\"public reads articles\">"));
    // ln(1 + 3) beats ln(1 + 9) halved by ten days of age.
    let hot = http
        .get(format!("{base}/articles?view=hot"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let first_place = hot
        .find("<span class=read-rank aria-hidden=true>01</span>")
        .unwrap();
    let second_place = hot
        .find("<span class=read-rank aria-hidden=true>02</span>")
        .unwrap();
    let mine = hot.find("今天分享一个好玩的工具").unwrap();
    assert!(first_place < mine && mine < second_place && hot.contains(race));
    assert!(hot.contains("<a href=\"/articles?view=hot\" aria-current=\"page\">热读</a>"));
    assert_eq!(
        http.get(format!("{base}/articles?view=hot&page=2"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // 文娱 · 解说: a work 影视 has reviews of comes first, with what 影视's guide checked.
    sqlx::query("TRUNCATE catalog_entries, catalog_items")
        .execute(&state.pool)
        .await
        .unwrap();
    let entry = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO catalog_entries (id, topic_id, slug, name, name_en, creator, kind, key, source_urls, guide)
         VALUES ($1, 'movie', 'test-film', '测试片', 'Test Film', '导演甲', 'movie', 'movie:testfilm', '[]',
                 '{\"intro\": \"一部讲时间的片子。\", \"facts\": [{\"label\": \"片长\", \"text\": \"112 分钟\"}]}')",
    )
    .bind(entry)
    .execute(&state.pool)
    .await
    .unwrap();
    let story = |entry: Uuid, role: &'static str, age: &'static str| {
        let state = state.clone();
        async move {
            let item = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url, first_seen_at)
                 VALUES ($1, 'movie', CURRENT_DATE, sha256(convert_to($1::text, 'UTF8')), '一篇影评', '说结局', $2,
                         now() - $3::interval)",
            )
            .bind(item)
            .bind(format!("https://film.example/story/{item}"))
            .bind(age)
            .execute(&state.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO catalog_items (item_id, entry_id, role) VALUES ($1, $2, $3)")
                .bind(item)
                .bind(entry)
                .bind(role)
                .execute(&state.pool)
                .await
                .unwrap();
        }
    };
    story(entry, "review", "2 days").await;
    let explained = xabc::write(&state, xabc::Line::Film, Utc::now())
        .await
        .unwrap();
    let (film, title, body, cover, revision): (Option<String>, String, String, Option<Uuid>, i32) =
        sqlx::query_as("SELECT film, title, body, cover, revision FROM articles WHERE id = $1")
            .bind(explained)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (film.as_deref(), title.as_str()),
        (Some("movie:testfilm"), "一部片的解说")
    );
    let figure = body
        .lines()
        .skip_while(|line| !line.starts_with("![旧图](image:"))
        .take(2)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(figure.ends_with("*旧图（AI 生成）*"), "{body}");
    assert!(body.contains("[影评](https://film.example/review)"));
    let writing = |asked: &[Value]| {
        let request = asked
            .iter()
            .rev()
            .find(|request| request["text"]["format"]["name"] == "article")
            .unwrap();
        assert_eq!(request["model"], state.config.ai_writing_model.as_str());
        request["input"][0]["content"].as_str().unwrap().to_owned()
    };
    let material = writing(&asked_log.lock().unwrap());
    assert!(material.contains("作品：《测试片》（Test Film，电影，主创 导演甲）"));
    assert!(material.contains("影视栏目核实过的资料：一部讲时间的片子。\n- 片长：112 分钟"));
    assert!(
        xabc::last(&state, xabc::Line::Film)
            .await
            .unwrap()
            .is_some()
    );

    // The next is another work 影视 has a review of; then there is nothing to write.
    let work = |key: &'static str, name: &'static str| {
        let state = state.clone();
        async move {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO catalog_entries (id, topic_id, slug, name, name_en, creator, kind, key, source_urls)
                 VALUES ($1, 'movie', $2, $3, $3, '导演乙', 'movie', 'movie:' || $2, '[]')",
            )
            .bind(id)
            .bind(key)
            .bind(name)
            .execute(&state.pool)
            .await
            .unwrap();
            id
        }
    };
    // Its cover can't be drawn, so the picture the body shows stands for it.
    let other = work("otherfilm", "Other Film").await;
    story(other, "review", "1 day").await;
    covers_fail.store(true, Ordering::SeqCst);
    let second = xabc::write(&state, xabc::Line::Film, Utc::now())
        .await
        .unwrap();
    covers_fail.store(false, Ordering::SeqCst);
    let (film, body, standing): (Option<String>, String, Option<Uuid>) =
        sqlx::query_as("SELECT film, body, cover FROM articles WHERE id = $1")
            .bind(second)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(film.as_deref(), Some("movie:otherfilm"));
    let standing = standing.expect("the body's picture as the cover");
    assert!(body.contains(&format!("](image:{standing})")), "{body}");
    assert!(matches!(
        xabc::write(&state, xabc::Line::Film, Utc::now()).await,
        Err(AppError::Internal(message)) if message == "影视最近没有待解说的作品"
    ));

    // Three days after its article, four new stories about the work (the review from before it
    // counts) are not enough; a fifth makes the article due.
    sqlx::query("UPDATE articles SET updated_at = now() - INTERVAL '4 days' WHERE id = $1")
        .bind(explained)
        .execute(&state.pool)
        .await
        .unwrap();
    for _ in 0..3 {
        story(entry, "news", "1 day").await;
    }
    assert!(matches!(
        xabc::write(&state, xabc::Line::Film, Utc::now()).await,
        Err(AppError::Internal(message)) if message == "影视最近没有待解说的作品"
    ));
    story(entry, "news", "1 day").await;

    // The most new stories go first, written about or not: a work with seven before the article
    // with five, and that before a work with two reviews. Within four hours of the last 解说,
    // the scheduler's cutoff refuses another.
    let louder = work("louderfilm", "Louder Film").await;
    story(louder, "review", "1 day").await;
    for _ in 0..6 {
        story(louder, "news", "1 day").await;
    }
    let quieter = work("quietfilm", "Quiet Film").await;
    for _ in 0..2 {
        story(quieter, "review", "1 day").await;
    }
    assert!(matches!(
        xabc::write(&state, xabc::Line::Film, Utc::now() - chrono::Duration::hours(4)).await,
        Err(AppError::Conflict(message)) if message == "XABC 刚发过一篇文章"
    ));
    let loudest = xabc::write(&state, xabc::Line::Film, Utc::now())
        .await
        .unwrap();
    let film: Option<String> = sqlx::query_scalar("SELECT film FROM articles WHERE id = $1")
        .bind(loudest)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(film.as_deref(), Some("movie:louderfilm"));

    // The article is revised: its figure and cover stay, a new picture joins them, and the line's
    // clock moves on.
    let last = xabc::last(&state, xabc::Line::Film).await.unwrap().unwrap();
    let drawn_before = drawings_log.lock().unwrap().len();
    assert_eq!(
        xabc::write(&state, xabc::Line::Film, Utc::now())
            .await
            .unwrap(),
        explained
    );
    assert!(xabc::last(&state, xabc::Line::Film).await.unwrap().unwrap() > last);
    let material = writing(&asked_log.lock().unwrap());
    assert!(material.contains("\n\n![](keep:1)\n\n") && !material.contains("image:"));
    assert!(material.contains(
        "影视栏目在上次修订之后收到的内容：\n- 一篇影评：说结局（https://film.example/story/"
    ));
    let (title, revised, kept_cover, revised_at): (String, String, Option<Uuid>, i32) =
        sqlx::query_as("SELECT title, body, cover, revision FROM articles WHERE id = $1")
            .bind(explained)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (title.as_str(), kept_cover, revised_at),
        ("修订后的解说", cover, revision + 1)
    );
    assert!(
        revised.contains(&format!("{figure}\n\n![新图](image:")),
        "{revised}"
    );
    // No cover is drawn again.
    assert_eq!(drawings_log.lock().unwrap().len(), drawn_before + 1);

    // 解说 lists its articles as picture cards under Culture's tabs, and 分享 doesn't.
    let page = |path: String| {
        let http = http.clone();
        async move { http.get(path).send().await.unwrap().text().await.unwrap() }
    };
    let list = page(format!("{base}/culture/commentary")).await;
    assert!(list.contains("<a href=\"/culture/movie\">影视</a><a href=\"/culture/commentary\" aria-current=\"true\">解说</a>"));
    assert!(
        list.contains("class=\"item image-news-card catalog-card\"")
            && list.contains("修订后的解说")
    );
    assert!(
        !page(format!("{base}/articles"))
            .await
            .contains("修订后的解说")
    );
    // Each list searches its own articles, and search pages stay out of search engines.
    let found = http
        .get(format!("{base}/culture/commentary?q=修订后"))
        .send()
        .await
        .unwrap();
    assert_eq!(found.headers()["x-robots-tag"], "noindex");
    let found = found.text().await.unwrap();
    assert!(found.contains("「修订后」找到 1 篇") && found.contains("修订后的解说"));
    assert!(found.contains("name=q value=\"修订后\""), "{found}");
    let shared = page(format!("{base}/articles?q=修订后")).await;
    assert!(shared.contains("「修订后」找到 0 篇") && shared.contains("没有找到相关文章"));
    assert!(
        page(format!("{base}/culture/commentary?q=没有这部"))
            .await
            .contains("没有找到相关解说")
    );
    let article = page(format!("{base}/articles/{explained}")).await;
    assert!(article.contains("<a href=\"/culture/commentary\">← 解说</a>"));
    assert!(article.contains(
        "<a class=tab href=\"/culture\" data-last=\"/culture/book\" aria-current=\"true\">文娱</a>"
    ));

    // 值得读 opens with XABC's newest 分享, newest first, however well a 解说 is read; the rest
    // rank 分享 and 解说 together as 热读 does, unread ones following newest first.
    sqlx::query("UPDATE articles SET views = 100 WHERE id = $1")
        .bind(explained)
        .execute(&state.pool)
        .await
        .unwrap();
    let worth: Value =
        serde_json::from_str(&page(format!("{base}/articles/worth.json")).await).unwrap();
    let ranked: Vec<(String, bool)> = worth["articles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|article| {
            (
                article["id"].as_str().unwrap().to_owned(),
                article["film"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(ranked.len(), 5);
    assert_eq!(ranked[0], (id.to_string(), false));
    assert!(!ranked[1].1);
    assert_eq!(
        ranked[2..],
        [
            (explained.to_string(), true),
            (loudest.to_string(), true),
            (second.to_string(), true)
        ]
    );
}
