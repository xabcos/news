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
    // A caption under a picture is not the body's words; italics elsewhere are.
    assert_eq!(
        excerpt(&format!(
            "![甲](image:{id})\n*甲的 _旧_ 照片（图：[example.com](<https://example.com/a>)）*\n\n*开头*的话。"
        )),
        "开头的话。"
    );
    // A formula on one line reads as what it shows; one with scripts or fractions, as its TeX.
    assert_eq!(
        excerpt(
            "式子：\n\n\\[\n\\mathbf P=m(\\mathbf C-\\mathbf V)\n\\]\n\n当 \\(\\cos\\theta<1\\)，\\(m_0\\) 与 \\(\\frac{1}{2}\\)"
        ),
        "式子： 𝐏=m(𝐂−𝐕) 当 cos θ<1，m_0 与 \\frac{1}{2}"
    );
    // Set apart or within a line, as the page lays it out.
    assert_eq!(
        excerpt("\\[\\mathchoice{A}{B}{C}{D}\\]\n\n\\(\\mathchoice{A}{B}{C}{D}\\)"),
        "A B"
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
    let ids: Vec<Uuid> = (0..=ARTICLE_IMAGES as u128).map(Uuid::from_u128).collect();
    let body = |count: usize| {
        ids[..count]
            .iter()
            .map(|id| format!("![](image:{id}) ![again](image:{id})"))
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let all = ARTICLE_IMAGES;
    let shown = checked("Title", &body(all), Some(ids[0])).unwrap();
    assert_eq!(pictures(&shown.body).len(), all);
    assert!(checked("Title", &body(all), Some(ids[all])).is_err());
    assert!(checked("Title", &body(all - 1), Some(ids[all])).is_ok());
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
/// While the returned switch is on, it refuses uploads. The same server plays English Wikipedia,
/// whose every entry but Satya Nadella's leads with a photograph; X, where every account but
/// `NoFace` shares a profile picture, 300 pixels square; and news pages under `/news/`, each
/// sharing the scene photograph as its picture, a file large enough to show.
async fn tmpfile(picture: Vec<u8>) -> (String, reqwest::Client, Arc<AtomicBool>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, rustls};
    let cert = rcgen::generate_simple_self_signed(vec![
        "media.example".into(),
        "en.wikipedia.org".into(),
        "upload.wikimedia.org".into(),
        "x.com".into(),
        "pbs.twimg.com".into(),
        "abs.twimg.com".into(),
    ])
    .unwrap();
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
                let host = String::from_utf8_lossy(&connect)
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_owned();
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
                } else if let Some(title) = path.strip_prefix("/wiki/") {
                    let photograph = if title == "Satya_Nadella" {
                        String::new()
                    } else {
                        format!(
                            "<meta property=\"og:image\" content=\"https://upload.wikimedia.org/wikipedia/commons/a/ab/{title}.jpg\">"
                        )
                    };
                    let page = format!("<html><head>{photograph}</head></html>");
                    ("text/html", page.into_bytes())
                } else if host == "x.com:443" {
                    let account = path.trim_start_matches('/');
                    let picture = if account == "NoFace" {
                        "https://abs.twimg.com/sticky/default_profile_images/default_profile_400x400.png".to_owned()
                    } else {
                        format!("https://pbs.twimg.com/profile_images/1/{account}_400x400.jpg")
                    };
                    let page = format!(
                        "<html><head><meta property=\"og:image\" content=\"{picture}\"></head></html>"
                    );
                    ("text/html", page.into_bytes())
                } else if path.starts_with("/profile_images/") {
                    ("image/jpeg", jpeg(300, 300))
                } else if path == "/files/scene.jpg" {
                    ("image/jpeg", jpeg(640, 400))
                } else if path.starts_with("/news/") {
                    let page = "<html><head><meta property=\"og:image\" content=\"https://media.example/files/scene.jpg\"></head></html>";
                    ("text/html", page.as_bytes().to_vec())
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
    assert_eq!(created["images"], json!({}));
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
    let url = image["url"].as_str().unwrap().to_owned();
    assert!(url.starts_with("https://media.example/"));
    let image = image["id"].as_str().unwrap().to_owned();
    // The editor opens knowing where each picture is stored, for text copied out of it.
    let opened: Value = get(&format!("/api/articles/{id}"), &author)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(opened["images"], json!({ image.as_str(): url }));
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

    // The writing page renders what it holds now, unsaved, as the article page will: the author's
    // own pictures through this server, not another article's, nothing that runs. An administrator
    // may ask, another reader can't.
    let render = |cookie: &str, body: &str| {
        send(
            reqwest::Method::POST,
            &format!("/api/articles/{id}/render"),
            cookie,
        )
        .json(&json!({ "body": body }))
    };
    let unsaved = format!(
        "## 还没保存\n\n**粗体**\n\n![图](image:{image})\n\n![别人的](image:{})\n\n<script>alert(1)</script>",
        elsewhere["id"].as_str().unwrap()
    );
    let rendered: Value = render(&author, &unsaved)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let html = rendered["html"].as_str().unwrap();
    assert!(html.contains("<h2>还没保存</h2>") && html.contains("<strong>粗体</strong>"));
    assert!(html.contains(&format!("src=\"/api/articles/{id}/images/{image}\"")));
    assert_eq!(html.matches("<img").count(), 1);
    assert!(!html.contains("<script>"));
    // The editor knows only the article's own pictures, for the administrator as for the author.
    for cookie in [&author, &admin] {
        let opened: Value = get(&format!("/api/articles/{id}"), cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(opened["images"], json!({ image.as_str(): url }));
    }
    assert_eq!(status(render(&admin, &unsaved).send().await.unwrap()), 200);
    // A long article renders as it saves, up to the same size.
    assert_eq!(
        status(render(&author, &"长".repeat(100_000)).send().await.unwrap()),
        200
    );
    assert_eq!(
        status(render(&author, &"长".repeat(800_000)).send().await.unwrap()),
        413
    );
    assert_eq!(status(render(&reader, &unsaved).send().await.unwrap()), 404);

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

    // Another reader can't delete an article; an administrator deletes anyone's.
    let path = format!("/api/articles/{}", other["id"].as_str().unwrap());
    for (cookie, expected) in [(&reader, 404), (&admin, 204), (&admin, 404)] {
        let response = send(reqwest::Method::DELETE, &path, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(status(response), expected);
    }
}

/// XABC's lines from start to finish against stand-ins for the AI service and tmpfile: a 分享
/// article's searches, writing, drawn pictures, publication and pages; then 解说 explaining a
/// work 影视 discusses and then another, revising the first, and listing them under Culture;
/// then 时代's profiles and 追踪's events, written, brought up to date and listed.
#[tokio::test]
#[ignore = "requires NEWS_XABC_TEST_DATABASE_URL and local test configuration"]
async fn postgres_xabc_shares_explains_profiles_and_tracks() {
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
    // Whether 追踪's writer finds anything new for the article it brings up to date.
    let news = Arc::new(AtomicBool::new(true));
    // Whether grok finds a photograph of a person or a company for 时代.
    let sighting = Arc::new(AtomicBool::new(true));
    // Whether an administrator saves the Binance article while 追踪's scouts search.
    let saving = Arc::new(AtomicBool::new(false));
    let admin = sqlx::PgPool::connect_lazy(&database_url).unwrap();
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
                let (asked, failing, news, saving, sighting) = (
                    asked.clone(),
                    failing.clone(),
                    news.clone(),
                    saving.clone(),
                    sighting.clone(),
                );
                move |Json(request): Json<Value>| {
                    let (asked, failing, news, saving, sighting, admin, body) = (
                        asked.clone(),
                        failing.clone(),
                        news.clone(),
                        saving.clone(),
                        sighting.clone(),
                        admin.clone(),
                        body.clone(),
                    );
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
                        // 时代: in a round of people talked about now, grok names two people and
                        // the briefing model one of them; a round of leaders names Ren Zhengfei,
                        // a round of companies Huawei, and a round an administrator asks for
                        // whom they name, unless nobody can be found. Huawei is a company. The
                        // scouts give the Wikipedia entries and the X accounts of Jensen Huang and
                        // Satya Nadella and Ren Zhengfei's account, each met in their sources; Lisa
                        // Su's entry and account they give without having met them.
                        let people = instructions.contains("「时代」");
                        if request["text"]["format"]["name"] == "finds" && people {
                            let input = request["input"][0]["content"].as_str().unwrap_or_default();
                            let grok = request["tools"].as_array().unwrap().len() == 2;
                            let named = if let Some((_, name)) = input.split_once("点名：") {
                                if name == "查无此人" { Vec::new() } else { vec![name] }
                            } else if input.contains("有分量的人物") {
                                vec!["Lisa Su", if grok { "Ren Zhengfei" } else { "ren zhengfei" }]
                            } else if input.contains("举足轻重的企业") {
                                vec![if grok { "Huawei" } else { "huawei" }]
                            } else if grok {
                                vec!["Jensen Huang", "Lisa Su"]
                            } else {
                                vec!["lisa su"]
                            };
                            let finds: Vec<Value> = named
                                .into_iter()
                                .map(|person| {
                                    let (wiki, x, met): (&str, &str, &[&str]) = match person.to_lowercase().as_str() {
                                        "jensen huang" => ("https://en.wikipedia.org/wiki/Jensen_Huang", "JensenHuang", &["https://en.wikipedia.org/wiki/Jensen_Huang", "https://x.com/JensenHuang/status/4"]),
                                        "lisa su" => ("https://en.wikipedia.org/wiki/Lisa_Su", "LisaSu", &[]),
                                        "ren zhengfei" => ("", "https://x.com/NoFace", &["https://x.com/NoFace/status/2"]),
                                        "satya nadella" => ("https://en.wikipedia.org/wiki/Satya_Nadella", "@satyanadella", &["https://en.wikipedia.org/wiki/Satya_Nadella", "https://x.com/satyanadella/status/1"]),
                                        _ => ("", "", &[]),
                                    };
                                    let sources: Vec<&str> = ["https://people.example/news"].iter().chain(met).copied().collect();
                                    json!({
                                        "title": format!("{person} 的近况"), "why": "大家都在谈",
                                        "points": "发布了新芯片", "sources": sources,
                                        "pictures": [], "name": person,
                                        "company": person.eq_ignore_ascii_case("huawei"),
                                        "wiki": wiki, "x": x,
                                    })
                                })
                                .collect();
                            return (
                                StatusCode::OK,
                                Json(reply(json!({ "finds": finds }), "https://cited.example/people")),
                            );
                        }
                        if request["text"]["format"]["name"] == "article" && people {
                            let material = request["input"][0]["content"].as_str().unwrap_or_default();
                            let name = material
                                .lines()
                                .find_map(|line| line.strip_prefix("人物：").or_else(|| line.strip_prefix("企业：")))
                                .unwrap_or_default();
                            let kind = if instructions.contains("栏目写企业") { "企业" } else { "人物" };
                            // The name goes to the account the material gives.
                            let home = material
                                .lines()
                                .find_map(|line| line.strip_prefix("X："))
                                .map(|home| format!("[{name}]({home}) "))
                                .unwrap_or_default();
                            let revising = instructions.contains("这次是更新");
                            let (title, body) = if revising {
                                // The writer keeps the first figure and the comic the material
                                // names, and asks for a comic all the same.
                                let comic = material
                                    .split_once("其中 ")
                                    .and_then(|(_, rest)| rest.split_once(" 是四格漫画"))
                                    .map(|(comic, _)| comic)
                                    .filter(|comic| *comic != "![](keep:1)")
                                    .map(|comic| format!("{comic}\n\n"))
                                    .unwrap_or_default();
                                (
                                    format!("{name} 的{kind}介绍（更新）"),
                                    format!("{}\n\n![](keep:1)\n\n{comic}![](img:1)\n\n近况更新。", "更新后的人物，".repeat(80)),
                                )
                            } else {
                                (
                                    format!("{name} 的{kind}介绍"),
                                    format!("{home}{}\n\n![](img:1)\n\n![](img:2)\n\n看[报道](https://people.example/news)。", "一个人的经历，".repeat(80)),
                                )
                            };
                            return (
                                StatusCode::OK,
                                Json(reply(
                                    json!({
                                        "title": title, "body": body,
                                        "pictures": [
                                            { "page": "", "prompt": "四格漫画：他的一生", "caption": "四个时刻" },
                                            { "page": "", "prompt": "一幅插图", "caption": "插图" },
                                        ],
                                        "cover": "封面插画",
                                    }),
                                    "https://cited.example/people",
                                )),
                            );
                        }
                        // 评测: both scouts find a review; the writer places 数码's official
                        // photograph and a review's.
                        let review = instructions.contains("「数码 · 评测」");
                        if request["text"]["format"]["name"] == "finds" && review {
                            return (
                                StatusCode::OK,
                                Json(reply(
                                    json!({ "finds": [{
                                        "title": "一篇上手评测", "why": "测得细", "points": "续航一天半",
                                        "sources": ["https://reviews.example/duo"], "pictures": [],
                                    }] }),
                                    "https://cited.example/review",
                                )),
                            );
                        }
                        if request["text"]["format"]["name"] == "article" && review {
                            let revising = instructions.contains("这次是更新");
                            let (title, body) = if revising {
                                ("iPhone Duo 综合评测（更新）", format!("{}\n\n![](keep:1)\n\n长期使用之后。", "更新后的评测，".repeat(80)))
                            } else {
                                ("iPhone Duo 综合评测", format!("![](keep:1)\n\n{}\n\n看[上手评测](https://reviews.example/duo)。", "各家评测这么说，".repeat(80)))
                            };
                            return (
                                StatusCode::OK,
                                Json(reply(
                                    json!({
                                        "title": title, "body": body,
                                        "pictures": [{ "page": "", "prompt": "一台手机", "caption": "手机" }],
                                        "cover": "封面插画",
                                    }),
                                    "https://cited.example/review",
                                )),
                            );
                        }
                        // The writing model says what an administrator's request asks for.
                        if request["text"]["format"]["name"] == "understanding" {
                            let input = request["input"][0]["content"].as_str().unwrap_or_default();
                            let asked = input.split_once("管理员的请求：").map_or("", |(_, asked)| asked);
                            return (
                                StatusCode::OK,
                                Json(reply(json!({ "understanding": format!(" 他想弄懂「{asked}」。 ") }), "https://elsewhere.example/x")),
                            );
                        }
                        // grok looks for a photograph of a person or a company, and finds one
                        // while it may.
                        if request["text"]["format"]["name"] == "photo" {
                            let (image, page) = if sighting.load(Ordering::SeqCst) {
                                ("https://media.example/files/scene.jpg", "https://media.example/news/photo")
                            } else {
                                ("", "")
                            };
                            return (
                                StatusCode::OK,
                                Json(reply(json!({ "image": image, "page": page }), "https://cited.example/photo")),
                            );
                        }
                        // 追踪: grok names two events, the first by a slug in other words, and
                        // the briefing model the first.
                        let tracker = instructions.contains("「追踪」");
                        if request["text"]["format"]["name"] == "finds" && tracker {
                            if saving.load(Ordering::SeqCst) {
                                sqlx::query("UPDATE articles SET updated_at = now() WHERE event = 'binance-hack-2026-10'")
                                    .execute(&admin)
                                    .await
                                    .unwrap();
                            }
                            if failing.load(Ordering::SeqCst) == 2 {
                                return (StatusCode::BAD_REQUEST, Json(json!({})));
                            }
                            let grok = request["tools"].as_array().unwrap().len() == 2;
                            let named = if grok {
                                vec![("Binance hack 2026-10", "被盗约 63 亿美元"), ("ai-capex-selloff-2026-10", "科技股下跌")]
                            } else {
                                vec![("binance-hack-2026-10", "币安称全额赔付")]
                            };
                            let finds: Vec<Value> = named
                                .into_iter()
                                .map(|(event, points)| json!({
                                    "title": format!("{event} 的进展"), "why": "全网都在谈",
                                    "points": points, "sources": ["https://events.example/news"],
                                    "pictures": ["https://media.example/news/scene"], "event": event,
                                }))
                                .collect();
                            return (
                                StatusCode::OK,
                                Json(reply(json!({ "finds": finds }), "https://cited.example/events")),
                            );
                        }
                        if request["text"]["format"]["name"] == "article" && tracker {
                            let material = request["input"][0]["content"].as_str().unwrap_or_default();
                            let event = material
                                .lines()
                                .find_map(|line| line.strip_prefix("事件："))
                                .unwrap_or_default();
                            let (title, body) = if !instructions.contains("这次是更新") {
                                (
                                    format!("{event} 的追踪"),
                                    format!("{}\n\n![](img:1)\n\n看[报道](https://events.example/news)。", "事情的经过，".repeat(80)),
                                )
                            } else if news.load(Ordering::SeqCst) {
                                (
                                    format!("{event} 的追踪（更新）"),
                                    format!("{}\n\n![](keep:1)\n\n![](img:1)\n\n最新进展。", "更新后的追踪，".repeat(80)),
                                )
                            } else {
                                (format!("{event} 的追踪"), String::new())
                            };
                            return (
                                StatusCode::OK,
                                Json(reply(
                                    json!({
                                        "title": title, "body": body,
                                        "pictures": [{ "page": "", "prompt": "现场", "caption": "现场" }],
                                        "cover": "封面插画",
                                    }),
                                    "https://cited.example/events",
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
    // Wikipedia and the pages pictures come from are read through the same local server.
    state.origin_http = client.clone();
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
    let id = xabc::write(&state, xabc::Line::Share, since)
        .await
        .unwrap()
        .unwrap();

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
        .unwrap()
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
        .unwrap()
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
        Ok(None)
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
        Ok(None)
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
        .unwrap()
        .unwrap();
    let film: Option<String> = sqlx::query_scalar("SELECT film FROM articles WHERE id = $1")
        .bind(loudest)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(film.as_deref(), Some("movie:louderfilm"));

    // While 影视 collects nothing, 解说 writes nothing, however due the article is.
    sqlx::query("UPDATE topics SET enabled = false WHERE id = 'movie'")
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(matches!(
        xabc::write(&state, xabc::Line::Film, Utc::now()).await,
        Ok(None)
    ));
    sqlx::query("UPDATE topics SET enabled = true WHERE id = 'movie'")
        .execute(&state.pool)
        .await
        .unwrap();

    // The article is revised: its figure and cover stay, a new picture joins them, and the line's
    // clock moves on.
    let last = xabc::last(&state, xabc::Line::Film).await.unwrap().unwrap();
    let drawn_before = drawings_log.lock().unwrap().len();
    assert_eq!(
        xabc::write(&state, xabc::Line::Film, Utc::now())
            .await
            .unwrap(),
        Some(explained)
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

    // 精选 opens with XABC's newest 分享, newest first, however well a 解说 is read; the rest
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
    assert_eq!(worth["commentary"], true);
    // While 影视 is not public, 精选 leaves 解说 out and links no list of it.
    let movie_shown = |visible: bool| {
        let state = state.clone();
        async move {
            sqlx::query("UPDATE topics SET visible = $1 WHERE id = 'movie'")
                .bind(visible)
                .execute(&state.pool)
                .await
                .unwrap();
            state.digest.reload_topics(&state).await.unwrap();
        }
    };
    movie_shown(false).await;
    let worth: Value =
        serde_json::from_str(&page(format!("{base}/articles/worth.json")).await).unwrap();
    assert_eq!(worth["commentary"], false);
    assert_eq!(worth["articles"].as_array().unwrap().len(), 2);
    movie_shown(true).await;

    // 时代's round goes by the hour it starts, an hour after `since`: at midnight it looks for
    // people talked about now, at one for leaders, at five for a company. The rounds start in
    // two days, so nothing published in the test is within an interval of them.
    let round = |hour: u32| {
        let zone = state.config.timezone;
        let start = (Utc::now().with_timezone(&zone).date_naive() + chrono::Days::new(2))
            .and_hms_opt(hour, 0, 0)
            .unwrap()
            .and_local_timezone(zone)
            .earliest()
            .unwrap();
        start.to_utc() - chrono::Duration::hours(1)
    };
    let (hot, leaders, company) = (round(0), round(1), round(5));
    let photo_asked = || {
        asked_log
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|request| request["text"]["format"]["name"] == "photo")
            .cloned()
            .unwrap()
    };

    // 时代: the person both scouts name goes first. Her profile shows her comic, drawn in 时代's
    // style, and the illustration the writer asks for, drawn as 分享's are. The scouts give her
    // Wikipedia entry and her X account without having met them, so the writer hears of neither,
    // and her cover is the photograph grok found and looked at.
    let drawn_before = drawings_log.lock().unwrap().len();
    let profile = xabc::write(&state, xabc::Line::People, hot)
        .await
        .unwrap()
        .unwrap();
    type Profile = (Option<String>, String, String, Option<Uuid>, Option<i32>);
    let profiled = |id: Uuid| {
        sqlx::query_as::<_, Profile>(
            "SELECT a.person, a.title, a.body, a.cover, i.width FROM articles a
             LEFT JOIN article_images i ON i.id = a.cover WHERE a.id = $1",
        )
        .bind(id)
        .fetch_one(&state.pool)
    };
    let (person, title, body, cover, width) = profiled(profile).await.unwrap();
    assert_eq!(
        (person.as_deref(), title.as_str(), width),
        (Some("lisa-su"), "Lisa Su 的人物介绍", Some(640))
    );
    // The comic, as wide as the photograph, isn't the cover.
    assert!(!body.contains(&cover.unwrap().to_string()));
    let mut prompts: Vec<String> = drawings_log.lock().unwrap()[drawn_before..]
        .iter()
        .map(|drawing| drawing["prompt"].as_str().unwrap().to_owned())
        .collect();
    prompts.sort();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[0].starts_with("一幅插图\n画面中不要出现"),
        "{prompts:?}"
    );
    assert!(
        prompts[1].starts_with("四格漫画：他的一生\n画成一张四格漫画")
            && !prompts[1].contains("画面里没有主角"),
        "{prompts:?}"
    );
    assert!(
        body.contains("*四个时刻（AI 生成）*\n\n![插图](image:"),
        "{body}"
    );
    let photo = photo_asked();
    assert!(
        photo["instructions"]
            .as_str()
            .unwrap()
            .contains("人物长文找一张封面照片")
    );
    assert_eq!(
        photo["input"][0]["content"],
        "Lisa Su\n文章标题：Lisa Su 的人物介绍"
    );
    assert_eq!(photo["tools"][0]["enable_image_understanding"], true);
    let material = writing(&asked_log.lock().unwrap());
    assert!(material.contains("人物：Lisa Su\n线索：\n1.【X 与全网】Lisa Su 的近况"));
    assert!(material.contains("2.【全网】lisa su 的近况"));

    // Then the one grok named first; the scouts hear who was profiled lately. The scouts met his
    // Wikipedia entry and his X account, which the writer hears of and links his name to. The
    // entry leads with a photograph, which goes before his account's picture, so that is his
    // cover, though his body shows only his comic, and grok isn't asked for one.
    let photos = || {
        asked_log
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request["text"]["format"]["name"] == "photo")
            .count()
    };
    let photos_before = photos();
    let other = xabc::write(&state, xabc::Line::People, hot)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(photos(), photos_before);
    let (person, body, portrait): (Option<String>, String, Option<i32>) = sqlx::query_as(
        "SELECT a.person, a.body, i.width FROM articles a
         LEFT JOIN article_images i ON i.id = a.cover WHERE a.id = $1",
    )
    .bind(other)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        (person.as_deref(), portrait),
        (Some("jensen-huang"), Some(400))
    );
    assert_eq!(body.matches("](image:").count(), 2);
    assert!(body.contains("*四个时刻（AI 生成）*"));
    assert!(writing(&asked_log.lock().unwrap()).contains(
        "人物：Jensen Huang\n维基百科：https://en.wikipedia.org/wiki/Jensen_Huang\nX：https://x.com/JensenHuang\n线索："
    ));
    assert!(body.starts_with("[Jensen Huang](https://x.com/JensenHuang) "));
    let searched = asked_log
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|request| request["text"]["format"]["name"] == "finds")
        .unwrap()["input"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(searched.contains("这一轮找此刻最受关注的人物"));
    assert!(searched.contains("这些人物最近写过，这次不要选：lisa-su\n"));

    // Days later the first is brought up to date; a profile an administrator took back waits,
    // and then nobody is left. Hers now reads like one written before 时代 drew comics, a
    // photograph and no comic, so it gets one. With no portrait on Wikipedia and no photograph
    // grok finds, the cover follows the picture the profile opens with now.
    let (photo, old) = (Uuid::new_v4(), Uuid::new_v4());
    for (image, name) in [(photo, "lisa"), (old, "old")] {
        sqlx::query(
            "INSERT INTO article_images (id, article_id, url, content_type, width, height, expires_at)
             VALUES ($1, $2, 'https://media.example/files/' || $3 || '.jpg', 'image/jpeg', 800, 600,
                     now() + INTERVAL '1 day')",
        )
        .bind(image)
        .bind(profile)
        .bind(name)
        .execute(&state.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE articles SET updated_at = now() - INTERVAL '4 days',
           body = CASE WHEN id = $1 THEN $3 ELSE body END,
           cover = CASE WHEN id = $1 THEN $4 ELSE cover END,
           status = CASE WHEN id = $2 THEN 'pending' ELSE status END
         WHERE id IN ($1, $2)",
    )
    .bind(profile)
    .bind(other)
    .bind(format!(
        "{}\n\n![人像](image:{photo})\n*人像（例）*",
        "一个人的经历，".repeat(80)
    ))
    .bind(old)
    .execute(&state.pool)
    .await
    .unwrap();
    sighting.store(false, Ordering::SeqCst);
    let drawn_before = drawings_log.lock().unwrap().len();
    assert_eq!(
        xabc::write(&state, xabc::Line::People, hot).await.unwrap(),
        Some(profile)
    );
    sighting.store(true, Ordering::SeqCst);
    assert_eq!(drawings_log.lock().unwrap().len(), drawn_before + 1);
    let (title, revision, body, cover): (String, i32, String, Option<Uuid>) =
        sqlx::query_as("SELECT title, revision, body, cover FROM articles WHERE id = $1")
            .bind(profile)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (title.as_str(), revision, cover),
        ("Lisa Su 的人物介绍（更新）", 2, Some(photo))
    );
    assert!(
        body.contains("*人像（例）*\n\n![四个时刻](image:"),
        "{body}"
    );
    let material = writing(&asked_log.lock().unwrap());
    assert!(material.contains("现有全文：\n标题：Lisa Su 的人物介绍\n\n一个人的经历，"));
    assert!(material.contains("最近的线索：\n1.【X 与全网】Lisa Su 的近况"));
    assert!(!material.contains("是四格漫画"));
    assert!(matches!(
        xabc::write(&state, xabc::Line::People, hot).await,
        Err(AppError::Internal(message)) if message == "两路检索都没有找到可写的人物"
    ));
    // A profile taken back while it was being rewritten keeps its words.
    let rewritten = xabc::Finished {
        title: "改写".into(),
        body: "字".repeat(500),
        pictures: Vec::new(),
        cover: None,
    };
    assert!(matches!(
        xabc::revise(&state, other, 1, None, &rewritten).await,
        Err(AppError::Conflict(_))
    ));

    // 时代 lists its profiles under a tab of its own, the one brought up to date last first and
    // without an author; a profile leads back there and opens with its words, its photograph
    // standing for it only on the lists.
    sqlx::query("UPDATE articles SET status = 'published' WHERE id = $1")
        .bind(other)
        .execute(&state.pool)
        .await
        .unwrap();
    let people = page(format!("{base}/people")).await;
    assert!(people.contains("<img src=\"https://media.example/files/lisa.jpg\""));
    assert!(people.contains("<body class=\"public reads articles article-cards people\">"));
    assert!(people.contains("<a class=tab href=\"/people\" aria-current=\"true\">时代</a>"));
    let latest = people.find("Lisa Su 的人物介绍（更新）").unwrap();
    assert!(latest < people.find("Jensen Huang 的人物介绍").unwrap());
    assert!(!people.contains("XABC</a>"));
    assert!(
        page(format!("{base}/people?q=Jensen"))
            .await
            .contains("「Jensen」找到 1 篇")
    );
    // Read or not, profiles stay off 分享's list, 热读 and XABC's page.
    sqlx::query("UPDATE articles SET views = 50 WHERE id = $1")
        .bind(profile)
        .execute(&state.pool)
        .await
        .unwrap();
    for list in [
        "/articles",
        "/articles?view=hot",
        &format!("/articles?by={author}"),
    ] {
        assert!(
            !page(format!("{base}{list}")).await.contains("人物介绍"),
            "{list}"
        );
    }
    let article = page(format!("{base}/articles/{profile}")).await;
    assert!(article.contains("<a href=\"/people\">← 时代</a>"));
    // It shows when it was brought up to date, and its data still tells when it first went out.
    let (published, updated): (DateTime<Utc>, DateTime<Utc>) =
        sqlx::query_as("SELECT published_at, updated_at FROM articles WHERE id = $1")
            .bind(profile)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(published < updated);
    assert!(article.contains(&format!("<time datetime=\"{}\">", updated.to_rfc3339())));
    assert!(article.contains(&format!("\"datePublished\":\"{}\"", published.to_rfc3339())));
    assert!(article.contains("<a class=tab href=\"/people\" aria-current=\"true\">时代</a>"));
    assert!(!article.contains("class=article-cover"));
    // 精选 ranks profiles with the rest.
    let worth: Value =
        serde_json::from_str(&page(format!("{base}/articles/worth.json")).await).unwrap();
    assert!(worth["articles"].as_array().unwrap().iter().any(|article| {
        article["id"] == profile.to_string()
            && article["person"] == true
            && article["film"] == false
    }));

    // Brought up to date, a profile asks Wikipedia for its portrait again. It shows a photograph,
    // its comic and an illustration: the first picture drawn is its comic, which the writer hears
    // to keep, so nothing is drawn.
    let talk = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO article_images (id, article_id, url, content_type, width, height, expires_at)
         VALUES ($1, $2, 'https://media.example/files/talk.jpg', 'image/jpeg', 800, 600,
                 now() + INTERVAL '1 day')",
    )
    .bind(talk)
    .bind(other)
    .execute(&state.pool)
    .await
    .unwrap();
    let (before, shown): (Option<Uuid>, String) = sqlx::query_as(
        "UPDATE articles SET updated_at = now() - INTERVAL '4 days', body = $2 || body
         WHERE id = $1 RETURNING cover, body",
    )
    .bind(other)
    .bind(format!("![展览现场](image:{talk})\n*展览现场（例）*\n\n"))
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let comic = crate::articles::pictures(&shown)[1];
    let drawn_before = drawings_log.lock().unwrap().len();
    assert_eq!(
        xabc::write(&state, xabc::Line::People, hot).await.unwrap(),
        Some(other)
    );
    assert_eq!(drawings_log.lock().unwrap().len(), drawn_before);
    assert!(
        writing(&asked_log.lock().unwrap())
            .contains("其中 ![](keep:2) 是四格漫画，留在「你可能不知道」后面，不再画新的。")
    );
    let (_, _, body, after, _) = profiled(other).await.unwrap();
    assert!(before.is_some() && after.is_some() && after != before);
    assert_eq!(crate::articles::pictures(&body), [talk, comic]);

    let searched = || {
        asked_log
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|request| request["text"]["format"]["name"] == "finds")
            .unwrap()["input"][0]["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    // A round of leaders passes over every person profiled, however long ago; a round of
    // companies writes up a company under a key of its own, with a comic, shown by a photograph
    // grok looked at, and only once.
    sqlx::query(
        "UPDATE articles SET updated_at = now() - INTERVAL '4 days' WHERE person IS NOT NULL",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let leader = xabc::write(&state, xabc::Line::People, leaders)
        .await
        .unwrap()
        .unwrap();
    // His X account shares no picture of its own, so grok's photograph is his cover; the writer
    // links his name to the account all the same.
    let (person, _, body, _, width) = profiled(leader).await.unwrap();
    assert_eq!(
        (person.as_deref(), width),
        (Some("ren-zhengfei"), Some(640))
    );
    assert!(body.starts_with("[Ren Zhengfei](https://x.com/NoFace) "));
    assert!(
        writing(&asked_log.lock().unwrap())
            .contains("人物：Ren Zhengfei\nX：https://x.com/NoFace\n线索：")
    );
    let heard = searched();
    assert!(heard.contains("这一轮不看热度，找科技与科学领域有分量的人物"));
    let passed = heard
        .lines()
        .find_map(|line| line.strip_prefix("这些人物已经写过，这次不要选："))
        .unwrap();
    assert!(
        passed.contains("lisa-su") && passed.contains("jensen-huang"),
        "{passed}"
    );
    let drawn_before = drawings_log.lock().unwrap().len();
    let firm = xabc::write(&state, xabc::Line::People, company)
        .await
        .unwrap()
        .unwrap();
    let (person, title, body, cover, width) = profiled(firm).await.unwrap();
    assert_eq!(
        (person.as_deref(), title.as_str(), width),
        (Some("company:huawei"), "Huawei 的企业介绍", Some(640))
    );
    let drawn = drawings_log.lock().unwrap()[drawn_before..].to_vec();
    assert_eq!(drawn.len(), 2);
    // A company's comic has nobody at its centre.
    assert!(drawn.iter().any(|drawing| {
        let prompt = drawing["prompt"].as_str().unwrap();
        prompt.starts_with("四格漫画") && prompt.contains("画面里没有主角")
    }));
    assert!(body.contains("*四个时刻（AI 生成）*") && !body.contains(&cover.unwrap().to_string()));
    assert!(searched().contains("这一轮找科技行业举足轻重的企业"));
    assert!(
        photo_asked()["instructions"]
            .as_str()
            .unwrap()
            .contains("企业长文找一张封面照片")
    );
    sqlx::query("UPDATE articles SET updated_at = now() - INTERVAL '4 days' WHERE id = $1")
        .bind(firm)
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(matches!(
        xabc::write(&state, xabc::Line::People, company).await,
        Err(AppError::Internal(message)) if message == "两路检索都没有找到可写的企业"
    ));
    assert!(searched().contains("这些企业已经写过，这次不要选：huawei\n"));
    assert!(
        page(format!("{base}/people"))
            .await
            .contains("Huawei 的企业介绍")
    );

    // An administrator names people and companies for 时代, which lines them up, each once, and
    // writes them one after another before its own rounds, as the scheduler starts it.
    let (_, administrator) = account(&state, "admin").await;
    let origin = state.config.public_base_url.origin().ascii_serialization();
    let name = |name: &str| {
        http.post(format!("{base}/api/admin/people"))
            .header("cookie", &administrator)
            .header("origin", &origin)
            .json(&json!({ "name": name }))
            .send()
    };
    let ahead = |subject: &'static str| {
        let request = name(subject);
        async move { request.await.unwrap().json::<Value>().await.unwrap()["ahead"].clone() }
    };
    // 时代 writes the next name on its list, and the test waits until it is done.
    let next = || {
        let state = &state;
        async move {
            xabc::start(state, xabc::Line::People).await;
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                while state.in_flight().contains("__xabc_people__") {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
        }
    };
    assert_eq!(name(" ").await.unwrap().status(), 400);
    // While 时代 is writing, names wait.
    state.in_flight().insert("__xabc_people__".into());
    assert_eq!(ahead(" Huawei ").await, 0);
    assert_eq!(ahead("查无此人").await, 1);
    assert_eq!(ahead("Satya Nadella").await, 2);
    assert_eq!(ahead("Huawei").await, 0);
    xabc::start(&state, xabc::Line::People).await;
    state.in_flight().remove("__xabc_people__");
    // The scouts tell Huawei is a company, so the company written once is brought up to date all
    // the same, shown by a photograph grok looked at. It reads like one written before 时代 drew
    // comics, so it gets one, with nobody at its centre.
    sqlx::query("UPDATE articles SET body = $2 WHERE id = $1")
        .bind(firm)
        .bind("一家企业的经历，".repeat(80))
        .execute(&state.pool)
        .await
        .unwrap();
    let drawn_before = drawings_log.lock().unwrap().len();
    next().await;
    assert_eq!(profiled(firm).await.unwrap().1, "Huawei 的企业介绍（更新）");
    let heard = asked_log
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|request| request["text"]["format"]["name"] == "finds")
        .cloned()
        .unwrap();
    let input = heard["input"][0]["content"].as_str().unwrap();
    assert!(
        input.contains("只写管理员点名的这一位人物或这一家企业") && input.ends_with("点名：Huawei")
    );
    assert_eq!(
        heard["text"]["format"]["schema"]["properties"]["finds"]["items"]["properties"]["company"]
            ["type"],
        "boolean"
    );
    let material = writing(&asked_log.lock().unwrap());
    assert!(
        material.contains("企业：Huawei\n这篇企业介绍上次更新于")
            && !material.contains("是四格漫画"),
        "{material}"
    );
    let drawn = drawings_log.lock().unwrap()[drawn_before..].to_vec();
    assert!(
        drawn.len() == 1
            && drawn[0]["prompt"]
                .as_str()
                .unwrap()
                .contains("画面里没有主角"),
        "{drawn:?}"
    );
    assert!(
        photo_asked()["instructions"]
            .as_str()
            .unwrap()
            .contains("企业长文找一张封面照片")
    );
    // A name the scouts cannot find is dropped, and the next goes on though 时代 failed: someone
    // not written yet, written up anew though a profile went out a moment ago. His Wikipedia
    // entry shows no photograph, so the picture of his X account shows him on the lists, and grok
    // isn't asked for a photograph.
    let photos_before = photos();
    next().await;
    next().await;
    let satya: Uuid = sqlx::query_scalar("SELECT id FROM articles WHERE person = 'satya-nadella'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let (_, title, body, _, width) = profiled(satya).await.unwrap();
    assert_eq!(
        (title.as_str(), width, photos()),
        ("Satya Nadella 的人物介绍", Some(300), photos_before)
    );
    assert!(body.starts_with("[Satya Nadella](https://x.com/satyanadella) "));
    // With the list written, 时代's own round waits its interval.
    let asked_before = asked_log.lock().unwrap().len();
    next().await;
    assert_eq!(asked_log.lock().unwrap().len(), asked_before);
    // Named again, it is brought up to date however lately it was, keeping its comic.
    let drawn_before = drawings_log.lock().unwrap().len();
    assert_eq!(ahead("Huawei").await, 0);
    next().await;
    let revision: i32 = sqlx::query_scalar("SELECT revision FROM articles WHERE id = $1")
        .bind(firm)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(revision, 3);
    assert!(writing(&asked_log.lock().unwrap()).contains("其中 ![](keep:1) 是四格漫画"));
    assert_eq!(drawings_log.lock().unwrap().len(), drawn_before);

    // An administrator asks 分享 to look things into, each once and in turn, apart from 时代's
    // names. Both scouts look the request up, nothing is left out as written lately, and the
    // writer makes a guide of it, published as XABC's 分享 with a cover of its own.
    let research = |brief: &str, cookie: &str| {
        http.post(format!("{base}/api/admin/research"))
            .header("cookie", cookie)
            .header("origin", &origin)
            .json(&json!({ "brief": brief }))
            .send()
    };
    let (_, reader) = account(&state, "user").await;
    assert_eq!(research("找找用法", &reader).await.unwrap().status(), 403);
    assert_eq!(research(" ", &administrator).await.unwrap().status(), 400);
    let asks = |brief: &'static str| {
        let request = research(brief, &administrator);
        async move { request.await.unwrap().json::<Value>().await.unwrap()["ahead"].clone() }
    };
    assert_eq!(asks(" 这个工具怎么用，以官方文档为准 ").await, 0);
    assert_eq!(asks("另一个问题").await, 1);
    assert_eq!(asks("这个工具怎么用，以官方文档为准").await, 0);
    assert_eq!(ahead("Huawei").await, 0);
    let shared = || {
        let state = &state;
        async move {
            xabc::start(state, xabc::Line::Share).await;
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                while state.in_flight().contains("__xabc_article__") {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
        }
    };
    // XABC's 分享 so far, the latest and its cover.
    let shares = || {
        sqlx::query_as::<_, (i64, Uuid, Option<Uuid>)>(
            "SELECT COUNT(*) OVER (), id, cover FROM articles
             WHERE author_id = $1 AND num_nonnulls(film, person, event, product) = 0
             ORDER BY published_at DESC LIMIT 1",
        )
        .bind(author)
        .fetch_one(&state.pool)
    };
    let mut researched = Uuid::nil();
    for brief in ["这个工具怎么用，以官方文档为准", "另一个问题"] {
        let asked_before = asked_log.lock().unwrap().len();
        let (before, ..) = shares().await.unwrap();
        shared().await;
        let requests = asked_log.lock().unwrap()[asked_before..].to_vec();
        // First the request is understood, and both scouts and the writer hear that.
        assert_eq!(requests[0]["text"]["format"]["name"], "understanding");
        assert_eq!(requests[0]["model"], state.config.ai_writing_model.as_str());
        let understood = format!("管理员的请求：{brief}\n对请求的理解：他想弄懂「{brief}」。");
        let searches: Vec<&Value> = requests
            .iter()
            .filter(|request| request["text"]["format"]["name"] == "finds")
            .collect();
        assert_eq!(searches.len(), 2);
        for search in searches {
            assert!(
                search["instructions"]
                    .as_str()
                    .unwrap()
                    .contains("管理员请 XABC 就一件事做一次全网调研")
                    && search["input"][0]["content"]
                        .as_str()
                        .unwrap()
                        .ends_with(&understood)
            );
        }
        let guide = requests
            .iter()
            .find(|request| request["text"]["format"]["name"] == "article")
            .unwrap();
        assert!(
            guide["instructions"]
                .as_str()
                .unwrap()
                .contains("这一篇是管理员点名的调研")
        );
        assert!(
            guide["input"][0]["content"]
                .as_str()
                .unwrap()
                .contains(&format!(
                    "{understood}\n线索：\n1.【X 与全网】一个好玩的工具"
                ))
        );
        let (after, latest, cover) = shares().await.unwrap();
        assert!(after == before + 1 && cover.is_some());
        // A page read to understand the request keeps its link, which an hourly 分享 drops.
        let body: String = sqlx::query_scalar("SELECT body FROM articles WHERE id = $1")
            .bind(latest)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        assert!(
            body.contains("看看[别处](https://elsewhere.example/x)"),
            "{body}"
        );
        researched = latest;
    }
    // With the requests written, 分享's own round waits its interval, and 时代's name is still
    // there for it.
    let asked_before = asked_log.lock().unwrap().len();
    shared().await;
    assert_eq!(asked_log.lock().unwrap().len(), asked_before);
    next().await;
    assert!(asked_log.lock().unwrap().len() > asked_before);

    // 追踪: the event both scouts name goes first, under the key its slug makes, in an article that
    // shows no drawn picture. Showing none at all, it takes for the lists the picture the finds'
    // page shares.
    let tracker =
        |state: AppState| async move { xabc::write(&state, xabc::Line::Tracker, Utc::now()).await };
    let drawn_before = drawings_log.lock().unwrap().len();
    let followed = tracker(state.clone()).await.unwrap().unwrap();
    let (event, title, body, cover): (Option<String>, String, String, Option<String>) =
        sqlx::query_as(
            "SELECT a.event, a.title, a.body, i.url FROM articles a
             LEFT JOIN article_images i ON i.id = a.cover WHERE a.id = $1",
        )
        .bind(followed)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        (event.as_deref(), title.as_str()),
        (Some("binance-hack-2026-10"), "binance-hack-2026-10 的追踪")
    );
    assert!(cover.is_some() && !body.contains("](image:"), "{body}");
    assert_eq!(drawings_log.lock().unwrap().len(), drawn_before);
    let material = writing(&asked_log.lock().unwrap());
    assert!(
        material.contains(
            "事件：binance-hack-2026-10\n线索：\n1.【X 与全网】Binance hack 2026-10 的进展"
        )
    );
    assert!(material.contains("2.【全网】binance-hack-2026-10 的进展"));
    // Then the other one grok named, while the first waits; then neither may be written about,
    // which is no failure. The scouts hear of the events followed by their keys.
    let selloff = tracker(state.clone()).await.unwrap().unwrap();
    let event: Option<String> = sqlx::query_scalar("SELECT event FROM articles WHERE id = $1")
        .bind(selloff)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(event.as_deref(), Some("ai-capex-selloff-2026-10"));
    assert_eq!(tracker(state.clone()).await.unwrap(), None);
    let heard = searched();
    assert!(heard.contains("正在追踪的事件（标识｜标题｜上次更新）：\n- ai-capex-selloff-2026-10｜ai-capex-selloff-2026-10 的追踪｜"));
    assert!(heard.contains("\n- binance-hack-2026-10｜binance-hack-2026-10 的追踪｜"));
    // Both searches failing is a failure.
    failing.store(2, Ordering::SeqCst);
    assert!(matches!(
        tracker(state.clone()).await,
        Err(AppError::Internal(message)) if message == "两路检索都失败了"
    ));
    failing.store(0, Ordering::SeqCst);

    // Saved by an administrator while the scouts search, an article waits like one just brought
    // up to date.
    sqlx::query("UPDATE articles SET updated_at = now() - INTERVAL '7 hours' WHERE id = $1")
        .bind(followed)
        .execute(&state.pool)
        .await
        .unwrap();
    saving.store(true, Ordering::SeqCst);
    assert_eq!(tracker(state.clone()).await.unwrap(), None);
    saving.store(false, Ordering::SeqCst);

    // Hours later the first is brought up to date: its figure stays, and its cover is the
    // picture it opens with now.
    let scene = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO article_images (id, article_id, url, content_type, width, height, expires_at)
         VALUES ($1, $2, 'https://media.example/files/scene.jpg', 'image/jpeg', 800, 600,
                 now() + INTERVAL '1 day')",
    )
    .bind(scene)
    .bind(followed)
    .execute(&state.pool)
    .await
    .unwrap();
    let figure = format!("![现场](image:{scene})\n*现场（例）*");
    sqlx::query(
        "UPDATE articles SET updated_at = now() - INTERVAL '7 hours', body = $2 || body
         WHERE id = $1",
    )
    .bind(followed)
    .bind(format!("{figure}\n\n"))
    .execute(&state.pool)
    .await
    .unwrap();
    assert_eq!(tracker(state.clone()).await.unwrap(), Some(followed));
    let (title, body, revision, cover): (String, String, i32, Option<Uuid>) =
        sqlx::query_as("SELECT title, body, revision, cover FROM articles WHERE id = $1")
            .bind(followed)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (title.as_str(), revision, cover),
        ("binance-hack-2026-10 的追踪（更新）", 2, Some(scene))
    );
    assert!(body.contains(&figure), "{body}");
    let material = writing(&asked_log.lock().unwrap());
    assert!(material.contains("这篇追踪上次更新于 "));
    assert!(
        material.contains(
            "现有全文：\n标题：binance-hack-2026-10 的追踪\n\n![](keep:1)\n\n事情的经过，"
        )
    );
    assert!(material.contains("最新的线索：\n1.【X 与全网】Binance hack 2026-10 的进展"));

    // An event whose article was taken back stays out, though both scouts name it; they hear it
    // is no longer followed.
    sqlx::query(
        "UPDATE articles SET updated_at = now() - INTERVAL '7 hours',
           status = CASE WHEN id = $1 THEN 'pending' ELSE status END
         WHERE event IS NOT NULL",
    )
    .bind(followed)
    .execute(&state.pool)
    .await
    .unwrap();
    assert_eq!(tracker(state.clone()).await.unwrap(), Some(selloff));
    assert!(searched().contains(
        "不再追踪的事件（标识｜标题）：\n- binance-hack-2026-10｜binance-hack-2026-10 的追踪（更新）\n"
    ));

    // Back again, it has nothing new to take: the article stays as it was, and the event waits
    // as if brought up to date, so the other one goes next.
    sqlx::query(
        "UPDATE articles SET status = 'published', updated_at = now() - INTERVAL '7 hours'
         WHERE event IS NOT NULL",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let standing = || {
        let state = state.clone();
        async move {
            sqlx::query_as::<_, (i32, DateTime<Utc>)>(
                "SELECT revision, updated_at FROM articles WHERE id = $1",
            )
            .bind(followed)
            .fetch_one(&state.pool)
            .await
            .unwrap()
        }
    };
    let before = standing().await;
    news.store(false, Ordering::SeqCst);
    assert_eq!(tracker(state.clone()).await.unwrap(), None);
    assert_eq!(standing().await, before);
    news.store(true, Ordering::SeqCst);
    assert_eq!(tracker(state.clone()).await.unwrap(), Some(selloff));

    // 追踪 lists its articles under a tab of its own, the one brought up to date last first and
    // dated to the minute; an article leads back there, opens with its words and stays off
    // 分享's list.
    let events = page(format!("{base}/tracker")).await;
    assert!(events.contains("<body class=\"public reads articles article-cards\">"));
    assert_eq!(events.matches("<a class=image-news-media").count(), 2);
    assert!(events.contains("<a class=tab href=\"/tracker\" aria-current=\"true\">追踪</a>"));
    let latest = events
        .find("ai-capex-selloff-2026-10 的追踪（更新）")
        .unwrap();
    assert!(latest < events.find("binance-hack-2026-10 的追踪（更新）").unwrap());
    let updated: DateTime<Utc> =
        sqlx::query_scalar("SELECT updated_at FROM articles WHERE id = $1")
            .bind(selloff)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(events.contains(&format!(
        "<time datetime=\"{}\">{}</time>",
        updated.to_rfc3339(),
        updated
            .with_timezone(&state.config.timezone)
            .format("%Y-%m-%d %H:%M")
    )));
    assert!(
        page(format!("{base}/tracker?q=selloff"))
            .await
            .contains("「selloff」找到 1 篇")
    );
    assert!(!page(format!("{base}/articles")).await.contains("的追踪"));
    let article = page(format!("{base}/articles/{followed}")).await;
    assert!(article.contains("<a href=\"/tracker\">← 追踪</a>"));
    assert!(article.contains("<a class=tab href=\"/tracker\" aria-current=\"true\">追踪</a>"));
    assert!(!article.contains("class=article-cover"));
    assert!(
        page(format!("{base}/articles/sitemap.xml"))
            .await
            .contains("/tracker</loc>")
    );
    // 精选 ranks 追踪 with the rest, but opens with XABC's 分享 alone, the newest first, what it
    // looked into on request among them.
    sqlx::query("UPDATE articles SET views = 80 WHERE id = $1")
        .bind(selloff)
        .execute(&state.pool)
        .await
        .unwrap();
    let worth: Value =
        serde_json::from_str(&page(format!("{base}/articles/worth.json")).await).unwrap();
    let worth = worth["articles"].as_array().unwrap();
    assert_eq!(worth[0]["id"], researched.to_string());
    assert!(worth.iter().any(|article| {
        article["id"] == selloff.to_string()
            && article["event"] == true
            && article["person"] == false
    }));

    // Each line waits for its own articles: a 分享 just out keeps another 分享 back, not 追踪.
    let finished = |title: &str| xabc::Finished {
        title: title.into(),
        body: "字".repeat(500),
        pictures: Vec::new(),
        cover: None,
    };
    let shared = Utc::now();
    xabc::publish(&state, author, shared, None, &finished("丙"))
        .await
        .unwrap();
    assert!(matches!(
        xabc::publish(&state, author, shared, None, &finished("丁")).await,
        Err(AppError::Conflict(_))
    ));
    let event = Some(xabc::Subject::Event("guard-2026-10"));
    xabc::publish(&state, author, shared, event, &finished("戊"))
        .await
        .unwrap();
    // 分享 times itself by its own articles alone.
    let shared: DateTime<Utc> =
        sqlx::query_scalar("SELECT published_at FROM articles WHERE title = '丙'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        xabc::last(&state, xabc::Line::Share).await.unwrap(),
        Some(shared)
    );

    // 数码 · 评测: a product 数码 checked and two owners' posts discuss is reviewed. Its official
    // photograph, which 数码 keeps, goes into the review where the writer places it, read but not
    // copied, and stands for it on the lists.
    let product = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO catalog_entries (id, topic_id, slug, name, name_en, creator, kind, key, source_urls, guide, images)
         VALUES ($1, 'digital', 'iphone-duo', 'iPhone Duo', 'iPhone Duo', 'Apple', 'phone', 'apple:iphoneduo', '[]',
                 '{\"intro\": \"首款可折叠 iPhone。\", \"facts\": [{\"label\": \"芯片\", \"text\": \"A20 Pro\"}]}',
                 jsonb_build_array(jsonb_build_object(
                     'url', 'https://media.example/files/scene.jpg', 'source_url', 'https://media.example/news/duo',
                     'caption', 'iPhone Duo', 'caption_en', 'iPhone Duo', 'credit', 'Apple',
                     'expires_at', now() + INTERVAL '30 days')))",
    )
    .bind(product)
    .execute(&state.pool)
    .await
    .unwrap();
    let gadget = |role: &'static str| {
        let state = state.clone();
        async move {
            let item = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url)
                 VALUES ($1, 'digital', CURRENT_DATE, sha256(convert_to($1::text, 'UTF8')), '一位用户的实拍', '折痕几乎看不见', $2)",
            )
            .bind(item)
            .bind(format!("https://x.com/owner/status/{}", item.as_u128() % 1_000_000_000))
            .execute(&state.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO catalog_items (item_id, entry_id, role) VALUES ($1, $2, $3)")
                .bind(item)
                .bind(product)
                .bind(role)
                .execute(&state.pool)
                .await
                .unwrap();
        }
    };
    gadget("share").await;
    // AI 硬件 files the same product too, without a guide, and collected the same post: one
    // owner's post, counted once, is not enough yet.
    let twin = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO catalog_entries (id, topic_id, slug, name, name_en, creator, kind, key, source_urls)
         VALUES ($1, 'ai-hardware', 'iphone-duo', 'iPhone Duo', 'iPhone Duo', 'Apple', 'computer', 'apple:iphoneduo', '[]')",
    )
    .bind(twin)
    .execute(&state.pool)
    .await
    .unwrap();
    let copied = |role: &'static str, fingerprint: Option<Vec<u8>>| {
        let state = state.clone();
        async move {
            let item = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url)
                 VALUES ($1, 'ai-hardware', CURRENT_DATE, COALESCE($2, sha256(convert_to($1::text, 'UTF8'))),
                         '另一栏收的内容', '同一款手机', 'https://reviews.example/' || $1::text)",
            )
            .bind(item)
            .bind(fingerprint)
            .execute(&state.pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO catalog_items (item_id, entry_id, role) VALUES ($1, $2, $3)")
                .bind(item)
                .bind(twin)
                .bind(role)
                .execute(&state.pool)
                .await
                .unwrap();
        }
    };
    let post: Vec<u8> =
        sqlx::query_scalar("SELECT fingerprint FROM items WHERE topic_id = 'digital'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    copied("share", Some(post)).await;
    assert!(matches!(
        xabc::write(&state, xabc::Line::Review, Utc::now()).await,
        Err(AppError::Internal(message)) if message == "数码最近没有待评测的产品"
    ));
    // A review AI 硬件 collected makes two. Only AI 硬件's stories are recent now, and the guide
    // is still found in 数码's entry.
    sqlx::query(
        "UPDATE items SET first_seen_at = now() - INTERVAL '20 days' WHERE topic_id = 'digital'",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    copied("review", None).await;
    let reviewed = xabc::write(&state, xabc::Line::Review, Utc::now())
        .await
        .unwrap()
        .unwrap();
    let (key, title, body, cover): (Option<String>, String, String, Option<Uuid>) =
        sqlx::query_as("SELECT product, title, body, cover FROM articles WHERE id = $1")
            .bind(reviewed)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (key.as_deref(), title.as_str()),
        (Some("apple:iphoneduo"), "iPhone Duo 综合评测")
    );
    let official: (Uuid, String, i32) =
        sqlx::query_as("SELECT id, url, width FROM article_images WHERE article_id = $1")
            .bind(reviewed)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (official.1.as_str(), official.2, cover),
        (
            "https://media.example/files/scene.jpg",
            640,
            Some(official.0)
        )
    );
    assert!(body.starts_with(&format!(
        "![iPhone Duo](image:{})\n*iPhone Duo（图：[Apple](<https://media.example/news/duo>)）*",
        official.0
    )));
    assert!(body.contains("[上手评测](https://reviews.example/duo)"));
    let material = writing(&asked_log.lock().unwrap());
    assert!(material.contains("产品：iPhone Duo（iPhone Duo，Apple）\n数码栏目核实过的官方介绍：首款可折叠 iPhone。\n官方参数：\n- 芯片：A20 Pro\n官方图片：\n1. iPhone Duo（Apple）\n"));
    assert!(material.contains("【评测】另一栏收的内容：同一款手机"));
    assert_eq!(material.matches("【用户分享】").count(), 1);
    // Reviewed once, it waits for more news.
    assert!(matches!(
        xabc::write(&state, xabc::Line::Review, Utc::now()).await,
        Err(AppError::Internal(message)) if message == "数码最近没有待评测的产品"
    ));
    // Days later, four new reviews and posts are not enough and the fifth brings it up to date,
    // its photograph kept; what came before the last update doesn't count.
    sqlx::query(
        "UPDATE items SET first_seen_at = now() - INTERVAL '5 days' WHERE topic_id IN ('digital', 'ai-hardware')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE articles SET updated_at = now() - INTERVAL '4 days' WHERE id = $1")
        .bind(reviewed)
        .execute(&state.pool)
        .await
        .unwrap();
    for role in ["review", "share", "review", "share"] {
        gadget(role).await;
    }
    assert!(matches!(
        xabc::write(&state, xabc::Line::Review, Utc::now()).await,
        Err(AppError::Internal(message)) if message == "数码最近没有待评测的产品"
    ));
    gadget("review").await;
    assert_eq!(
        xabc::write(&state, xabc::Line::Review, Utc::now())
            .await
            .unwrap(),
        Some(reviewed)
    );
    let (title, body, revision): (String, String, i32) =
        sqlx::query_as("SELECT title, body, revision FROM articles WHERE id = $1")
            .bind(reviewed)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (title.as_str(), revision),
        ("iPhone Duo 综合评测（更新）", 2)
    );
    assert!(body.contains(&format!("![iPhone Duo](image:{})", official.0)));
    assert!(
        writing(&asked_log.lock().unwrap())
            .contains("数码栏目在上次更新之后收到的评测和用户分享：")
    );

    // 评测 lists its reviews under 数码's tabs, after its topics, and a review leads back there;
    // 分享's list keeps out of it.
    let list = page(format!("{base}/gadgets/reviews")).await;
    assert!(list.contains("<a href=\"/gadgets/reviews\" aria-current=\"true\">评测</a></nav>"));
    assert!(list.contains("<body class=\"public catalog gadgets catalog-commentary\">"));
    assert!(list.contains("iPhone Duo 综合评测（更新）"));
    let article = page(format!("{base}/articles/{reviewed}")).await;
    assert!(article.contains("<a href=\"/gadgets/reviews\">← 评测</a>"));
    assert!(!article.contains("class=article-cover"));
    assert!(!page(format!("{base}/articles")).await.contains("综合评测"));
}
