use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

/// The edition the official pages on hooks are at, which the handbook's test moves on; the
/// others stay as they are.
static HOOKS_EDITION: AtomicU8 = AtomicU8::new(0);

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
    assert_eq!(pictures(&shown.body, shown.cover).len(), all);
    assert!(checked("Title", &body(all), Some(ids[all])).is_err());
    assert!(checked("Title", &body(all - 1), Some(ids[all])).is_ok());
    // Addresses that aren't the article's own pictures don't count.
    assert!(pictures("![](https://example.com/a.png) ![](image:nope)", None).is_empty());
    // They are found as the page finds them, where a formula's backtick hides no picture, and the
    // cover comes first.
    let (cover, inside) = (ids[1], ids[2]);
    assert_eq!(
        pictures(&format!("\\(x`\\) ![照片](image:{inside}) `"), Some(cover)),
        [cover, inside]
    );
    assert_eq!(
        pictures(
            &format!("![a](image:{inside}) ![b](image:{cover})"),
            Some(cover)
        ),
        [cover, inside]
    );
    assert_eq!(checked("  标题  ", "", None).unwrap().title, "标题");
    // Postgres text holds no NUL: it goes, from the excerpt too.
    let clean = checked("标\0题", "正\0文", None).unwrap();
    assert_eq!(
        (clean.title, clean.body, clean.excerpt),
        ("标题".into(), "正文".into(), "正文".into())
    );
    assert!(checked(&"题".repeat(TITLE_CHARS + 1), "", None).is_err());
    assert!(checked("", &"字".repeat(BODY_CHARS + 1), None).is_err());
}

#[test]
fn only_jpeg_png_webp_and_gif_pictures_are_taken_and_their_size_is_read() {
    assert_eq!(image_kind(&jpeg(64, 48)).unwrap(), ("image/jpeg", (64, 48)));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(3, 2))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    assert_eq!(image_kind(png.get_ref()).unwrap(), ("image/png", (3, 2)));
    let mut gif = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(5, 4))
        .write_to(&mut gif, image::ImageFormat::Gif)
        .unwrap();
    assert_eq!(image_kind(gif.get_ref()).unwrap(), ("image/gif", (5, 4)));
    for refused in [
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".as_slice(),
        b"<html><script>alert(1)</script></html>",
        b"",
    ] {
        assert!(matches!(
            image_kind(refused),
            Err(AppError::UnsupportedMediaType(_))
        ));
    }
    // A picture's signature without a readable picture behind it.
    assert!(matches!(
        image_kind(b"\xFF\xD8\xFF\xE0broken"),
        Err(AppError::BadRequest(_))
    ));
}

#[test]
fn an_article_renders_only_its_own_pictures_and_no_markup() {
    let own = Uuid::from_u128(1);
    let foreign = Uuid::from_u128(2);
    let html = crate::site::markdown(
        crate::site::parsed(
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
        "code.claude.com".into(),
        "learn.chatgpt.com".into(),
        "platform.claude.com".into(),
        "developers.openai.com".into(),
        "api-docs.deepseek.com".into(),
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
                } else if path.contains(".md") {
                    // An official documentation page, as its Markdown twin reads.
                    let edition = if path.contains("hooks") {
                        HOOKS_EDITION.load(Ordering::SeqCst)
                    } else {
                        0
                    };
                    // Edition 9 is the page gone, which DeepSeek's site answers with its HTML.
                    let page = if edition == 9 {
                        "<!doctype html>\n<html lang=\"en\"><body>DeepSeek API Docs</body></html>"
                            .to_owned()
                    } else {
                        format!(
                            "# {path}\n\nRun `claude -p` with `--output-format json`, set `CLAUDE_CODE_USE_BEDROCK` and `model` to claude-opus-5-5. Edition {edition}."
                        )
                    };
                    ("text/markdown; charset=utf-8", page.into_bytes())
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
    use std::sync::Mutex;
    let database_url =
        std::env::var("NEWS_ARTICLES_TEST_DATABASE_URL").expect("disposable database URL");
    // The AI reviews each submit as `verdict` says and keeps every request it is sent. While
    // `gated`, it says it has one and answers only once `released`. Its first verdict passes but
    // gives a reason, which contradicts itself.
    let verdict = Arc::new(Mutex::new(json!({ "pass": true, "reason": "测试拦下" })));
    let reviews: Arc<Mutex<Vec<Value>>> = Arc::default();
    let gated = Arc::new(AtomicBool::new(false));
    let (arrived, released) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    let ai = Router::new().route(
        "/v1/responses",
        post({
            let (verdict, reviews) = (verdict.clone(), reviews.clone());
            let (gated, arrived, released) = (gated.clone(), arrived.clone(), released.clone());
            move |Json(request): Json<Value>| {
                reviews.lock().unwrap().push(request);
                let text = verdict.lock().unwrap().to_string();
                let gate = gated
                    .load(Ordering::SeqCst)
                    .then(|| (arrived.clone(), released.clone()));
                async move {
                    if let Some((arrived, released)) = gate {
                        arrived.notify_one();
                        released.notified().await;
                    }
                    Json(
                        json!({ "status": "completed", "output": [{ "type": "message",
                        "content": [{ "type": "output_text", "text": text }] }] }),
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
    // The AI was shown the title, the body and the one picture, the cover too, and held it.
    let shown = reviews.lock().unwrap()[0]["input"][0]["content"].clone();
    assert!(
        shown[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("标题：第一篇\n\nHello **world**")
    );
    assert_eq!(shown[1]["image_url"], url.as_str());
    assert_eq!(shown.as_array().unwrap().len(), 2);
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
    assert_eq!(queue["pending"][0]["held"], "AI：测试拦下");
    let mine: Value = get("/api/articles", &author)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let listed = mine["articles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|article| article["id"] == id.as_str())
        .unwrap();
    assert_eq!(
        (listed["status"].as_str(), &listed["held"]),
        (Some("pending"), &Value::Null)
    );
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
            assert_eq!(queue["published"][0]["kind"], "share");
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
    // Its tab follows 调研's on every page, the two with 时代 and 追踪 at their sort weight, and
    // the sitemap lists it.
    assert!(list.contains("<a class=tab href=\"/articles\" aria-current=\"true\">分享</a>"));
    let reads = anonymous("/reads")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(reads.contains(
        "<a class=tab href=\"/research\">调研</a><a class=tab href=\"/articles\">分享</a><a class=tab href=\"/people\">时代</a><a class=tab href=\"/tracker\">追踪</a>"
    ));
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

    // Taken back, it stays with the administrators: its author's changes aren't sent to the AI
    // until a ❤️. After one the AI reviews the author's edits again, and passed, one is public
    // at once.
    assert_eq!(status(heart(&admin, false, 5).send().await.unwrap()), 200);
    let queue: Value = get("/api/admin/articles", &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queue["pending"][0]["held"], "管理员下线");
    let asked = reviews.lock().unwrap().len();
    let saved: Value = save(&author, 5, &body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (saved["status"].as_str(), saved["revision"].as_i64()),
        (Some("draft"), Some(6))
    );
    let submitted: Value = submit(6).send().await.unwrap().json().await.unwrap();
    assert_eq!(submitted["status"], "pending");
    assert_eq!(reviews.lock().unwrap().len(), asked);
    assert_eq!(status(heart(&admin, true, 6).send().await.unwrap()), 200);
    // A picture tmpfile no longer keeps isn't shown to the AI, as readers don't see it either.
    let expire = |days: i32| {
        sqlx::query(
            "UPDATE article_images SET expires_at = now() + make_interval(days => $2)
             WHERE article_id = $1::uuid",
        )
        .bind(&id)
        .bind(days)
        .execute(&state.pool)
    };
    expire(-1).await.unwrap();
    *verdict.lock().unwrap() = json!({ "pass": true, "reason": "" });
    let saved: Value = save(&author, 6, &body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (saved["status"].as_str(), saved["revision"].as_i64()),
        (Some("published"), Some(7))
    );
    let shown = reviews.lock().unwrap()[asked]["input"][0]["content"].clone();
    assert_eq!(shown.as_array().unwrap().len(), 1);
    expire(30).await.unwrap();
    assert_eq!(
        status(anonymous(&format!("/articles/{id}")).send().await.unwrap()),
        200
    );
    // An administrator's ❤️ while the AI reviews is what the author is told, and the AI's later
    // refusal holds nothing public.
    *verdict.lock().unwrap() = json!({ "pass": false, "reason": "测试拦下" });
    gated.store(true, Ordering::SeqCst);
    let update = tokio::spawn(save(&author, 7, &body).send());
    arrived.notified().await;
    assert_eq!(status(heart(&admin, true, 8).send().await.unwrap()), 200);
    released.notify_one();
    let saved: Value = update.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!(
        (saved["status"].as_str(), saved["revision"].as_i64()),
        (Some("published"), Some(8))
    );
    let standing = || async {
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT status, held FROM articles WHERE id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(&state.pool)
        .await
        .unwrap()
    };
    // Nor does a refusal from before a ❤️ hold the revision its author saved since, though that
    // waits, as a save leaves it, for its own review to start; the overtaken update hears of the
    // conflict.
    let overtaken = tokio::spawn(save(&author, 8, &body).send());
    arrived.notified().await;
    assert_eq!(status(heart(&admin, true, 9).send().await.unwrap()), 200);
    sqlx::query("UPDATE articles SET status = 'pending', revision = 10 WHERE id = $1::uuid")
        .bind(&id)
        .execute(&state.pool)
        .await
        .unwrap();
    released.notify_one();
    assert_eq!(status(overtaken.await.unwrap().unwrap()), 409);
    assert_eq!(standing().await, ("pending".to_owned(), None));
    assert_eq!(status(heart(&admin, true, 10).send().await.unwrap()), 200);
    // A refusal holds what its author saved and submitted again while it was being given, so
    // the later review's pass publishes nothing.
    *verdict.lock().unwrap() = json!({ "pass": false, "reason": "测试拦下" });
    let refused = tokio::spawn(save(&author, 10, &body).send());
    arrived.notified().await;
    let saved: Value = save(&author, 11, &body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["status"], "draft");
    *verdict.lock().unwrap() = json!({ "pass": true, "reason": "" });
    let resubmitted = tokio::spawn(submit(12).send());
    arrived.notified().await;
    released.notify_one();
    assert_eq!(status(refused.await.unwrap().unwrap()), 409);
    released.notify_one();
    let submitted: Value = resubmitted.await.unwrap().unwrap().json().await.unwrap();
    assert_eq!(submitted["status"], "pending");
    assert_eq!(
        standing().await,
        ("pending".to_owned(), Some("AI：测试拦下".to_owned()))
    );
    gated.store(false, Ordering::SeqCst);
    assert_eq!(status(heart(&admin, true, 12).send().await.unwrap()), 200);
    // Once its author's reviews for the day are used up, an update waits for an administrator
    // without asking the AI.
    sqlx::query("UPDATE authors SET reviews = $2 WHERE user_id = $1")
        .bind(writer)
        .bind(DAILY_REVIEWS)
        .execute(&state.pool)
        .await
        .unwrap();
    let asked = reviews.lock().unwrap().len();
    let saved: Value = save(&author, 12, &body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["status"], "pending");
    assert_eq!(reviews.lock().unwrap().len(), asked);
    let queue: Value = get("/api/admin/articles", &admin)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(queue["pending"][0]["held"], "作者今天的 AI 审核次数已用完");
    assert_eq!(status(heart(&admin, true, 13).send().await.unwrap()), 200);

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
/// then 时代's profiles and 追踪's events, written, brought up to date and listed; last XAI's
/// 分享 on what X talks about.
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
    // Holds research on questions about writing side by side until the test lets it go.
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
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
                let (asked, failing, news, saving, sighting, gate) = (
                    asked.clone(),
                    failing.clone(),
                    news.clone(),
                    saving.clone(),
                    sighting.clone(),
                    gate.clone(),
                );
                move |Json(request): Json<Value>| {
                    let (asked, failing, news, saving, sighting, gate, admin, body) = (
                        asked.clone(),
                        failing.clone(),
                        news.clone(),
                        saving.clone(),
                        sighting.clone(),
                        gate.clone(),
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
                        // The writing model says what a reader's question asks, unless it can't
                        // be written.
                        if request["text"]["format"]["name"] == "understanding" {
                            let input = request["input"][0]["content"].as_str().unwrap_or_default();
                            let asked = input.split_once("读者的问题：").map_or("", |(_, asked)| asked);
                            if asked.contains("写不出") {
                                return (StatusCode::BAD_REQUEST, Json(json!({})));
                            }
                            if asked.contains("并行") {
                                gate.acquire().await.unwrap().forget();
                            }
                            return (
                                StatusCode::OK,
                                Json(reply(json!({ "understanding": format!(" 他想弄懂「{asked}」。 ") }), "https://elsewhere.example/x")),
                            );
                        }
                        // The AI passes what it reviews, but gambling.
                        if request["text"]["format"]["name"] == "review" {
                            let text = request["input"][0]["content"][0]["text"].as_str().unwrap_or_default();
                            let verdict = if text.contains("赌场") {
                                json!({ "pass": false, "reason": "涉及赌博" })
                            } else {
                                json!({ "pass": true, "reason": "" })
                            };
                            return (StatusCode::OK, Json(reply(verdict, "https://cited.example/review")));
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
                        // the briefing model the first; in a round an administrator asks for,
                        // both name the event they name, unless nothing can be found, and know
                        // 币安被盗 by its slug only when they hear of it.
                        let tracker = instructions.contains("「追踪」");
                        if request["text"]["format"]["name"] == "finds" && tracker {
                            if saving.load(Ordering::SeqCst) {
                                sqlx::query("UPDATE articles SET updated_at = now() WHERE kind = 'event' AND subject = 'binance-hack-2026-10'")
                                    .execute(&admin)
                                    .await
                                    .unwrap();
                            }
                            if failing.load(Ordering::SeqCst) == 2 {
                                return (StatusCode::BAD_REQUEST, Json(json!({})));
                            }
                            let grok = request["tools"].as_array().unwrap().len() == 2;
                            let input = request["input"][0]["content"].as_str().unwrap_or_default();
                            let named = if let Some((_, event)) = input.split_once("点名：") {
                                let event = match event {
                                    "币安被盗" if input.contains("\n- binance-hack-2026-10｜") => "binance-hack-2026-10",
                                    "币安被盗" => "binance-theft-2026-10",
                                    event => event,
                                };
                                if event == "查无此事" { Vec::new() } else { vec![(event, "最新进展")] }
                            } else if grok {
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
                        // 分享: the scouts find a fun tool, XAI's also what X talks about.
                        let trending = instructions.contains("你是 XAI");
                        if request["text"]["format"]["name"] == "finds" {
                            let grok = request["tools"].as_array().unwrap().len() == 2;
                            match failing.load(Ordering::SeqCst) {
                                2 => return (StatusCode::BAD_REQUEST, Json(json!({}))),
                                1 if grok => return (StatusCode::BAD_REQUEST, Json(json!({}))),
                                _ => {}
                            }
                            let mut finds = vec![json!({
                                "title": "一个好玩的工具", "why": "好玩", "points": "能做很多事",
                                "sources": ["https://known.example/tool"], "pictures": [],
                            })];
                            if trending {
                                finds.push(json!({
                                    "title": "大家都在聊的事", "why": "X 上都在讨论", "points": "来龙去脉",
                                    "sources": ["https://talk.example/thread"], "pictures": [],
                                }));
                            }
                            return (
                                StatusCode::OK,
                                Json(reply(json!({ "finds": finds }), "https://cited.example/search")),
                            );
                        }
                        (
                            StatusCode::OK,
                            Json(reply(
                                json!({
                                    "title": if trending { "大家都在聊的一件事" } else { "今天分享一个好玩的工具" },
                                    "body": body,
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
    // 调研 is written one at a time first; several at once come last.
    config.max_concurrent_research = 1;
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
    let author = xabc::author(&state, xabc::Line::Share).await.unwrap();
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
        sqlx::query_as("SELECT subject, title, body, cover, revision FROM articles WHERE id = $1")
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
        sqlx::query_as("SELECT subject, body, cover FROM articles WHERE id = $1")
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
        Err(AppError::Conflict(message)) if message == "这条线刚发过一篇文章"
    ));
    let loudest = xabc::write(&state, xabc::Line::Film, Utc::now())
        .await
        .unwrap()
        .unwrap();
    let film: Option<String> = sqlx::query_scalar("SELECT subject FROM articles WHERE id = $1")
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
                article["kind"] == "film",
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
            "SELECT a.subject, a.title, a.body, a.cover, i.width FROM articles a
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
        "SELECT a.subject, a.body, i.width FROM articles a
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
    assert!(
        worth["articles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|article| { article["id"] == profile.to_string() && article["kind"] == "person" })
    );

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
    let comic = crate::articles::pictures(&shown, None)[1];
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
    assert_eq!(crate::articles::pictures(&body, None), [talk, comic]);

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
    sqlx::query("UPDATE articles SET updated_at = now() - INTERVAL '4 days' WHERE kind = 'person'")
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
    let (administrator_id, administrator) = account(&state, "admin").await;
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
    let satya: Uuid = sqlx::query_scalar(
        "SELECT id FROM articles WHERE kind = 'person' AND subject = 'satya-nadella'",
    )
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

    // 调研: readers ask, and XABC researches each question and writes it up under their pen
    // names, one at a time: the one cut off by a restart first, then Pro and administrators before
    // Free, each in the order asked.
    let ask = |question: &str, public: bool, cookie: &str| {
        http.post(format!("{base}/api/research"))
            .header("cookie", cookie)
            .header("origin", &origin)
            .json(&json!({ "question": question, "public": public }))
            .send()
    };
    let answer = |request: reqwest::RequestBuilder| async move {
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json::<Value>().await.unwrap_or_default())
    };
    let asked = |question: &'static str, public: bool, cookie: String| {
        answer(
            http.post(format!("{base}/api/research"))
                .header("cookie", cookie)
                .header("origin", &origin)
                .json(&json!({ "question": question, "public": public })),
        )
    };
    let listed = |cookie: String| {
        answer(
            http.get(format!("{base}/api/research"))
                .header("cookie", cookie),
        )
    };
    let reader = |name: &'static str| {
        let state = &state;
        async move {
            let (id, cookie) = account(state, "user").await;
            sqlx::query("INSERT INTO authors (user_id, name) VALUES ($1, $2)")
                .bind(id)
                .bind(name)
                .execute(&state.pool)
                .await
                .unwrap();
            (id, cookie)
        }
    };
    let counted = |id: i64| {
        sqlx::query_scalar::<_, i16>("SELECT research FROM authors WHERE user_id = $1")
            .bind(id)
            .fetch_one(&state.pool)
    };
    // Until XABC writes no research.
    let idle = || {
        let state = &state;
        async move {
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                while state
                    .in_flight()
                    .iter()
                    .any(|job| job.starts_with("__research__"))
                {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
        }
    };
    // XABC writes the first question in the queue, and the test waits until it is done.
    let researched = || async {
        research::tick(&state).await;
        idle().await;
    };
    // A question needs a session and a pen name.
    assert_eq!(
        http.post(format!("{base}/api/research"))
            .header("origin", &origin)
            .json(&json!({ "question": "怎么用", "public": true }))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let (_, nameless) = account(&state, "user").await;
    assert_eq!(ask("怎么用", true, &nameless).await.unwrap().status(), 409);
    let (free, free_cookie) = reader("小林").await;
    let (other, other_cookie) = reader("阿青").await;
    let (pro, pro_cookie) = reader("老王").await;
    sqlx::query("UPDATE users SET pro_until = now() + INTERVAL '1 day' WHERE id = $1")
        .bind(pro)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO authors (user_id, name) VALUES ($1, '站长')")
        .bind(administrator_id)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(ask(" ", true, &free_cookie).await.unwrap().status(), 400);
    // Free asks twice a day, and only in public.
    let (status, refused) = asked("SSH 怎么免密登录", false, free_cookie.clone()).await;
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (403, Some("不公开是 Pro 功能"))
    );
    let (status, first) = asked(" SSH 怎么免密登录 ", true, free_cookie.clone()).await;
    assert_eq!(
        (status, &first["ahead"], &first["minutes"]),
        (200, &json!(0), &json!(8))
    );
    let (_, failing_question) = asked("写不出的问题", true, free_cookie.clone()).await;
    assert_eq!(failing_question["ahead"], 1);
    let (status, refused) = asked("第三个问题", true, free_cookie.clone()).await;
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (429, Some("今天的 2 次调研已用完，明天再来吧"))
    );
    // The AI turns a question away before it waits, and the allowance comes back.
    let (status, refused) = asked("澳门赌场怎么玩", true, other_cookie.clone()).await;
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (400, Some("这个问题不能调研：涉及赌博"))
    );
    assert_eq!(counted(other).await.unwrap(), 0);
    // Pro and an administrator wait before Free, each in the order asked; Pro keeps its own.
    let (_, kept) = asked("Pro 的问题", false, pro_cookie.clone()).await;
    assert_eq!(kept["ahead"], 0);
    let (_, by_admin) = asked("站长的问题", true, administrator.clone()).await;
    assert_eq!(by_admin["ahead"], 1);
    // The day the allowance counted a question on stays with it; an administrator's has none.
    let charged = |id: &Value| {
        sqlx::query_scalar::<_, Option<chrono::NaiveDate>>("SELECT day FROM research WHERE id = $1")
            .bind(Uuid::parse_str(id.as_str().unwrap()).unwrap())
            .fetch_one(&state.pool)
    };
    assert_eq!(
        charged(&first["id"]).await.unwrap(),
        Some(state.digest.today())
    );
    assert_eq!(charged(&by_admin["id"]).await.unwrap(), None);
    let (_, mine) = listed(free_cookie.clone()).await;
    assert_eq!(
        (
            &mine["private"],
            &mine["limit"],
            &mine["left"],
            mine["queue"].as_array().unwrap().len()
        ),
        (&json!(false), &json!(2), &json!(0), 2)
    );
    assert_eq!(
        (
            &mine["queue"][0]["question"],
            &mine["queue"][0]["ahead"],
            &mine["queue"][0]["public"],
            &mine["queue"][0]["mine"]
        ),
        (
            &json!("SSH 怎么免密登录"),
            &json!(2),
            &json!(true),
            &json!(true)
        )
    );
    let (_, everyone) = listed(administrator.clone()).await;
    let line: Vec<(&str, &Value, &Value)> = everyone["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|asked| {
            (
                asked["author"].as_str().unwrap(),
                &asked["pro"],
                &asked["ahead"],
            )
        })
        .collect();
    assert_eq!(
        line,
        [
            ("老王", &json!(true), &json!(0)),
            ("站长", &json!(true), &json!(1)),
            ("小林", &json!(false), &json!(2)),
            ("小林", &json!(false), &json!(3)),
        ]
    );
    assert_eq!(
        (&everyone["left"], &everyone["private"]),
        (&Value::Null, &json!(true))
    );
    // A day allowing none allows none, even the first of a new day.
    sqlx::query("UPDATE authors SET day = NULL WHERE user_id = $1")
        .bind(pro)
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(matches!(
        take(&state.pool, pro, state.digest.today(), Quota::Research(0)).await,
        Err(AppError::TooManyRequests(_))
    ));
    sqlx::query("UPDATE authors SET day = $2, research = 1 WHERE user_id = $1")
        .bind(pro)
        .bind(state.digest.today())
        .execute(&state.pool)
        .await
        .unwrap();
    // With Free set to none, 调研 is Pro's.
    let mut closed = state.clone();
    closed.config = Arc::new(Config {
        research_free_daily: 0,
        ..(*state.config).clone()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::web::router(closed);
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (status, refused) = answer(
        http.post(format!("{closed_base}/api/research"))
            .header("cookie", &other_cookie)
            .header("origin", &origin)
            .json(&json!({ "question": "怎么用", "public": true })),
    )
    .await;
    assert_eq!(
        (status, refused["error"]["message"].as_str()),
        (403, Some("调研是 Pro 功能，升级 Pro 后每天可以调研 8 次"))
    );
    // A reader takes back what waits, and the allowance comes back; someone else's isn't theirs.
    let (_, taken) = asked("不想问了", true, other_cookie.clone()).await;
    let take_back = |id: &Value, cookie: &str| {
        http.delete(format!("{base}/api/research/{}", id.as_str().unwrap()))
            .header("cookie", cookie)
            .header("origin", &origin)
            .send()
    };
    assert_eq!(
        take_back(&taken["id"], &free_cookie)
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(counted(other).await.unwrap(), 1);
    assert_eq!(
        take_back(&taken["id"], &other_cookie)
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(counted(other).await.unwrap(), 0);

    // 调研 writes beside the run slots collection shares, so it goes on with all of them taken.
    let busy = state
        .run_slots
        .clone()
        .acquire_many_owned(state.config.max_concurrent_runs as u32)
        .await
        .unwrap();
    // Pro's question goes first: both scouts look up what it really asks, and the writer makes of
    // it an article under the reader's pen name, as the question's id, kept to the reader.
    let asked_before = asked_log.lock().unwrap().len();
    researched().await;
    drop(busy);
    let requests = asked_log.lock().unwrap()[asked_before..].to_vec();
    assert_eq!(requests[0]["text"]["format"]["name"], "understanding");
    let understood = "读者的问题：Pro 的问题\n对问题的理解：他想弄懂「Pro 的问题」。";
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
                .contains("「调研」栏目提了一个问题")
                && search["input"][0]["content"]
                    .as_str()
                    .unwrap()
                    .ends_with(understood)
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
            .contains("这一篇回答一位读者的问题")
    );
    let article = |id: &Value| {
        sqlx::query_as::<_, (i64, String, Option<String>, Option<Uuid>, String)>(
            "SELECT author_id, status, subject, cover, body FROM articles WHERE id = $1",
        )
        .bind(Uuid::parse_str(id.as_str().unwrap()).unwrap())
        .fetch_one(&state.pool)
    };
    let (author_id, status, question, cover, body) = article(&kept["id"]).await.unwrap();
    assert_eq!(
        (author_id, status.as_str(), question.as_deref()),
        (pro, "draft", Some("Pro 的问题"))
    );
    assert!(cover.is_some());
    // A page read to understand the question keeps its link.
    assert!(
        body.contains("看看[别处](https://elsewhere.example/x)"),
        "{body}"
    );
    // An administrator's goes out at once, and a reader's after the AI reviews it.
    researched().await;
    assert_eq!(article(&by_admin["id"]).await.unwrap().1, "published");
    researched().await;
    assert_eq!(article(&first["id"]).await.unwrap().1, "published");
    let shown = page(format!("{base}/articles/{}", first["id"].as_str().unwrap())).await;
    assert!(
        shown.contains("<p class=research-question><b>问</b><span>SSH 怎么免密登录</span></p>")
    );
    assert!(shown.contains(&format!("<a href=\"/research?by={free}\">小林</a>")));
    assert!(shown.contains("<a href=\"/research\">← 调研</a>"));
    assert!(shown.contains("<a class=tab href=\"/research\" aria-current=\"true\">调研</a>"));
    let listing = page(format!("{base}/research")).await;
    assert!(
        listing.contains("<form class=research-ask>") && listing.contains("搜索调研的标题和内容")
    );
    assert!(listing.contains(&format!("/articles/{}", first["id"].as_str().unwrap())));
    assert!(!listing.contains(&format!("/articles/{}", kept["id"].as_str().unwrap())));
    assert!(
        page(format!("{base}/research?by={free}"))
            .await
            .contains("小林 的调研")
    );
    assert!(
        !page(format!("{base}/articles"))
            .await
            .contains(first["id"].as_str().unwrap())
    );
    assert!(
        page(format!("{base}/articles/sitemap.xml"))
            .await
            .contains("/research</loc>")
    );
    // The words are XABC's: its reader can't save over them.
    let saved = http
        .patch(format!(
            "{base}/api/articles/{}",
            first["id"].as_str().unwrap()
        ))
        .header("cookie", &free_cookie)
        .header("origin", &origin)
        .json(&json!({ "revision": 1, "title": "改了", "body": "改了", "cover": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 404);
    // A question the AI couldn't look at is reviewed with its article, shown on the page with it.
    let unchecked = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO research (id, author_id, question, pro, public, day)
         VALUES ($1, $2, '澳门赌场的问题', false, true, $3)",
    )
    .bind(unchecked)
    .bind(other)
    .bind(state.digest.today())
    .execute(&state.pool)
    .await
    .unwrap();
    // A question XABC can't write is marked, not paid for twice, and its allowance given back;
    // its reader clears it.
    let asked_before = asked_log.lock().unwrap().len();
    researched().await;
    let understandings = asked_log.lock().unwrap()[asked_before..]
        .iter()
        .filter(|request| request["text"]["format"]["name"] == "understanding")
        .count();
    assert_eq!(understandings, 1);
    let (_, mine) = listed(free_cookie.clone()).await;
    assert_eq!(
        (
            &mine["queue"][0]["failed"],
            &mine["queue"][0]["ahead"],
            &mine["left"]
        ),
        (&json!(true), &Value::Null, &json!(1))
    );
    assert_eq!(
        take_back(&failing_question["id"], &free_cookie)
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(counted(free).await.unwrap(), 1);
    researched().await;
    let (status, held): (String, Option<String>) =
        sqlx::query_as("SELECT status, held FROM articles WHERE id = $1")
            .bind(unchecked)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), held.as_deref()),
        ("pending", Some("AI：涉及赌博"))
    );
    // Pro switches its research public, through the AI's review, and back; Free can't.
    let publicize = |id: &Value, public: bool, cookie: &str| {
        answer(
            http.patch(format!("{base}/api/research/{}", id.as_str().unwrap()))
                .header("cookie", cookie)
                .header("origin", &origin)
                .json(&json!({ "public": public })),
        )
    };
    assert_eq!(publicize(&first["id"], false, &free_cookie).await.0, 403);
    assert_eq!(
        publicize(&kept["id"], true, &pro_cookie).await,
        (200, json!({ "status": "published" }))
    );
    assert_eq!(
        publicize(&kept["id"], false, &pro_cookie).await,
        (200, json!({ "status": "draft" }))
    );
    assert_eq!(publicize(&kept["id"], false, &pro_cookie).await.0, 409);
    // Each switch is a revision of its own, so a review of an earlier one publishes nothing.
    let revision: i32 = sqlx::query_scalar("SELECT revision FROM articles WHERE id = $1")
        .bind(Uuid::parse_str(kept["id"].as_str().unwrap()).unwrap())
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(revision, 3);
    // Its reader previews it without the editor: the words are XABC's.
    let preview = http
        .get(format!(
            "{base}/api/articles/{}/preview",
            kept["id"].as_str().unwrap()
        ))
        .header("cookie", &pro_cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(preview.contains("预览 · 草稿") && !preview.contains("继续编辑"));
    // What waits switches too, and an administrator sees the reader's choice.
    let (_, waiting) = asked("还没写的问题", true, pro_cookie.clone()).await;
    assert_eq!(
        publicize(&waiting["id"], false, &pro_cookie).await,
        (200, json!({ "public": false }))
    );
    let (_, everyone) = listed(administrator.clone()).await;
    assert_eq!(
        (
            &everyone["queue"][0]["public"],
            &everyone["queue"][0]["mine"]
        ),
        (&json!(false), &json!(false))
    );
    let kept_row = everyone["articles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|article| article["id"] == kept["id"])
        .unwrap();
    assert_eq!(
        (&kept_row["status"], &kept_row["pro"], &kept_row["author"]),
        (&json!("draft"), &json!(true), &json!("老王"))
    );
    // A question cut off by a restart is written first again, ahead of Pro.
    let (_, cut) = asked("重启前的问题", true, other_cookie.clone()).await;
    sqlx::query("UPDATE research SET started_at = now() WHERE id = $1")
        .bind(Uuid::parse_str(cut["id"].as_str().unwrap()).unwrap())
        .execute(&state.pool)
        .await
        .unwrap();
    let (_, everyone) = listed(administrator.clone()).await;
    assert_eq!(
        (
            &everyone["queue"][0]["question"],
            &everyone["queue"][0]["writing"],
            &everyone["queue"][0]["ahead"],
            &everyone["queue"][1]["ahead"],
            &everyone["queue"][1]["now"],
            &everyone["queue"][1]["minutes"]
        ),
        (
            &json!("重启前的问题"),
            &json!(false),
            &json!(0),
            &json!(1),
            &json!(false),
            &json!(16)
        )
    );
    researched().await;
    assert_eq!(article(&cut["id"]).await.unwrap().1, "published");
    researched().await;
    assert_eq!(article(&waiting["id"]).await.unwrap().1, "draft");
    // Several are written at once, as many as XABC may, Pro and administrators first; one that
    // waits is told how many wait before it, whether it starts at once and about when it is done.
    let mut side_by_side = state.clone();
    side_by_side.config = Arc::new(Config {
        max_concurrent_research: 2,
        ..(*state.config).clone()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let wide = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::web::router(side_by_side.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let ask_wide = |question: &'static str, cookie: String| {
        answer(
            http.post(format!("{wide}/api/research"))
                .header("cookie", cookie)
                .header("origin", &origin)
                .json(&json!({ "question": question, "public": true })),
        )
    };
    let (_, free_one) = ask_wide("并行的问题一", other_cookie.clone()).await;
    assert_eq!(
        (&free_one["ahead"], &free_one["now"], &free_one["minutes"]),
        (&json!(0), &json!(true), &json!(8))
    );
    let (_, admin_one) = ask_wide("并行的问题二", administrator.clone()).await;
    let (_, pro_one) = ask_wide("并行的问题三", pro_cookie.clone()).await;
    assert_eq!(
        (&pro_one["ahead"], &pro_one["now"]),
        (&json!(1), &json!(true))
    );
    let standings = || async {
        let (_, everyone) = answer(
            http.get(format!("{wide}/api/research"))
                .header("cookie", &administrator),
        )
        .await;
        assert_eq!(everyone["writers"], 2);
        everyone["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|asked| {
                (
                    asked["question"].as_str().unwrap().to_owned(),
                    asked["ahead"].clone(),
                    asked["now"].clone(),
                    asked["minutes"].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        standings().await,
        [
            ("并行的问题二".into(), json!(0), json!(true), json!(8)),
            ("并行的问题三".into(), json!(1), json!(true), json!(8)),
            ("并行的问题一".into(), json!(2), json!(false), json!(16)),
        ]
    );
    // One tick starts the two it may, written side by side, held at the gate; Free's waits behind
    // them, told it comes next, in about two rounds.
    research::tick(&side_by_side).await;
    // Both reach the gate before either goes on.
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while asked_log
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                request["text"]["format"]["name"] == "understanding"
                    && request["input"][0]["content"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("并行的问题")
            })
            .count()
            < 2
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let researching = || {
        state
            .in_flight()
            .iter()
            .filter(|job| job.starts_with("__research__"))
            .count()
    };
    assert_eq!(researching(), 2);
    assert_eq!(
        standings().await,
        [
            ("并行的问题二".into(), Value::Null, Value::Null, Value::Null),
            ("并行的问题三".into(), Value::Null, Value::Null, Value::Null),
            ("并行的问题一".into(), json!(0), json!(false), json!(16)),
        ]
    );
    let (_, everyone) = answer(
        http.get(format!("{wide}/api/research"))
            .header("cookie", &administrator),
    )
    .await;
    let flags: Vec<&Value> = everyone["queue"]
        .as_array()
        .unwrap()
        .iter()
        .map(|asked| &asked["writing"])
        .collect();
    assert_eq!(
        (&everyone["writing"], flags),
        (&json!(2), vec![&json!(true), &json!(true), &json!(false)])
    );
    gate.add_permits(3);
    idle().await;
    assert_eq!(
        standings().await,
        [("并行的问题一".into(), json!(0), json!(true), json!(8))]
    );
    research::tick(&side_by_side).await;
    idle().await;
    for asked in [&admin_one, &pro_one, &free_one] {
        assert!(article(&asked["id"]).await.is_ok());
    }
    // A question this process writes isn't started again beside it; one marked started that no
    // one here writes, cut off by a restart, is.
    let (_, busy) = ask_wide("正在写的问题", administrator.clone()).await;
    let (_, after) = ask_wide("接着写的问题", administrator.clone()).await;
    let busy_id = Uuid::parse_str(busy["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE research SET started_at = now() WHERE id = $1")
        .bind(busy_id)
        .execute(&state.pool)
        .await
        .unwrap();
    let mark = format!("__research__{busy_id}");
    state.in_flight().insert(mark.clone());
    research::tick(&side_by_side).await;
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while researching() > 1 || article(&after["id"]).await.is_err() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(article(&busy["id"]).await.is_err());
    state.in_flight().remove(&mark);
    research::tick(&side_by_side).await;
    idle().await;
    assert!(article(&busy["id"]).await.is_ok());

    // 追踪: the event both scouts name goes first, under the key its slug makes, in an article that
    // shows no drawn picture. Showing none at all, it takes for the lists the picture the finds'
    // page shares.
    let tracker =
        |state: AppState| async move { xabc::write(&state, xabc::Line::Tracker, Utc::now()).await };
    let drawn_before = drawings_log.lock().unwrap().len();
    let followed = tracker(state.clone()).await.unwrap().unwrap();
    let (event, title, body, cover): (Option<String>, String, String, Option<String>) =
        sqlx::query_as(
            "SELECT a.subject, a.title, a.body, i.url FROM articles a
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
    let event: Option<String> = sqlx::query_scalar("SELECT subject FROM articles WHERE id = $1")
        .bind(selloff)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(event.as_deref(), Some("ai-capex-selloff-2026-10"));
    assert_eq!(tracker(state.clone()).await.unwrap(), None);
    let heard = searched();
    assert!(heard.starts_with("现在是 ") && heard.contains("这一轮按热度找事件"));
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
         WHERE kind = 'event'",
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
         WHERE kind = 'event'",
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
    // 精选 opens with XABC's 分享 alone, the newest first, ranks 调研 with the rest, and closes
    // with the two articles that head 追踪's list, then the two that head 时代's: however well
    // read, they show there and not among the hottest.
    sqlx::query("UPDATE articles SET views = 80 WHERE id = $1 OR id = $2")
        .bind(selloff)
        .bind(Uuid::parse_str(first["id"].as_str().unwrap()).unwrap())
        .execute(&state.pool)
        .await
        .unwrap();
    let shared: Uuid = sqlx::query_scalar(
        "SELECT id FROM articles WHERE author_id = $1 AND kind = 'share'
         ORDER BY published_at DESC LIMIT 1",
    )
    .bind(author)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let worth: Value =
        serde_json::from_str(&page(format!("{base}/articles/worth.json")).await).unwrap();
    let worth = worth["articles"].as_array().unwrap();
    assert_eq!(worth.len(), 12);
    assert_eq!(worth[0]["id"], shared.to_string());
    let (ranked, heads) = worth.split_at(8);
    assert!(
        ranked
            .iter()
            .any(|article| { article["id"] == first["id"] && article["kind"] == "question" })
    );
    assert!(ranked.iter().all(|article| article["kind"] != "event"));
    assert_eq!(
        heads
            .iter()
            .map(|article| article["id"].clone())
            .collect::<Vec<_>>(),
        [selloff, followed, firm, satya].map(|id| id.to_string())
    );

    // An administrator names events for 追踪, which lines them up, each once, and follows them
    // one after another before its own rounds: anew, or in the article an event has however
    // lately that was brought up to date, but never in one taken back.
    let name_event = |name: &str| {
        http.post(format!("{base}/api/admin/tracker"))
            .header("cookie", &administrator)
            .header("origin", &origin)
            .json(&json!({ "name": name }))
            .send()
    };
    let event_ahead = |event: &'static str| {
        let request = name_event(event);
        async move { request.await.unwrap().json::<Value>().await.unwrap()["ahead"].clone() }
    };
    let next_event = || {
        let state = &state;
        async move {
            xabc::start(state, xabc::Line::Tracker).await;
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                while state.in_flight().contains("__xabc_tracker__") {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap();
        }
    };
    let standing_of = |id: Uuid| {
        let state = state.clone();
        async move {
            sqlx::query_as::<_, (i32, DateTime<Utc>, String)>(
                "SELECT revision, updated_at, status FROM articles WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(name_event(" ").await.unwrap().status(), 400);
    state.in_flight().insert("__xabc_tracker__".into());
    assert_eq!(event_ahead("ai-capex-selloff-2026-10").await, 0);
    assert_eq!(event_ahead("查无此事").await, 1);
    assert_eq!(event_ahead("Chip export probe 2026-11").await, 2);
    assert_eq!(event_ahead("binance-hack-2026-10").await, 3);
    assert_eq!(event_ahead(" ai-capex-selloff-2026-10 ").await, 0);
    xabc::start(&state, xabc::Line::Tracker).await;
    state.in_flight().remove("__xabc_tracker__");
    // The event brought up to date a moment ago is brought up to date again. The scouts hear
    // what was named after the events followed, and nothing of the round's own demands: not of
    // heat, nor of technology alone.
    let (revision, ..) = standing_of(selloff).await;
    next_event().await;
    assert_eq!(standing_of(selloff).await.0, revision + 1);
    let heard = searched();
    assert!(heard.contains("正在追踪的事件（标识｜标题｜上次更新）："));
    assert!(heard.contains("只追踪管理员点名的这一件事"));
    assert!(heard.ends_with("点名：ai-capex-selloff-2026-10"), "{heard}");
    let told = asked_log
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|request| request["text"]["format"]["name"] == "finds")
        .unwrap()["instructions"]
        .as_str()
        .unwrap()
        .to_owned();
    for demand in ["这一轮按热度找事件", "只收科技事件", "没有新进展就留空"]
    {
        assert!(
            !heard.contains(demand) && !told.contains(demand),
            "{demand}"
        );
    }
    // An event the scouts cannot find is dropped, and the next goes on though 追踪 failed: an
    // event not followed yet, written about anew though an article was brought up to date a
    // moment ago.
    let events = || {
        let state = state.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM articles WHERE kind = 'event'")
                .fetch_one(&state.pool)
                .await
                .unwrap()
        }
    };
    let before = events().await;
    next_event().await;
    assert_eq!(events().await, before);
    next_event().await;
    let (title, status): (String, String) = sqlx::query_as(
        "SELECT title, status FROM articles WHERE kind = 'event' AND subject = 'chip-export-probe-2026-11'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        (title.as_str(), status.as_str()),
        ("chip-export-probe-2026-11 的追踪", "published")
    );
    // An event whose article an administrator took back stays as it is, though named.
    sqlx::query("UPDATE articles SET status = 'pending' WHERE id = $1")
        .bind(followed)
        .execute(&state.pool)
        .await
        .unwrap();
    let before = standing_of(followed).await;
    next_event().await;
    assert_eq!(standing_of(followed).await, before);
    assert!(searched().ends_with("点名：binance-hack-2026-10"));
    // Followed again but last brought up to date months ago, it is named in other words and
    // lands in its article all the same: the scouts hear of every event ever followed.
    sqlx::query(
        "UPDATE articles SET status = 'published', updated_at = now() - INTERVAL '40 days'
         WHERE id = $1",
    )
    .bind(followed)
    .execute(&state.pool)
    .await
    .unwrap();
    let (revision, ..) = standing_of(followed).await;
    let before = events().await;
    name_event("币安被盗").await.unwrap();
    next_event().await;
    assert!(searched().contains("\n- binance-hack-2026-10｜"));
    assert_eq!(
        (standing_of(followed).await.0, events().await),
        (revision + 1, before)
    );

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
    let event = Some((Kind::Event, "guard-2026-10"));
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
        Ok(None)
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
        sqlx::query_as("SELECT subject, title, body, cover FROM articles WHERE id = $1")
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
        Ok(None)
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
        Ok(None)
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

    // XAI writes 分享 on what X's tech circle talks about most, from its own brief and under its
    // own pen name: its scouts hear what both pen names shared lately, and a find about a page
    // XABC linked to is left out. It goes by its own clock, so XABC's 分享 just out holds it
    // back no more than its own would XABC, and 精选 opens with the newest of either pen name's.
    let asked_before = asked_log.lock().unwrap().len();
    let talked = xabc::write(
        &state,
        xabc::Line::Trending,
        shared - chrono::Duration::seconds(1),
    )
    .await
    .unwrap()
    .unwrap();
    let (title, name): (String, String) = sqlx::query_as(
        "SELECT a.title, w.name FROM articles a JOIN authors w ON w.user_id = a.author_id
         WHERE a.id = $1 AND a.status = 'published'",
    )
    .bind(talked)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        (title.as_str(), name.as_str()),
        ("大家都在聊的一件事", "XAI")
    );
    let requests = asked_log.lock().unwrap()[asked_before..].to_vec();
    let searches: Vec<&Value> = requests
        .iter()
        .filter(|request| request["text"]["format"]["name"] == "finds")
        .collect();
    assert_eq!(searches.len(), 2);
    assert!(searches.iter().all(|search| {
        search["instructions"]
            .as_str()
            .unwrap()
            .contains("你是 XAI 的选题编辑")
            && search["input"][0]["content"]
                .as_str()
                .unwrap()
                .contains("- 今天分享一个好玩的工具\n")
    }));
    let material = writing(&requests);
    assert!(material.contains("1.【X 与全网】大家都在聊的事"));
    assert!(!material.contains("】一个好玩的工具"));
    assert_eq!(
        xabc::last(&state, xabc::Line::Share).await.unwrap(),
        Some(shared)
    );
    assert!(
        xabc::last(&state, xabc::Line::Trending)
            .await
            .unwrap()
            .is_some_and(|at| at > shared)
    );
    let worth: Value =
        serde_json::from_str(&page(format!("{base}/articles/worth.json")).await).unwrap();
    assert_eq!(worth["articles"][0]["id"], talked.to_string());
    // XABC hears what XAI shared, too.
    let asked_before = asked_log.lock().unwrap().len();
    assert!(matches!(
        xabc::write(&state, xabc::Line::Share, Utc::now()).await,
        Err(AppError::Internal(message)) if message.contains("最近写过")
    ));
    let searches = asked_log.lock().unwrap()[asked_before..].to_vec();
    assert_eq!(searches.len(), 2);
    assert!(searches.iter().all(|search| {
        search["input"][0]["content"]
            .as_str()
            .unwrap()
            .contains("- 大家都在聊的一件事\n")
    }));
}

/// The Codex & Claude handbook from start to finish against stand-ins for the AI service and the
/// official documentation: every chapter written in one round, the one whose code named an option
/// the documentation lacks written again, and none drawn; nothing due the round after; a chapter
/// whose official pages changed brought up to date, then left as it is when the writer finds
/// nothing to change; a chapter the topic's news piles up on brought up to date; the handbook's
/// page, chapters, book and sitemap while its topic is shown, and nothing once it is hidden; and
/// no writing while the topic collects nothing.
#[tokio::test]
#[ignore = "requires NEWS_HANDBOOK_TEST_DATABASE_URL and local test configuration"]
async fn postgres_handbook_writes_checks_and_serves() {
    use crate::{
        articles::handbook::{CHAPTERS, TOPIC},
        config::Config,
    };
    use axum::routing::post;
    use sha2::{Digest, Sha256};
    use std::sync::Mutex;
    let database_url =
        std::env::var("NEWS_HANDBOOK_TEST_DATABASE_URL").expect("disposable database URL");
    let asked: Arc<Mutex<Vec<Value>>> = Arc::default();
    // Whether the writer, bringing a chapter up to date, finds anything to change.
    let changes = Arc::new(AtomicBool::new(true));
    // Whether the writer, told to check what the official pages lack, comes back empty.
    let blank = Arc::new(AtomicBool::new(true));
    // Whether an administrator saves the Hooks chapter while the writer checks it.
    let saving = Arc::new(AtomicBool::new(false));
    let admin = sqlx::PgPool::connect_lazy(&database_url).unwrap();
    let reply = |text: Value| {
        json!({
            "id": "resp_handbook", "status": "completed",
            "output": [{ "type": "message", "content": [{
                "type": "output_text", "text": text.to_string(), "annotations": [],
            }] }],
            "usage": { "input_tokens": 100, "output_tokens": 50 },
        })
    };
    let ai = axum::Router::new().route(
        "/v1/responses",
        post({
            let (asked, changes, blank, saving) =
                (asked.clone(), changes.clone(), blank.clone(), saving.clone());
            move |Json(request): Json<Value>| {
                let (asked, changes, blank, saving, admin) = (
                    asked.clone(),
                    changes.clone(),
                    blank.clone(),
                    saving.clone(),
                    admin.clone(),
                );
                async move {
                    asked.lock().unwrap().push(request.clone());
                    if request["text"]["format"]["name"] == "finds" {
                        return Json(reply(json!({ "finds": [{
                            "title": "钩子的用法", "why": "有用", "points": "退出码 2 拦住动作",
                            "sources": ["https://x.com/dev/status/1"], "pictures": [],
                        }] })));
                    }
                    let input = request["input"][0]["content"].as_str().unwrap_or_default();
                    let revising = request["instructions"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("这次是更新");
                    // A new Hooks chapter names an option no official page has, until it is told.
                    let hooks = input.contains("这一章：「Hooks」");
                    let checking = input.contains("都找不到");
                    let odd = hooks && !revising && !checking;
                    if hooks && revising && saving.load(Ordering::SeqCst) {
                        sqlx::query(
                            "UPDATE articles SET revision = revision + 1 WHERE kind = 'chapter' AND subject = 'hooks'",
                        )
                        .execute(&admin)
                        .await
                        .unwrap();
                    }
                    let body = if (revising && !changes.load(Ordering::SeqCst))
                        || (checking && blank.load(Ordering::SeqCst))
                    {
                        String::new()
                    } else {
                        format!(
                            "{}\n\n```bash\nclaude -p \"总结\" --output-format json{}\n```\n\n看[官方文档](https://code.claude.com/docs/zh-CN/hooks)和[别处](https://elsewhere.example/x)。",
                            if revising { "更新后的内容，" } else { "这一章的内容，" }.repeat(80),
                            if odd { " --made-up" } else { "" }
                        )
                    };
                    Json(reply(json!({
                        "title": "写作给的标题", "body": body,
                        "pictures": [{ "page": "", "prompt": "一幅插图", "caption": "图" }],
                        "cover": "封面",
                    })))
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
    let (media, client, _) = tmpfile(jpeg(400, 300)).await;
    config.tmpfile_base_url = media.parse().unwrap();
    config.tmpfile_api_key = "fixture".into();
    let mut state = AppState::new(config).await.unwrap();
    // The official pages are read through the same local server.
    state.origin_http = client.clone();
    state.http = client;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_handbook_test"));
    sqlx::query("TRUNCATE articles, article_images CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO topics (id, name, name_en, brief) VALUES ($1, 'Codex & Claude', 'Codex & Claude', 'Claude Code 与 Codex')
         ON CONFLICT (id) DO UPDATE SET enabled = true, visible = true",
    )
    .bind(TOPIC)
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM items WHERE topic_id = $1")
        .bind(TOPIC)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    HOOKS_EDITION.store(0, Ordering::SeqCst);
    let line = xabc::Line::Handbook;
    let author = xabc::author(&state, line).await.unwrap();
    assert_eq!(xabc::last(&state, line).await.unwrap(), None);

    // A round writes the chapters in order until the writer, told to check the Hooks chapter's
    // option the official pages lack, comes back empty: that draft doesn't go out.
    assert!(matches!(
        xabc::write(&state, line, Utc::now()).await,
        Err(AppError::Internal(message)) if message.contains("核对后的正文是空的")
    ));
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM articles WHERE kind = 'chapter'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        before as usize,
        CHAPTERS
            .iter()
            .position(|chapter| chapter.key == "hooks")
            .unwrap()
    );
    // The next round writes the rest under their own titles, without pictures, each checked
    // against all its official pages; the Hooks chapter is written again without the option.
    blank.store(false, Ordering::SeqCst);
    let last = xabc::write(&state, line, Utc::now())
        .await
        .unwrap()
        .unwrap();
    #[derive(sqlx::FromRow)]
    struct Chapter {
        id: Uuid,
        chapter: String,
        title: String,
        body: String,
        status: String,
        revision: i32,
        author_id: i64,
        sources: Value,
        checked_at: DateTime<Utc>,
    }
    let chapters = || async {
        sqlx::query_as::<_, Chapter>(
            "SELECT id, subject AS chapter, title, body, status, revision, author_id, sources,
                    checked_at
             FROM articles WHERE kind = 'chapter'",
        )
        .fetch_all(&state.pool)
        .await
        .unwrap()
    };
    let written = chapters().await;
    assert_eq!(written.len(), CHAPTERS.len());
    for chapter in &CHAPTERS {
        let article = written
            .iter()
            .find(|article| article.chapter == chapter.key)
            .unwrap();
        assert_eq!(
            (
                article.title.as_str(),
                article.status.as_str(),
                article.revision
            ),
            (chapter.title, "published", 1)
        );
        assert_eq!(article.author_id, author);
        assert_eq!(
            article.sources.as_object().unwrap().len(),
            chapter.pages.len(),
            "{}",
            chapter.key
        );
    }
    assert_eq!(
        last,
        written
            .iter()
            .find(|article| article.chapter == "sdk")
            .unwrap()
            .id
    );
    let hooks = written
        .iter()
        .find(|article| article.chapter == "hooks")
        .unwrap();
    assert!(hooks.body.contains("--output-format json\n```"));
    assert!(!hooks.body.contains("--made-up"));
    assert!(
        hooks
            .body
            .contains("[官方文档](https://code.claude.com/docs/zh-CN/hooks)")
    );
    assert!(hooks.body.contains("和别处。"));
    let requests = asked.lock().unwrap().clone();
    let drafts: Vec<&str> = requests
        .iter()
        .filter(|request| request["text"]["format"]["name"] == "article")
        .map(|request| request["input"][0]["content"].as_str().unwrap())
        .collect();
    // Every chapter's draft, and the Hooks chapter's first draft and its check twice.
    assert_eq!(drafts.len(), CHAPTERS.len() + 3);
    assert!(drafts.iter().any(|input| {
        input.contains("这一章：「Hooks」") && input.contains("不是能解析的 JSON：--made-up")
    }));
    assert!(
        drafts
            .iter()
            .all(|input| input.contains("官方文档原文（以此为准）"))
    );
    let pictures: i64 = sqlx::query_scalar("SELECT count(*) FROM article_images")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(pictures, 0);
    let checked = written.iter().map(|article| article.checked_at).max();
    assert_eq!(xabc::last(&state, line).await.unwrap(), checked);

    // Nothing changed: the next round leaves every chapter as it is.
    assert_eq!(xabc::write(&state, line, Utc::now()).await.unwrap(), None);
    assert!(chapters().await.iter().all(|article| article.revision == 1));
    // Nor when a page is gone and its site answers with HTML called Markdown: it isn't read, and
    // the fingerprint noted stays.
    HOOKS_EDITION.store(9, Ordering::SeqCst);
    assert_eq!(xabc::write(&state, line, Utc::now()).await.unwrap(), None);
    let standing = chapters().await;
    assert!(standing.iter().all(|article| article.revision == 1));
    assert_eq!(
        standing
            .iter()
            .find(|article| article.chapter == "hooks")
            .unwrap()
            .sources,
        hooks.sources
    );
    HOOKS_EDITION.store(0, Ordering::SeqCst);
    // A chapter the book gains, as DeepSeek's was, is written in the next round, and the others
    // stay as they were.
    sqlx::query("DELETE FROM articles WHERE kind = 'chapter' AND subject = 'deepseek'")
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(xabc::last(&state, line).await.unwrap(), None);
    let gained = xabc::write(&state, line, Utc::now())
        .await
        .unwrap()
        .unwrap();
    let after = chapters().await;
    assert_eq!(after.len(), CHAPTERS.len());
    for article in &after {
        let before = standing
            .iter()
            .find(|before| before.chapter == article.chapter)
            .unwrap();
        if article.chapter == "deepseek" {
            assert_eq!(article.id, gained);
        } else {
            assert_eq!(
                (
                    article.id,
                    article.revision,
                    article.checked_at,
                    &article.sources
                ),
                (
                    before.id,
                    before.revision,
                    before.checked_at,
                    &before.sources
                )
            );
        }
    }

    // The official pages on hooks change: their chapter is brought up to date, told which.
    HOOKS_EDITION.store(1, Ordering::SeqCst);
    let before = asked.lock().unwrap().len();
    assert_eq!(
        xabc::write(&state, line, Utc::now()).await.unwrap(),
        Some(hooks.id)
    );
    let revised = chapters()
        .await
        .into_iter()
        .find(|article| article.chapter == "hooks")
        .unwrap();
    assert_eq!(revised.revision, 2);
    assert!(revised.body.starts_with("更新后的内容，"));
    assert_ne!(revised.sources, hooks.sources);
    let revision = asked.lock().unwrap()[before..].to_vec();
    assert_eq!(revision.len(), 1);
    let input = revision[0]["input"][0]["content"].as_str().unwrap();
    assert!(input.contains("hooks-guide】（自上次核对以来有改动）"));
    assert!(input.contains("现有全文：\n标题：Hooks"));

    // They change again, and the writer finds nothing to change, but an administrator saved the
    // chapter meanwhile: nothing is noted, and the next round checks it again. Then, with the
    // writer still finding nothing to change, only the check moves.
    HOOKS_EDITION.store(2, Ordering::SeqCst);
    changes.store(false, Ordering::SeqCst);
    saving.store(true, Ordering::SeqCst);
    assert_eq!(xabc::write(&state, line, Utc::now()).await.unwrap(), None);
    saving.store(false, Ordering::SeqCst);
    let saved = chapters()
        .await
        .into_iter()
        .find(|article| article.chapter == "hooks")
        .unwrap();
    assert_eq!(
        (saved.revision, saved.checked_at, &saved.sources),
        (3, revised.checked_at, &revised.sources)
    );
    assert_eq!(xabc::write(&state, line, Utc::now()).await.unwrap(), None);
    let kept = chapters()
        .await
        .into_iter()
        .find(|article| article.chapter == "hooks")
        .unwrap();
    assert_eq!((kept.revision, kept.body), (3, revised.body));
    assert!(kept.checked_at > revised.checked_at);
    assert_ne!(kept.sources, revised.sources);

    // News on MCP piles up a day after its chapter's check: that chapter is brought up to date,
    // from the news.
    changes.store(true, Ordering::SeqCst);
    let mcp = written
        .iter()
        .find(|article| article.chapter == "mcp")
        .unwrap();
    sqlx::query("UPDATE articles SET checked_at = now() - INTERVAL '2 days' WHERE id = $1")
        .bind(mcp.id)
        .execute(&state.pool)
        .await
        .unwrap();
    for n in 1..=3 {
        let url = format!("https://x.com/dev/status/{n}");
        sqlx::query(
            "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url)
             VALUES ($1, $2, CURRENT_DATE, $3, $4, '配置写法', $5)",
        )
        .bind(Uuid::new_v4())
        .bind(TOPIC)
        .bind(Sha256::digest(url.as_bytes()).to_vec())
        .bind(format!("Claude Code 的 MCP 服务器配置 {n}"))
        .bind(&url)
        .execute(&state.pool)
        .await
        .unwrap();
    }
    let before = asked.lock().unwrap().len();
    assert_eq!(
        xabc::write(&state, line, Utc::now()).await.unwrap(),
        Some(mcp.id)
    );
    let input = asked.lock().unwrap()[before]["input"][0]["content"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(input.contains("这一章：「MCP」"));
    assert!(input.contains("｜Claude Code 的 MCP 服务器配置 3｜配置写法｜"));

    // The handbook's page lists the chapters by group and the topic's news; a chapter stands at
    // its own address with the contents beside it and copies itself as Markdown.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::web::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let get = |path: String| {
        let request = http.get(format!("{base}{path}"));
        async move {
            let response = request.send().await.unwrap();
            (response.status().as_u16(), response.text().await.unwrap())
        }
    };
    let (status, hub) = get("/agents".into()).await;
    assert_eq!(status, 200);
    for chapter in &CHAPTERS {
        assert!(hub.contains(&format!("href=\"/agents/{}\"", chapter.key)));
    }
    assert!(hub.contains("Claude Code 的 MCP 服务器配置 3"));
    assert!(!hub.contains(".md\""));
    assert!(hub.contains(
        "href=\"/agents\" data-topic=agents aria-current=\"true\">Codex &amp; Claude</a>"
    ));
    let (status, page) = get("/agents/hooks".into()).await;
    assert_eq!(status, 200);
    assert!(page.contains("class=handbook-side"));
    assert!(page.contains("aria-current=\"page\">Hooks</a>"));
    assert!(page.contains(
        "<p class=handbook-crumb><a href=\"/agents\">Codex &amp; Claude</a> · 配置与扩展</p>"
    ));
    assert!(page.contains("<a rel=prev href=\"/agents/extensions\">"));
    assert!(page.contains("<a rel=next href=\"/agents/workflow\">"));
    assert!(page.contains("/agents/hooks\"><link rel=\"alternate\""));
    assert!(page.contains("data-copy-page=\"/agents/hooks.md\" data-copied=\"已复制\""));
    assert_eq!(get("/agents/unknown".into()).await.0, 404);
    // The chapter as Markdown, which 「复制页面」 copies, for agents rather than search engines;
    // the handbook has no whole book.
    let markdown = http
        .get(format!("{base}/agents/hooks.md"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        (
            markdown.status().as_u16(),
            markdown.headers()["content-type"].to_str().unwrap(),
            markdown.headers()["x-robots-tag"].to_str().unwrap(),
        ),
        (200, "text/markdown; charset=utf-8", "noindex")
    );
    let markdown = markdown.text().await.unwrap();
    assert!(markdown.starts_with("# Hooks\n\n> 来源：http"));
    assert!(markdown.contains("/agents/hooks · 更新于 "));
    assert!(markdown.contains("更新后的内容，"));
    assert!(markdown.contains("```bash\nclaude -p \"总结\" --output-format json\n```"));
    assert_eq!(get("/agents/unknown.md".into()).await.0, 404);
    assert_eq!(get("/agents.md".into()).await.0, 404);
    // Chapters are listed at their own addresses, and stay out of 分享 and 精选.
    let (_, sitemap) = get("/articles/sitemap.xml".into()).await;
    assert!(sitemap.contains("/agents/hooks</loc>"));
    assert!(!sitemap.contains(&hooks.id.to_string()));
    let (_, shared) = get("/articles".into()).await;
    assert!(!shared.contains(&hooks.id.to_string()));
    let (_, worth) = get("/articles/worth.json".into()).await;
    assert!(!worth.contains(&hooks.id.to_string()));

    // Hidden, the handbook has no page, chapter, Markdown or place in the sitemap for readers.
    sqlx::query("UPDATE topics SET visible = false WHERE id = $1")
        .bind(TOPIC)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    for path in [
        "/agents".to_owned(),
        "/agents/hooks".to_owned(),
        "/agents/hooks.md".to_owned(),
        format!("/articles/{}", hooks.id),
    ] {
        assert_eq!(get(path.clone()).await.0, 404, "{path}");
    }
    assert!(
        !get("/articles/sitemap.xml".into())
            .await
            .1
            .contains("/agents")
    );

    // While the topic collects nothing, the handbook waits, whatever changed.
    sqlx::query("UPDATE topics SET enabled = false WHERE id = $1")
        .bind(TOPIC)
        .execute(&state.pool)
        .await
        .unwrap();
    HOOKS_EDITION.store(3, Ordering::SeqCst);
    assert_eq!(xabc::write(&state, line, Utc::now()).await.unwrap(), None);
    HOOKS_EDITION.store(0, Ordering::SeqCst);
    // Readers still find nothing, while an administrator reads a chapter and copies its Markdown.
    state.digest.reload_topics(&state).await.unwrap();
    let (_, admin) = account(&state, "admin").await;
    for path in ["/agents/hooks", "/agents/hooks.md"] {
        assert_eq!(get(path.into()).await.0, 404, "{path}");
        let preview = http
            .get(format!("{base}/api/admin/preview{path}"))
            .header("cookie", &admin)
            .send()
            .await
            .unwrap();
        assert_eq!(
            (
                preview.status().as_u16(),
                preview.headers()["cache-control"].to_str().unwrap()
            ),
            (200, "private, no-store"),
            "{path}"
        );
    }
}
