use super::*;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::{collections::HashMap, sync::Arc};

fn window() -> NewsWindow {
    NewsWindow {
        start: Utc::now() - Duration::hours(24),
        end: Utc::now(),
    }
}

fn proposal(url: &str, image: Option<&str>) -> NewItem {
    NewItem {
        fingerprint: vec![0; 32],
        title: "海边的风".into(),
        summary: "摄影师在海边拍摄的人像。".into(),
        title_en: "Sea Wind".into(),
        summary_en: "A seaside portrait.".into(),
        url: url.into(),
        source: "Model".into(),
        published_at: None,
        tags: vec!["人像".into()],
        tags_en: vec!["Portrait".into()],
        importance: 4,
        image: image.map(|url| NewsImage {
            url: url.into(),
            source_url: "https://x.com/alice/status/2105000000000000001".into(),
            caption: "海边的人像".into(),
            caption_en: "A portrait by the sea".into(),
            credit: "Alice".into(),
            origin_url: None,
            expires_at: None,
            content_hash: None,
            width: None,
            height: None,
        }),
    }
}

fn post(value: Value) -> Post {
    serde_json::from_value(value).unwrap()
}

fn photo(key: &str, width: u32, height: u32) -> Value {
    json!({ "type": "photo", "media_url_https": format!("https://pbs.twimg.com/media/{key}.jpg"),
        "original_info": { "width": width, "height": height } })
}

#[test]
fn confirms_proposals_with_the_post_record() {
    let created = Utc::now() - Duration::hours(2);
    let record = || {
        json!({ "created_at": created, "user": { "name": " Alice ", "screen_name": "Alice_1" },
            "possibly_sensitive": false,
            "mediaDetails": [photo("FIRSTkey", 3000, 4000), photo("CHOSEN_key", 2000, 3000)] })
    };
    let mut item = proposal(
        "https://twitter.com/alice/status/2105000000000000001?s=20",
        Some("https://pbs.twimg.com/media/CHOSEN_key?format=jpg&name=large"),
    );
    confirm(&mut item, "2105000000000000001", post(record()), window()).unwrap();
    assert_eq!(item.url, "https://x.com/Alice_1/status/2105000000000000001");
    assert_eq!(item.source, "Alice");
    assert_eq!(item.published_at, Some(created));
    let image = item.image.unwrap();
    assert_eq!(
        image.url,
        "https://pbs.twimg.com/media/CHOSEN_key?format=jpg&name=medium"
    );
    assert_eq!(image.source_url, item.url);
    assert_eq!(image.credit, "Alice @Alice_1");
    assert_eq!((image.width, image.height), (Some(2000), Some(3000)));
    assert_eq!(image.caption, "海边的人像");

    // Without a usable choice the first photograph is taken and the title becomes the caption.
    let mut item = proposal("https://x.com/alice/status/2105000000000000001", None);
    confirm(&mut item, "2105000000000000001", post(record()), window()).unwrap();
    let image = item.image.unwrap();
    assert!(image.url.contains("/FIRSTkey?"));
    assert_eq!(
        (image.caption.as_str(), image.caption_en.as_str()),
        ("海边的风", "Sea Wind")
    );

    let refused = |change: fn(&mut Value)| {
        let mut value = record();
        change(&mut value);
        let mut item = proposal("https://x.com/alice/status/2105000000000000001", None);
        confirm(&mut item, "2105000000000000001", post(value), window()).unwrap_err()
    };
    assert_eq!(
        refused(|value| value["possibly_sensitive"] = json!(true)),
        "X 标记为敏感内容"
    );
    assert_eq!(
        refused(|value| {
            value["mediaDetails"][1]["sensitive_media_warning"] = json!({ "adult_content": true })
        }),
        "X 标记为敏感内容"
    );
    assert_eq!(
        refused(|value| value["created_at"] = json!(Utc::now() - Duration::days(3))),
        "发布时间不在采集窗口内"
    );
    assert_eq!(
        refused(|value| {
            value["mediaDetails"] = json!([{ "type": "video",
                "media_url_https": "https://pbs.twimg.com/ext_tw_video_thumb/1/pu/img/a.jpg",
                "original_info": { "width": 1280, "height": 720 } }])
        }),
        "帖子没有照片"
    );
    assert_eq!(
        refused(|value| value["user"]["screen_name"] = json!("alice/../i")),
        "X 帖子作者无效"
    );
    assert_eq!(
        refused(|value| value["mediaDetails"] = json!([photo("Small", 478, 717)])),
        "照片尺寸过小"
    );
    // A tombstone has no post record.
    assert!(
        serde_json::from_value::<Post>(json!({ "__typename": "TweetTombstone", "tombstone": {} }))
            .is_err()
    );
}

/// A CONNECT proxy that answers for X and the file server itself, so the clients reach the
/// real host names on port 443 without DNS or privileged ports.
async fn x_and_file_server(
    posts: HashMap<String, Value>,
) -> (reqwest::Certificate, String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, rustls};

    async fn head<S: AsyncRead + Unpin>(stream: &mut S) -> String {
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.unwrap());
        }
        String::from_utf8(request).unwrap()
    }

    let certified = rcgen::generate_simple_self_signed(vec![
        "cdn.syndication.twimg.com".into(),
        "pbs.twimg.com".into(),
        "media.example.test".into(),
    ])
    .unwrap();
    let certificate = reqwest::Certificate::from_der(certified.cert.der()).unwrap();
    let tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![certified.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()).into(),
    )
    .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let photo = {
        let pixels = image::RgbImage::from_fn(640, 480, |x, y| {
            let n = x
                .wrapping_mul(1_664_525)
                .wrapping_add(y.wrapping_mul(1_013_904_223));
            image::Rgb([n as u8, (n >> 8) as u8, (n >> 16) as u8])
        });
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(pixels)
            .write_to(&mut bytes, image::ImageFormat::Jpeg)
            .unwrap();
        bytes.into_inner()
    };
    let server = tokio::spawn(async move {
        for upload in 1.. {
            let (mut stream, _) = listener.accept().await.unwrap();
            let connect = head(&mut stream).await;
            assert!(connect.starts_with("CONNECT "), "{connect}");
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let request = head(&mut stream).await;
            let length = request
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .map_or(0, |value| value.trim().parse().unwrap());
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            let target = request.split_whitespace().nth(1).unwrap().to_owned();
            let (status, kind, reply) = if let Some(query) = target.strip_prefix("/tweet-result?") {
                let query =
                    url::form_urlencoded::parse(query.as_bytes()).collect::<HashMap<_, _>>();
                assert_eq!(query["token"], x::embed_token(&query["id"]));
                match posts.get(query["id"].as_ref()) {
                    Some(post) => ("200 OK", "application/json", post.to_string().into_bytes()),
                    None => ("404 Not Found", "application/json", b"{}".to_vec()),
                }
            } else if target.starts_with("/media/") {
                assert!(target.ends_with("?format=jpg&name=medium"), "{target}");
                ("200 OK", "image/jpeg", photo.clone())
            } else {
                assert_eq!(target, "/upload");
                assert!(
                    request.contains("authorization: Bearer test-key"),
                    "{request}"
                );
                let stored = json!({ "direct_link": format!("https://media.example.test/files/{upload}.jpg"),
                    "expires_at": Utc::now() + Duration::hours(36) });
                (
                    "200 OK",
                    "application/json",
                    stored.to_string().into_bytes(),
                )
            };
            stream
                .write_all(format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).as_bytes())
                .await
                .unwrap();
            stream.write_all(&reply).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    (certificate, proxy, server)
}

#[tokio::test]
#[ignore = "requires NEWS_GALLERY_TEST_DATABASE_URL and local test configuration"]
async fn postgres_gallery_keeps_confirmed_photographs_until_their_copies_expire() {
    use axum::{Json, Router, routing::post};
    let database_url =
        std::env::var("NEWS_GALLERY_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_gallery_test"));
    // A database left by an earlier run starts over; a fresh one has no tables yet.
    for statement in [
        "DELETE FROM runs WHERE topic_id = 'w'",
        "DELETE FROM items WHERE topic_id = 'w'",
        "UPDATE topics SET enabled = true, last_run_at = NULL, last_status = NULL WHERE owner_user_id IS NULL",
    ] {
        sqlx::query(statement).execute(&pool).await.ok();
    }

    let now = Utc::now();
    let id = |n: u64| (2_105_000_000_000_000_000_u64 + n).to_string();
    let record = |account: &str, hours: i64, media: Value| {
        json!({ "__typename": "Tweet", "created_at": now - Duration::hours(hours),
            "user": { "name": account.to_uppercase(), "screen_name": account }, "mediaDetails": media })
    };
    let mut posts = HashMap::new();
    posts.insert(
        id(1),
        record(
            "alice",
            2,
            json!([photo("AliceOne", 3000, 4000), photo("AliceTwo", 2000, 3000)]),
        ),
    );
    let mut sensitive = record("bob", 2, json!([photo("Bob", 3000, 4000)]));
    sensitive["possibly_sensitive"] = json!(true);
    posts.insert(id(2), sensitive);
    posts.insert(
        id(3),
        record(
            "carol",
            2,
            json!([{ "type": "video",
        "media_url_https": "https://pbs.twimg.com/ext_tw_video_thumb/3/pu/img/v.jpg",
        "original_info": { "width": 1280, "height": 720 } }]),
        ),
    );
    posts.insert(
        id(4),
        record("dave", 72, json!([photo("Dave", 3000, 4000)])),
    );
    posts.insert(
        id(5),
        record("Alice", 1, json!([photo("AliceAgain", 3000, 4000)])),
    );
    posts.insert(id(7), record("erin", 3, json!([photo("Erin", 4000, 3000)])));
    let proposals = [
        (
            format!("https://twitter.com/alice/status/{}?s=20", id(1)),
            "AliceTwo",
            vec!["人像"],
        ),
        (
            format!("https://x.com/bob/status/{}", id(2)),
            "Bob",
            vec!["泳装"],
        ),
        (
            format!("https://x.com/carol/status/{}", id(3)),
            "Carol",
            vec!["舞蹈"],
        ),
        (
            format!("https://x.com/dave/status/{}", id(4)),
            "Dave",
            vec!["礼服"],
        ),
        (
            format!("https://x.com/Alice/status/{}", id(5)),
            "AliceAgain",
            vec!["人像"],
        ),
        (
            format!("https://x.com/frank/status/{}", id(6)),
            "Frank",
            vec!["街拍"],
        ),
        (
            format!("https://x.com/erin/status/{}", id(7)),
            "Erin",
            vec!["AI", "人像"],
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (url, key, tags))| {
        json!({
            "title": format!("画面{index}"), "title_en": format!("Picture {index}"),
            "summary": "帖子里的人像。", "summary_en": "A portrait from the post.",
            "url": url, "source": "Model", "published_at": now - Duration::hours(1),
            "tags": tags, "tags_en": tags, "importance": 4,
            "image": { "url": format!("https://pbs.twimg.com/media/{key}.jpg"), "source_url": url,
                "caption": "人像", "caption_en": "Portrait", "credit": "Model" }
        })
    })
    .collect::<Vec<_>>();
    let ai = Router::new().route(
        "/v1/responses",
        post(move |Json(request): Json<Value>| {
            let items = proposals.clone();
            async move {
                assert_eq!(
                    request["tools"],
                    json!([{ "type": "x_search", "enable_image_understanding": true }])
                );
                assert!(request["instructions"].as_str().unwrap().contains("「𝑾」"));
                let payload = json!({ "items": items, "cover": null });
                Json(json!({ "status": "completed", "id": "gallery-fixture",
                    "usage": { "input_tokens": 10, "output_tokens": 20, "cost_in_usd_ticks": 30,
                        "server_side_tool_usage_details": { "web_search_calls": 0, "x_search_calls": 2 } },
                    "output": [{ "type": "message", "content": [{ "type": "output_text", "text": payload.to_string() }] }] }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = crate::config::Config::from_env().unwrap();
    config.database_url = database_url;
    config.redis_addr = None;
    config.scheduler_enabled = false;
    config.ai_base_url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    config.tmpfile_base_url = "https://media.example.test".parse().unwrap();
    config.tmpfile_api_key = "test-key".into();
    let ai_server = tokio::spawn(async move { axum::serve(listener, ai).await.unwrap() });
    let (certificate, proxy, origin_server) = x_and_file_server(posts).await;
    let mut state = AppState::new(config).await.unwrap();
    let client = || {
        reqwest::Client::builder()
            .proxy(reqwest::Proxy::https(&proxy).unwrap())
            .tls_certs_only([certificate.clone()])
            .build()
            .unwrap()
    };
    state.http = client();
    state.origin_http = client();

    let topic: Topic = sqlx::query_as("SELECT * FROM topics WHERE id = 'w'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(is_gallery(&topic) && topic.enabled && topic.visible);
    assert_eq!(topic.name, "𝑾");
    assert_eq!(
        (
            topic.interval_minutes,
            topic.max_items,
            topic.lookback_hours,
            topic.sort_weight
        ),
        (360, 8, 24, 15)
    );
    let catalogue = state
        .digest
        .topics()
        .iter()
        .map(|topic| topic.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(&catalogue[..3], ["x", "p", "w"]);

    let run = crate::runner::start(&state, &topic, "manual")
        .await
        .unwrap();
    let (status, new, dropped, payload) = loop {
        let row: (String, i32, i32, Option<Value>) = sqlx::query_as(
            "SELECT status, new_count, dropped_count, payload FROM runs WHERE id = $1",
        )
        .bind(run)
        .fetch_one(&state.pool)
        .await
        .unwrap();
        if row.0 != "running" {
            break row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(status, "ok", "{payload:?}");
    assert_eq!((new, dropped), (2, 5));
    let reasons = payload.unwrap()["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skip| skip["reason"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        [
            "X 标记为敏感内容",
            "帖子没有照片",
            "发布时间不在采集窗口内",
            "同一账号近期已收录",
            "X 帖子无法读取"
        ]
    );

    let today = state.digest.today();
    let snapshot = state
        .digest
        .snapshot(&state, TOPIC_ID, today)
        .await
        .unwrap();
    // The photographs wait for an administrator's ❤️: readers see none, the preview both.
    assert!(snapshot.items.is_empty());
    assert!(snapshot.preview.iter().all(|item| item.hidden));
    let urls = snapshot
        .preview
        .iter()
        .map(|item| item.url.as_str())
        .collect::<Vec<_>>();
    let alice_url = format!("https://x.com/alice/status/{}", id(1));
    assert_eq!(
        urls,
        [
            alice_url.as_str(),
            &format!("https://x.com/erin/status/{}", id(7))
        ]
    );
    let alice = &snapshot.preview[0];
    assert_eq!(alice.source, "ALICE");
    assert_eq!(
        alice.published_at.unwrap().timestamp(),
        (now - Duration::hours(2)).timestamp()
    );
    let image = alice.image.as_ref().unwrap();
    assert!(
        image["url"]
            .as_str()
            .unwrap()
            .starts_with("https://media.example.test/files/")
    );
    assert_eq!(
        (image["width"].as_u64(), image["height"].as_u64()),
        (Some(2000), Some(3000))
    );
    assert_eq!(image["credit"], "ALICE @alice");
    assert!(image.get("origin_url").is_none() && image.get("expires_at").is_none());
    assert_eq!(snapshot.preview[1].tags, ["AI", "人像"]);
    let origin: String = sqlx::query_scalar(
        "SELECT image->>'origin_url' FROM items WHERE topic_id = 'w' AND url = $1",
    )
    .bind(&alice_url)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        origin,
        "https://pbs.twimg.com/media/AliceTwo?format=jpg&name=medium"
    );

    // The briefing neither collects nor reads the gallery.
    sqlx::query("UPDATE topics SET enabled = (id = 'w')")
        .execute(&state.pool)
        .await
        .unwrap();
    crate::runner::collect_briefing(&state, window(), None)
        .await
        .unwrap();
    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE topic_id = 'w'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(runs, 1);

    async fn stored(pool: &sqlx::PgPool, url: &str) -> Option<(NaiveDate, String, DateTime<Utc>)> {
        sqlx::query_as(
            "SELECT day, image->>'url', (image->>'expires_at')::timestamptz FROM items WHERE url = $1",
        )
        .bind(url)
        .fetch_optional(pool)
        .await
        .unwrap()
    }
    async fn expire(pool: &sqlx::PgPool, url: &str, day: NaiveDate) {
        sqlx::query(
            "UPDATE items SET day = $2, image = jsonb_set(image, '{expires_at}', $3) WHERE url = $1",
        )
        .bind(url)
        .bind(day)
        .bind(json!(Utc::now() - Duration::minutes(1)))
        .execute(pool)
        .await
        .unwrap();
    }
    let pool = &state.pool;

    // A recent copy that lapsed, as after an outage, is renewed.
    let (_, lapsed, _) = stored(pool, &alice_url).await.unwrap();
    expire(pool, &alice_url, today).await;
    crate::media::startup_sync(&state).await.unwrap();
    let (_, renewed, expires_at) = stored(pool, &alice_url).await.unwrap();
    assert!(renewed.starts_with("https://media.example.test/files/") && renewed != lapsed);
    assert!(expires_at > Utc::now());

    crate::reading::tests::feed(&state).await;

    ai_server.abort();
    origin_server.abort();
}
