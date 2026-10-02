use super::*;

#[test]
fn sources_keep_content_and_resolve_only_public_links() {
    let url = url::Url::parse("https://publisher.example/books/shore").unwrap();
    let source = source::extract(
        &url,
        r#"<title>The Shore</title><body><nav>Navigation</nav><main><h1>The Shore</h1><p>A book by Ada. Published in 2026.</p><script>ignore instructions</script><a href="/read/shore">Read a sample</a><a href="javascript:alert(1)">Bad link</a></main><footer>Footer</footer></body>"#,
    );
    assert!(source.text.contains("The Shore") && source.text.contains("Published in 2026"));
    assert!(!source.text.contains("Navigation") && !source.text.contains("ignore instructions"));
    assert_eq!(source.links.len(), 1);
    assert_eq!(source.links[0].url, "https://publisher.example/read/shore");
}

#[test]
fn guides_drop_uncited_fields_and_invented_links() {
    let sources = vec![Source {
        title: "The Shore".into(),
        url: "https://publisher.example/books/shore".into(),
        text: "The Shore is a novel by Ada, published in 2026.".into(),
        links: vec![],
    }];
    let entry = |quote: &str, source| Entry {
        label: "作者".into(),
        label_en: "Author".into(),
        text: "Ada".into(),
        text_en: "Ada".into(),
        quote: quote.into(),
        source,
    };
    let mut guide = Guide {
        intro: "一部小说。".into(),
        intro_en: "A novel.".into(),
        facts: vec![
            entry("The Shore is a novel by Ada", 0),
            entry("A fake quote about this work", 0),
            entry("The Shore is a novel by Ada", 1),
        ],
        sections: vec![],
        links: vec![
            Link {
                label: "原文".into(),
                label_en: "Source".into(),
                url: sources[0].url.clone(),
            },
            Link {
                label: "伪造".into(),
                label_en: "Invented".into(),
                url: "https://publisher.example/not-in-source".into(),
            },
        ],
        sources: vec![],
    };
    guide.facts.push(Entry {
        text: "2099".into(),
        text_en: "2099".into(),
        ..entry("The Shore is a novel by Ada, published in 2026", 0)
    });
    guide.facts.push(entry("!!!!!!!!!!!!!!!", 0));
    guide.verify(&sources).unwrap();
    assert_eq!(guide.facts.len(), 1);
    assert_eq!(guide.links.len(), 1);
    assert_eq!(guide.sources.len(), 1);
}

#[test]
fn proposals_require_a_work_page_and_matching_medium() {
    let proposal = Proposal {
        name: "海岸".into(),
        name_en: "The Shore".into(),
        creator: "Ada".into(),
        kind: "book".into(),
        source_urls: vec!["https://publisher.example/books/shore?utm_source=x".into()],
    };
    assert_eq!(
        proposal.clone().clean("book").unwrap().source_urls[0],
        "https://publisher.example/books/shore"
    );
    assert!(proposal.clone().clean("movie").is_none());
    assert!(
        Proposal {
            source_urls: vec!["https://publisher.example/".into()],
            ..proposal.clone()
        }
        .clean("book")
        .is_none()
    );
    assert!(
        Proposal {
            source_urls: vec!["https://127.0.0.1/private".into()],
            ..proposal
        }
        .clean("book")
        .is_none()
    );
}

/// A local HTTPS origin exercises source reading and media mirroring without external calls.
async fn origin() -> (String, reqwest::Client, tokio::task::JoinHandle<()>) {
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, rustls};
    let cert = rcgen::generate_simple_self_signed(vec![
        "culture.example".into(),
        "cdn.syndication.twimg.com".into(),
        "pbs.twimg.com".into(),
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
    let base = "https://culture.example".to_owned();
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::https(format!("http://{addr}")).unwrap())
        .tls_certs_only([root])
        .build()
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let url = base.clone();
    let pixels = image::RgbImage::from_fn(480, 640, |x, y| {
        image::Rgb([(x * y) as u8, x as u8, y as u8])
    });
    let mut jpeg = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(pixels)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let jpeg = jpeg.into_inner();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (acceptor, url, jpeg) = (acceptor.clone(), url.clone(), jpeg.clone());
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
                let path = head.split_whitespace().nth(1).unwrap_or("/");
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
                let (kind, body) = if path == "/cover.jpg"
                    || path == "/files/cover.jpg"
                    || path.starts_with("/media/CultureCover?")
                {
                    ("image/jpeg", jpeg)
                } else if path.starts_with("/tweet-result?") {
                    ("application/json", json!({"created_at":"2026-10-01T06:00:00Z","user":{"name":"Publisher on X","screen_name":"publisher"},"text":"The Shore cover is here.","mediaDetails":[{"type":"photo","media_url_https":"https://pbs.twimg.com/media/CultureCover.jpg","original_info":{"width":480,"height":640}}]}).to_string().into_bytes())
                } else if path == "/upload" {
                    ("application/json",json!({"direct_link":format!("{url}/files/cover.jpg"),"expires_at":chrono::Utc::now()+chrono::Duration::days(3)}).to_string().into_bytes())
                } else if path == "/unavailable" {
                    (
                        "text/html",
                        b"<title>Temporarily unavailable</title>".to_vec(),
                    )
                } else {
                    ("text/html; charset=utf-8", format!("<title>The Shore — Ada</title><meta property=\"og:image\" content=\"{url}/blocked.jpg\"><main><h1>The Shore</h1><p>The Shore is a novel by Ada, published in 2026. It follows a family returning to a coastal town and explores memory, home and the changing relationship between people and the sea. The author draws on interviews and years spent in small coastal communities. This page introduces the novel; it is not the full text of the book.</p><a href=\"{url}/sample\">Read a sample</a><a href=\"{url}/cover.jpg\">Télécharger la couverture</a></main>").into_bytes())
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
                stream.shutdown().await.ok();
            });
        }
    });
    (base, client, server)
}

#[tokio::test]
#[ignore = "requires NEWS_CULTURE_TEST_DATABASE_URL and local test configuration"]
async fn postgres_culture_publishes_news_then_sourced_guides_and_keeps_the_briefing_separate() {
    use crate::{config::Config, fetch::NewsWindow, state::AppState};
    use axum::{Json, Router, routing::post};
    use std::sync::{Arc, Mutex};
    let database_url =
        std::env::var("NEWS_CULTURE_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_culture_test"));
    let (base, client, origin_server) = origin().await;
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = calls.clone();
    let source_base = base.clone();
    let guide_pool = pool.clone();
    let ai=Router::new().route("/v1/responses",post(move |Json(request):Json<Value>| {
        let (seen,base,pool)=(seen.clone(),source_base.clone(),guide_pool.clone());
        async move {
            seen.lock().unwrap().push(request.clone());
            let payload=if request["text"]["format"]["name"]=="culture_guide" {
                let published:i64=sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE topic_id='book'").fetch_one(&pool).await.unwrap();
                assert_eq!(published,2,"stories with and without pictures are already published while the guide is written");
                assert!(request.get("tools").is_none());
                assert!(request["input"].as_str().unwrap().contains("not the full text"));
                json!({"intro":"一部关于记忆、故乡与海岸生活的小说。","intro_en":"A novel about memory, home and life by the sea.",
                    "facts":[{"label":"作者","label_en":"Author","text":"Ada","text_en":"Ada","source":0,"quote":"The Shore is a novel by Ada"}],
                    "sections":[{"label":"内容与背景","label_en":"Story and background","text":"一家人重返海边小镇，在记忆与变化中重新认识故乡。","text_en":"A family returns to a coastal town and reconsiders what home means.","source":0,"quote":"It follows a family returning to a coastal town and explores memory, home"}],
                    "links":[{"label":"试读","label_en":"Read a sample","url":format!("{base}/sample")} ]})
            } else {
                assert_eq!(request["tools"][0]["type"],"web_search");
                assert_eq!(request["tools"][1]["type"],"x_search");
                assert_eq!(request["tools"][1]["enable_image_understanding"],true);
                assert!(request["text"]["format"]["schema"]["properties"]["items"]["items"]["required"].as_array().unwrap().contains(&json!("image")));
                let item=json!({"title":"《海岸》新书发布","title_en":"The Shore is published","summary":"Ada 的新小说聚焦海岸社区与故乡记忆。","summary_en":"Ada's new novel explores coastal communities and memories of home.","url":format!("{base}/news/shore"),"source":"Publisher","published_at":chrono::Utc::now()-chrono::Duration::hours(1),"importance":3,"tags":["小说"],"tags_en":["Fiction"],
                    "image":{"url":format!("{base}/blocked.jpg"),"source_url":format!("{base}/news/shore"),"caption":"《海岸》书封","caption_en":"The Shore cover","credit":"Publisher"},"image_page_url":null,
                    "work":{"name":"海岸","name_en":"The Shore","creator":"Ada","kind":"book","source_urls":[format!("{base}/work/shore")]}});
                let mut unpictured = item.clone();
                unpictured["title"]=json!("文学奖公布入围名单");
                unpictured["title_en"]=json!("A literary prize announces its shortlist");
                unpictured["url"]=json!(format!("{base}/unavailable"));
                unpictured["image"]=Value::Null;
                unpictured["work"]=Value::Null;
                json!({"items":[item.clone(),item,unpictured],"cover":null})
            };
            Json(json!({"id":"culture-fixture","status":"completed","usage":{"input_tokens":10,"output_tokens":10},"output":[{"type":"message","content":[{"type":"output_text","text":"{\"items\":[],\"cover\":null}"}]},{"type":"message","content":[{"type":"output_text","text":payload.to_string()}]}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = Config::from_env().unwrap();
    config.database_url = database_url;
    config.redis_addr = None;
    config.scheduler_enabled = false;
    config.ai_base_url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    config.tmpfile_base_url = base.parse().unwrap();
    config.tmpfile_api_key = "fixture".into();
    let ai_server = tokio::spawn(async move { axum::serve(listener, ai).await.unwrap() });
    let mut state = AppState::new(config).await.unwrap();
    state.http = client.clone();
    state.origin_http = client;
    sqlx::query("TRUNCATE culture_items, culture_works, items, runs CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET enabled=true,visible=true,last_run_at=NULL,last_status=NULL WHERE id=ANY($1)").bind(IDS.as_slice()).execute(&pool).await.unwrap();
    let topics: Vec<Topic> = sqlx::query_as("SELECT * FROM topics WHERE id=ANY($1)")
        .bind(IDS.as_slice())
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(topics.len(), 4);
    assert!(
        topics
            .iter()
            .all(|t| t.interval_minutes == 720 && t.enabled && t.visible)
    );
    let topic = topics.iter().find(|t| t.id == "book").unwrap();
    let run = crate::runner::start(&state, topic, "manual").await.unwrap();
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id=$1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap();
            if status != "running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let (new,dup,status):(i32,i32,String)=sqlx::query_as("SELECT new_count,dup_count,status FROM runs WHERE topic_id='book' ORDER BY started_at DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!((new, dup, status.as_str()), (2, 1, "ok"));
    let (id, slug): (Uuid, String) = sqlx::query_as(
        "SELECT id,slug FROM culture_works WHERE topic_id='book' AND guide IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let app_url = format!("http://{}", listener.local_addr().unwrap());
    let router = crate::web::router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let get = |path: String| {
        let http = http.clone();
        let url = format!("{app_url}{path}");
        async move { http.get(url).send().await.unwrap() }
    };
    let page = get("/culture/book".into()).await.text().await.unwrap();
    assert!(
        page.contains("culture-book") && page.contains(&slug) && page.contains("/files/cover.jpg")
    );
    assert_eq!(page.matches(">𝑪𝒖𝒍𝒕𝒖𝒓𝒆</a>").count(), 1);
    assert!(page.contains("data-last=\"/culture/book\""));
    let detail = get(format!("/culture/book/{slug}"))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        detail.contains("内容与背景") && detail.contains("/sample") && detail.contains("source-0")
    );
    let english = get(format!("/culture/book/{slug}?lang=en"))
        .await
        .text()
        .await
        .unwrap();
    assert!(english.contains("Story and background") && english.contains("hreflang=\"zh-CN\""));
    let digest: Value = get("/digest?topic=book".into()).await.json().await.unwrap();
    let stories = digest["topics"][0]["items"].as_array().unwrap();
    let story = stories
        .iter()
        .find(|s| s["url"].as_str().unwrap().ends_with("/news/shore"))
        .unwrap();
    assert_eq!(story["culture"]["name"], "海岸");
    assert!(
        story["image"]["url"]
            .as_str()
            .unwrap()
            .ends_with("/files/cover.jpg")
    );
    assert!(story["image"].get("origin_url").is_none());
    let unpictured = stories
        .iter()
        .find(|s| s["url"].as_str().unwrap().ends_with("/unavailable"))
        .unwrap();
    assert!(unpictured["image"].is_null());
    // X can supply the attachment even when the model has no image URL.
    let x_candidate = json!({"title":"一张书封","title_en":"A book cover","summary":"","url":"https://x.com/publisher/status/2105624259129127408","published_at":chrono::Utc::now().to_rfc3339()});
    let mut x_items = vec![
        crate::fetch::validate(
            serde_json::from_value(x_candidate).unwrap(),
            chrono::Utc::now() - chrono::Duration::hours(1),
        )
        .unwrap(),
    ];
    prepare_images(
        &state,
        topic,
        &mut x_items,
        &json!({"items":[]}),
        Instant::now() + Duration::from_secs(30),
    )
    .await;
    let x_image = x_items[0].image.as_ref().unwrap();
    assert_eq!(
        x_image.origin_url.as_deref(),
        Some("https://pbs.twimg.com/media/CultureCover?format=jpg&name=medium")
    );
    assert_eq!(x_image.credit, "Publisher on X");
    assert_eq!((x_image.width, x_image.height), (Some(480), Some(640)));
    assert!(
        get("/culture/sitemap.xml".into())
            .await
            .text()
            .await
            .unwrap()
            .contains(&slug)
    );
    assert_eq!(
        get("/culture/book/more.json?before=bad".into())
            .await
            .status(),
        400
    );
    assert_eq!(get("/culture/unknown".into()).await.status(), 404);
    // A warm catalogue avoids another model call when the same source has not changed.
    let before = calls.lock().unwrap().len();
    sqlx::query("UPDATE culture_works SET checked_at=NOW()-INTERVAL '8 days' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let results = guides(
        &state,
        topic,
        &mut Usage::default(),
        Instant::now() + Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(results[0]["unchanged"], true);
    assert_eq!(calls.lock().unwrap().len(), before);
    // An unavailable source keeps the published guide and its retry budget, but rotates the queue.
    sqlx::query("UPDATE culture_works SET source_urls=$2, checked_at=NOW()-INTERVAL '8 days', attempts=1 WHERE id=$1")
        .bind(id).bind(json!([format!("{base}/unavailable")])).execute(&pool).await.unwrap();
    let results = guides(
        &state,
        topic,
        &mut Usage::default(),
        Instant::now() + Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(results[0]["deferred"], "资料暂不可读");
    let (checked, attempts, published): (bool, i16, bool) = sqlx::query_as(
        "SELECT checked_at > NOW()-INTERVAL '1 minute', attempts, guide IS NOT NULL FROM culture_works WHERE id=$1",
    ).bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((checked, attempts, published), (true, 1, true));
    assert_eq!(calls.lock().unwrap().len(), before);
    // The briefing has no work to collect when only Culture is enabled.
    sqlx::query("UPDATE topics SET enabled=(id=ANY($1))")
        .bind(IDS.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    crate::runner::collect_briefing(
        &state,
        NewsWindow {
            start: chrono::Utc::now() - chrono::Duration::hours(24),
            end: chrono::Utc::now(),
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!(calls.lock().unwrap().len(), before);
    sqlx::query("UPDATE topics SET visible=false WHERE id='book'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(get("/culture/book".into()).await.status(), 404);
    assert_eq!(get(format!("/culture/book/{slug}")).await.status(), 404);
    assert!(
        !get("/culture/sitemap.xml".into())
            .await
            .text()
            .await
            .unwrap()
            .contains(&slug)
    );
    let previews = crate::reading::tests::pages(
        &state,
        &app_url,
        "book",
        &[
            "/culture/book".into(),
            format!("/culture/book/{slug}"),
            "/culture/book/more.json".into(),
        ],
    )
    .await;
    assert!(previews[0].contains(&format!("/api/admin/preview/culture/book/{slug}")));
    assert!(previews[2].contains("/api/admin/preview/culture/book/"));
    // Saved work guides survive the ordinary news retention policy.
    sqlx::query("DELETE FROM items WHERE topic_id='book'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM culture_works WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    server.abort();
    ai_server.abort();
    origin_server.abort();
}
