use super::*;

#[test]
fn x_aliases_and_tracking_links_have_one_identity() {
    let first = url::Url::parse("https://x.com/author/status/1234567890?s=20").unwrap();
    let alias = url::Url::parse("https://twitter.com/renamed/status/1234567890").unwrap();
    assert_eq!(fingerprint(&first), fingerprint(&alias));
    assert_ne!(
        fingerprint(&first),
        fingerprint(&url::Url::parse("https://x.com/author/status/1234567891").unwrap())
    );
}

#[test]
fn feed_tracking_does_not_create_duplicate_articles() {
    let article = url::Url::parse("https://netflixtechblog.com/article").unwrap();
    for link in [
        "https://netflixtechblog.com/article?source=rss----123",
        "https://netflixtechblog.com/article?wt_mc=rss&lang=en",
    ] {
        let url = url::Url::parse(link).unwrap();
        let expected = if link.contains("lang=") {
            url::Url::parse("https://netflixtechblog.com/article?lang=en").unwrap()
        } else {
            article.clone()
        };
        assert_eq!(fingerprint(&url), fingerprint(&expected));
    }
}

#[test]
fn copied_passages_are_rejected_across_whitespace() {
    let original = "A specific original argument with enough supporting detail to count as a substantive passage, rather than a common phrase that might appear in unrelated works.";
    assert!(copies_passage(&original.replace(' ', "\n"), original));
    assert!(!copies_passage(
        "An independent explanation of the practical tradeoffs.",
        original
    ));
}

fn source(path: &str) -> source::Source {
    source::Source {
        url: url::Url::parse(&format!("https://example.com/{path}")).unwrap(),
        title: "A small team’s notes on building dependable AI tools".into(),
        author: "Example Author".into(), language: "en".into(), published_at: Some(Utc::now()),
        body: "## Start with a question\n\nAn original source discusses how a small team reduced review effort by choosing a narrow task and checking the results against real examples. The scope of the experiment matters more than the number of features.\n\n".repeat(4),
        images: Vec::new(),
    }
}

fn item(index: usize) -> NewItem {
    crate::fetch::validate(serde_json::from_value(json!({
        "title":format!("从一件小事开始，让 AI 工具真正有用 · {index}"),
        "title_en":format!("Start small: making AI tools useful · {index}"),
        "summary":"一篇关于小团队如何缩小问题、建立反馈并持续改进的经验文章。", "summary_en":"An essay on how small teams narrow a problem and learn from feedback.",
        "url":format!("https://example.com/reading/{index}"), "source":"Example", "published_at":null,
        "tags":["AI"], "tags_en":["AI"], "importance":3,
    })).unwrap(), Utc::now() - chrono::Duration::days(3)).unwrap()
}

fn article(index: usize) -> Article {
    Article {
        author: "Example Author".into(), source_title: "A small team’s notes".into(), source_language:"en".into(),
        body: format!("## 从具体问题开始\n\n与其一开始建立包罗万象的系统，这篇文章提供了一条更容易验证的路径：先找到一个有明确输入、输出和使用者的小问题。\n\n## 给结果留下检验的空间\n\n真正值得关注的是反馈是否能够进入下一轮改进。团队需要记录失败的例子，区分模型的能力边界与流程本身的问题，再决定是否扩大使用范围。\n\n## 如何理解这份经验\n\n这些观察来自特定团队的实践，适用范围取决于数据质量、任务复杂度和检查成本。它提供的是一种思考顺序，而非对所有场景的效果承诺。\n\n第 {index} 篇阅读。"),
        body_en: "## Begin with a concrete problem\n\nA small, observable task makes it possible to learn from actual use. The useful lesson is to connect each experiment with a clear question, a review process and a decision about what to change next.\n\n## Keep the limits in view\n\nThe account describes one team’s experience. Its value lies in the questions it raises about evaluation, feedback and the cost of checking results, rather than a universal promise about performance.".into(),
        source_hash:vec![index as u8;32], fetched_at:Utc::now(), full_text: None,
    }
}

#[tokio::test]
#[ignore = "requires NEWS_READS_TEST_DATABASE_URL and local test configuration"]
async fn postgres_reads_publication_visibility_and_permanent_archive() {
    use axum::{Json, Router, routing::post};
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    let database_url =
        std::env::var("NEWS_READS_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_reads_test"));
    let mut config = crate::config::Config::from_env().unwrap();
    config.database_url = database_url;
    config.redis_addr = None;
    config.tmpfile_base_url = None;
    config.scheduler_enabled = false;
    let calls = Arc::new(AtomicUsize::new(0));
    let handler_calls = calls.clone();
    let ai = Router::new().route("/v1/responses", post(move |Json(request): Json<Value>| {
        let calls = handler_calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let input: Value = serde_json::from_str(request["input"].as_str().unwrap()).unwrap();
            assert!(request.get("tools").is_none());
            let draft = if request["text"]["format"]["name"] == "reads_translation" {
                if input["title"] == "Incomplete translation fixture" { json!({"texts":[]}) } else {
                json!({"texts":input["texts"].as_array().unwrap().iter().map(|text| json!({"id":text["id"],"zh": if text["text"].as_str().unwrap().starts_with("Start") { "从一个问题开始" } else { "原文指出，团队通过缩小任务范围，并使用真实样例检验结果，减少了复核工作。实验的范围比功能数量更重要。" }, "en":if input["english_needed"] == true { text["text"].as_str().unwrap() } else { "" }})).collect::<Vec<_>>()}) }
            } else {
                assert_eq!(request["text"]["format"]["name"], "reads_editorial");
                assert!(input["source_text"].as_str().unwrap().contains("scope of the experiment"));
                json!({"worthy":!input["url"].as_str().unwrap().ends_with("/rejected"),
                "source_language":if input["source_language"] == "und" { "en" } else { input["source_language"].as_str().unwrap() },
                "title":"让 AI 工具真正有用，从一个可验证的问题开始", "title_en":"Making AI useful starts with a question you can test",
                "summary":"从小团队的实践中，理解任务范围、验证成本与反馈速度之间的取舍。", "summary_en":"A reading of the tradeoffs between task scope, review costs and the speed of feedback.",
                "body":article(0).body.repeat(2),
                "body_en":article(0).body_en.repeat(3),
            }) };
            Json(json!({"status":"completed", "id":"reads-fixture", "usage":{"input_tokens":40,"output_tokens":50,"cost_in_usd_ticks":90},
                "output":[{"type":"message","content":[{"type":"output_text","text":draft.to_string()}]}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.ai_base_url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let ai_server = tokio::spawn(async move { axum::serve(listener, ai).await.unwrap() });
    let state = AppState::new(config).await.unwrap();
    let topic: Topic = sqlx::query_as("SELECT * FROM topics WHERE id='reads'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(topic.name, "𝑹𝒆𝒂𝒅𝒔");
    assert!(!topic.brief.contains("转载许可"));
    assert_eq!(
        (
            topic.interval_minutes,
            topic.window_start,
            topic.window_end,
            topic.max_items
        ),
        (64, 0, 24, 4)
    );
    assert_eq!(topic.weekdays, [1, 2, 3, 4, 5, 6, 7]);
    let run_id = Uuid::new_v4();
    sqlx::query("INSERT INTO runs (id,topic_id,triggered_by,status,model) VALUES ($1,'reads','manual','ok','test')")
        .bind(run_id).execute(&state.pool).await.unwrap();
    let today = state.digest.today();
    let old = today - chrono::Duration::days(200);
    // Available capacity never overrides an editorial rejection.
    let rejected = source("rejected");
    assert!(
        prepare_source(
            &state,
            &topic,
            &mut item(2),
            &mut Usage::default(),
            rejected
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(daily_count(&state, today).await.unwrap(), 0);
    let mut usage = Usage::default();
    let mut full_item = item(0);
    let full = prepare_source(&state, &topic, &mut full_item, &mut usage, source("essay"))
        .await
        .unwrap()
        .unwrap();
    assert!(full.full_text.is_some());
    assert_eq!(full.body, article(0).body.repeat(2));
    assert!(!full.body_en.is_empty());
    let document = full.full_text.as_ref().unwrap();
    assert_eq!(document.original, source("essay").body);
    assert!(document.zh.is_some());
    assert!(document.en.is_none());
    let mut brief_item = item(1);
    let mut incomplete = source("incomplete");
    incomplete.title = "Incomplete translation fixture".into();
    let brief = prepare_source(&state, &topic, &mut brief_item, &mut usage, incomplete)
        .await
        .unwrap()
        .unwrap();
    assert!(brief.full_text.is_none());
    assert!(!brief.body.contains("scope of the experiment"));
    assert_eq!(
        (
            usage.input_tokens,
            usage.output_tokens,
            usage.cost_in_usd_ticks
        ),
        (160, 200, 360)
    );
    let articles = HashMap::from([
        (full_item.fingerprint.clone(), full),
        (brief_item.fingerprint.clone(), brief),
    ]);
    assert_eq!(
        crate::runner::persist_items(
            &state,
            &topic,
            run_id,
            old,
            vec![full_item, brief_item],
            articles,
            vec![]
        )
        .await
        .unwrap(),
        (2, 0, 0)
    );
    // A repeat from a different day is not published or sent to the editor again.
    assert!(
        prepare_source(
            &state,
            &topic,
            &mut item(1),
            &mut usage,
            source("incomplete")
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    let mut repeat = item(0);
    repeat.url = source("essay").url.to_string();
    repeat.fingerprint = fingerprint(&source("essay").url);
    assert_eq!(
        crate::runner::persist_items(
            &state,
            &topic,
            run_id,
            today,
            vec![repeat],
            HashMap::new(),
            vec![]
        )
        .await
        .unwrap(),
        (0, 1, 0)
    );
    // No metadata-only placeholder survives a body failure.
    assert!(
        crate::runner::persist_items(
            &state,
            &topic,
            run_id,
            today,
            vec![item(999)],
            HashMap::new(),
            vec![]
        )
        .await
        .is_err()
    );
    assert_eq!(daily_count(&state, today).await.unwrap(), 0);
    let mut broken = article(998);
    broken.body = "too short".into();
    assert!(
        crate::runner::persist_items(
            &state,
            &topic,
            run_id,
            today,
            vec![item(998)],
            HashMap::from([(item(998).fingerprint, broken)]),
            vec![]
        )
        .await
        .is_err()
    );
    assert_eq!(daily_count(&state, today).await.unwrap(), 0);
    // Concurrent publications serialize their daily allowance under the topic lock.
    let batch = |start: usize| {
        let items: Vec<_> = (start..start + 14).map(item).collect();
        let articles = items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.fingerprint.clone(), article(start + i)))
            .collect();
        crate::runner::persist_items(&state, &topic, run_id, today, items, articles, vec![])
    };
    let (first, second) = tokio::join!(batch(10), batch(50));
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first.0 + second.0, 24);
    assert_eq!(first.2 + second.2, 4);
    assert_eq!(daily_count(&state, today).await.unwrap(), DAILY_LIMIT);
    // At the daily limit, a scheduled run does not call search or generation.
    let cap_run = crate::runner::start(&state, &topic, "manual")
        .await
        .unwrap();
    for _ in 0..100 {
        let status: String = sqlx::query_scalar("SELECT status FROM runs WHERE id=$1")
            .bind(cap_run)
            .fetch_one(&state.pool)
            .await
            .unwrap();
        if status == "ok" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let payload: Value = sqlx::query_scalar("SELECT payload FROM runs WHERE id=$1")
        .bind(cap_run)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(payload["skip"], "reads_daily_limit");
    assert_eq!(calls.load(Ordering::SeqCst), 5);
    // News retention cannot remove Reads or its body.
    let news_topic: Topic = sqlx::query_as("SELECT * FROM topics WHERE id='x'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    crate::runner::persist_items(
        &state,
        &news_topic,
        run_id,
        old,
        vec![item(1000)],
        HashMap::new(),
        vec![],
    )
    .await
    .unwrap();
    crate::runner::cleanup_items(&state, today - chrono::Duration::days(128))
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM read_articles")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(count, 26);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE topic_id='x'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let id: Uuid =
        sqlx::query_scalar("SELECT item_id FROM read_articles WHERE full_text IS NOT NULL")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = crate::web::router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .get(format!("{base}/reads/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let etag = response.headers()["etag"].clone();
    let html = response.text().await.unwrap();
    for text in [
        "实验的范围比功能数量更重要",
        "全文译文",
        "application/ld+json",
        "rel=\"canonical\"",
        "hreflang=\"en\"",
        "og:description",
        "Example Author",
    ] {
        assert!(html.contains(text), "missing {text}");
    }
    let cached = client
        .get(format!("{base}/reads/{id}"))
        .header("If-None-Match", etag)
        .send()
        .await
        .unwrap();
    assert_eq!(cached.status(), 304);
    let english = client
        .get(format!("{base}/reads/{id}?lang=en"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(english.contains("<html lang=\"en\""));
    assert!(english.contains("scope of the experiment"));
    assert!(english.contains("Original article"));
    let document = scraper::Html::parse_document(&html);
    assert_eq!(
        document
            .select(&scraper::Selector::parse(".read-title [data-page-views]").unwrap())
            .count(),
        1
    );
    assert_eq!(
        document
            .select(&scraper::Selector::parse(".read-source [data-page-views]").unwrap())
            .count(),
        0
    );
    assert!(!html.contains("授权全文"));
    assert!(!html.contains("Republished under"));
    let list = client
        .get(format!("{base}/reads?page=3"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(list.contains(&format!("/reads/{id}")));
    let sitemap = client
        .get(format!("{base}/reads/sitemap/1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(sitemap.contains(&id.to_string()));
    sqlx::query("UPDATE items SET hidden=true WHERE id=$1")
        .bind(id)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{base}/reads/{id}"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let sitemap = client
        .get(format!("{base}/reads/sitemap/1"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!sitemap.contains(&id.to_string()));
    sqlx::query("UPDATE items SET hidden=false WHERE id=$1")
        .bind(id)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET enabled=false WHERE id='reads'")
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{base}/reads/{id}"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    sqlx::query("UPDATE topics SET enabled=true WHERE id='reads'")
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{base}/reads?page=99"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    // Deleting an item also removes its body, with no orphaned public page.
    let deleted: Uuid =
        sqlx::query_scalar("SELECT item_id FROM read_articles WHERE full_text IS NULL LIMIT 1")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    sqlx::query("DELETE FROM items WHERE id=$1")
        .bind(deleted)
        .execute(&state.pool)
        .await
        .unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM read_articles WHERE item_id=$1)")
            .bind(deleted)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(!exists);
    let items: Vec<_> = (130..134).map(item).collect();
    let articles = items
        .iter()
        .enumerate()
        .map(|(i, item)| (item.fingerprint.clone(), article(130 + i)))
        .collect();
    crate::runner::persist_items(&state, &topic, run_id, old, items, articles, vec![])
        .await
        .unwrap();
    let first_page = client
        .get(format!("{base}/reads"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(first_page.contains("rel=next href=\"/reads?page=2\""));
    let last_page = client
        .get(format!("{base}/reads?page=3&lang=en"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(last_page.contains("rel=prev href=\"/reads?page=2&amp;lang=en\""));
    assert!(!last_page.contains("rel=next"));
    let feed = client
        .get(format!("{base}/feed.xml?topic=reads"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(feed.contains(&format!("href=\"{}reads/", state.config.public_base_url)));
    check_related_candidates(&state, &client, &base, id).await;
    check_cached_reads_stats(&state, &client, &base).await;
    discovery::check_queue(&state).await;
    check_full_text_variants(&state, &topic, &calls).await;
    check_full_text_image_renewal(&state, id).await;
    server.abort();
    ai_server.abort();
}

async fn check_full_text_variants(
    state: &AppState,
    topic: &Topic,
    calls: &std::sync::atomic::AtomicUsize,
) {
    use std::sync::atomic::Ordering;
    let mut japanese = source("essay");
    japanese.language = "ja".into();
    japanese.url.set_path("/japanese");
    // Multiple chunks, with both translations required for a non-English original.
    japanese.body = japanese.body.repeat(12);
    let mut usage = Usage::default();
    let before = calls.load(Ordering::SeqCst);
    let full = prepare_source(state, topic, &mut item(701), &mut usage, japanese)
        .await
        .unwrap()
        .unwrap();
    let full = full.full_text.unwrap();
    assert!(full.zh.is_some() && full.en.is_some());
    assert!(calls.load(Ordering::SeqCst) - before > 2);
    assert_eq!(
        full.zh.as_ref().unwrap().len(),
        full.en.as_ref().unwrap().len()
    );

    let mut unspecified = source("essay");
    unspecified.url.set_path("/without-html-language");
    unspecified.language = "und".into();
    let full = prepare_source(state, topic, &mut item(704), &mut usage, unspecified)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(full.source_language, "en");
    assert!(
        full.full_text.unwrap().en.is_none(),
        "English originals are reused even without an HTML language hint"
    );

    let mut incomplete = source("essay");
    incomplete.url.set_path("/incomplete-translation");
    incomplete.title = "Incomplete translation fixture".into();
    let mut usage = Usage::default();
    let brief = prepare_source(state, topic, &mut item(703), &mut usage, incomplete)
        .await
        .unwrap()
        .unwrap();
    assert!(brief.full_text.is_none());
    assert_eq!(brief.body, article(0).body.repeat(2));
    assert_eq!(
        usage.input_tokens, 80,
        "Failed translation usage is still accounted for"
    );
}

async fn check_full_text_image_renewal(state: &AppState, id: Uuid) {
    use sqlx::types::Json;
    let mut media_state = state.clone();
    let mut config = crate::config::Config::from_env().unwrap();
    config.tmpfile_base_url = Some("https://media.example.test".parse().unwrap());
    media_state.config = std::sync::Arc::new(config);
    let Json(mut document): Json<full_text::Document> =
        sqlx::query_scalar("SELECT full_text FROM read_articles WHERE item_id=$1")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let stored = crate::media::UploadResponse {
        direct_link: "https://media.example.test/previous.png".into(),
        expires_at: Utc::now() + chrono::Duration::hours(3),
    };
    document.images.push(full_text::Image {
        origin: "https://127.0.0.1/private.png".into(),
        stored: Some(stored.clone()),
    });
    sqlx::query("UPDATE read_articles SET full_text=$2, images_renew_at=NOW() - INTERVAL '1 hour' WHERE item_id=$1")
        .bind(id).bind(Json(&document)).execute(&state.pool).await.unwrap();
    // Disabled topics have no image traffic or renewal updates.
    sqlx::query("UPDATE topics SET enabled=false WHERE id='reads'")
        .execute(&state.pool)
        .await
        .unwrap();
    full_text::refresh_images(&media_state).await.unwrap();
    let still_due: bool =
        sqlx::query_scalar("SELECT images_renew_at < NOW() FROM read_articles WHERE item_id=$1")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(still_due);
    sqlx::query("UPDATE topics SET enabled=true WHERE id='reads'")
        .execute(&state.pool)
        .await
        .unwrap();
    full_text::refresh_images(&media_state).await.unwrap();
    let (Json(updated), next): (Json<full_text::Document>, DateTime<Utc>) =
        sqlx::query_as("SELECT full_text,images_renew_at FROM read_articles WHERE item_id=$1")
            .bind(id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert!(next > Utc::now());
    assert_eq!(
        updated.images[0].stored.as_ref().unwrap().direct_link,
        stored.direct_link
    );
    assert_eq!(
        updated.images[0].stored.as_ref().unwrap().expires_at,
        stored.expires_at
    );
    assert_eq!(updated.original, document.original);
    assert_eq!(updated.zh, document.zh);
}

async fn check_related_candidates(
    state: &AppState,
    client: &reqwest::Client,
    base: &str,
    current: Uuid,
) {
    let now = Utc::now();
    sqlx::query(
        "UPDATE items SET first_seen_at = CASE WHEN id=$1 THEN $2::timestamptz ELSE $2 - INTERVAL '2 days' END,
         tags = CASE WHEN id=$1 THEN ARRAY['related-test'] ELSE '{}'::text[] END, pinned=false
         WHERE topic_id='reads'",
    )
    .bind(current)
    .bind(now)
    .execute(&state.pool)
    .await
    .unwrap();
    // The hidden match and current article must not consume the 200 visible slots.
    // Match 200 is included; pinned match 201 is outside the recency window.
    sqlx::query(
        "WITH inserted AS (
            INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url, tags,
                               first_seen_at, hidden, pinned)
            SELECT lpad(to_hex(g+1),32,'0')::uuid, 'reads', CURRENT_DATE,
                   sha256(convert_to('related-'||g,'UTF8')), 'Related '||g, 'Reading guide',
                   'https://example.test/related/'||g,
                   CASE WHEN g IN (0,200,201) THEN ARRAY['related-test'] ELSE '{}'::text[] END,
                   $1::timestamptz - (CASE WHEN g=2 THEN 1 ELSE g END)*INTERVAL '1 second',
                   g=0, g=201
            FROM generate_series(0,201) g RETURNING id
        )
        INSERT INTO read_articles (item_id, author, source_title, source_language,
                                   body, source_hash, fetched_at)
        SELECT id, 'Example Author', 'Original essay', 'en',
               repeat('A paragraph explaining the original argument. ',6),
               sha256(convert_to(id::text,'UTF8')), $1 FROM inserted",
    )
    .bind(now)
    .execute(&state.pool)
    .await
    .unwrap();
    let html = client
        .get(format!("{base}/reads/{current}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    let document = scraper::Html::parse_document(&html);
    let selector = scraper::Selector::parse(".read-related .item-title a").unwrap();
    let links: Vec<_> = document
        .select(&selector)
        .map(|node| node.value().attr("href").unwrap())
        .collect();
    // Prefer the oldest eligible tag match, then break equal timestamps by ID.
    assert_eq!(
        links,
        [200, 2, 1].map(|g| format!("/reads/{}", Uuid::from_u128(g + 1)))
    );
    let html = client
        .get(format!("{base}/reads"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    let document = scraper::Html::parse_document(&html);
    let selector = scraper::Selector::parse(".reads-list .item-title a").unwrap();
    let links: Vec<_> = document
        .select(&selector)
        .map(|node| node.value().attr("href").unwrap())
        .collect();
    assert_eq!(links.len(), 12);
    assert_eq!(links[0], format!("/reads/{}", Uuid::from_u128(202)));
    assert_eq!(links[1], format!("/reads/{current}"));
    assert_eq!(links[2], format!("/reads/{}", Uuid::from_u128(3)));
    assert_eq!(links[3], format!("/reads/{}", Uuid::from_u128(2)));
    assert!(!links.contains(&format!("/reads/{}", Uuid::from_u128(1)).as_str()));
}

async fn check_cached_reads_stats(state: &AppState, client: &reqwest::Client, base: &str) {
    use axum::{Json, Router, http::StatusCode, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    sqlx::query("UPDATE items SET url = 'https://source' || (get_byte(uuid_send(id),15) % 16) || '.example/essay/' || id WHERE topic_id='reads'")
        .execute(&state.pool).await.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let fail_at = Arc::new(AtomicUsize::new(usize::MAX));
    let (counter, failure) = (calls.clone(), fail_at.clone());
    let app = Router::new().route("/pages", post(move |Json(request): Json<Value>| {
        let call = counter.fetch_add(1, Ordering::SeqCst);
        let failing = failure.load(Ordering::SeqCst);
        async move {
            if call >= failing { return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({}))); }
            let urls = request["urls"].as_array().unwrap();
            assert!(!urls.is_empty() && urls.len() <= 100);
            let pages: Vec<_> = urls.iter().map(|url| json!({"url":url, "visits":if failing == usize::MAX { 42 } else { 999 }})).collect();
            (StatusCode::OK, Json(json!({"pages":pages})))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/pages", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let cold = client
        .get(format!("{base}/reads?view=hot"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!cold.contains("class=read-rank"));
    assert!(cold.contains("href=\"/reads?view=hot\" aria-current=\"page\">热读</a>"));
    assert!(cold.contains("热读正在积累"));
    assert!(!cold.contains("<article class=item>"));
    // A single ranked article must keep the hot route and active tab.
    sqlx::query("UPDATE read_articles SET views=15, hot_rank=1 WHERE item_id=(SELECT id FROM items WHERE topic_id='reads' AND NOT hidden ORDER BY first_seen_at DESC LIMIT 1)")
        .execute(&state.pool).await.unwrap();
    let sparse = client
        .get(format!("{base}/reads?view=hot"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(sparse.matches("class=read-rank").count(), 1);
    assert!(sparse.contains("href=\"/reads?view=hot\" aria-current=\"page\">热读</a>"));
    assert!(sparse.contains("</time> · <span class=read-views"));
    assert!(!sparse.contains("热读 24"));
    stats::refresh(state, &endpoint).await.unwrap();
    let snapshot: Vec<(Uuid, i64, Option<i16>)> = sqlx::query_as("SELECT item_id, views, hot_rank FROM read_articles WHERE views IS NOT NULL ORDER BY item_id")
        .fetch_all(&state.pool).await.unwrap();
    assert_eq!(snapshot.iter().filter(|row| row.2.is_some()).count(), 24);
    assert!(snapshot.iter().all(|row| row.1 == 42));
    let before = calls.load(Ordering::SeqCst);
    let hot = client
        .get(format!("{base}/reads?view=hot&lang=en"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(hot.matches("class=read-rank").count(), 24);
    assert!(hot.contains(">Popular</a>"));
    assert!(!hot.contains("Popular 24"));
    assert!(hot.contains("/reads?view=hot&amp;lang=en"));
    assert!(!hot.contains("rel=next"));
    assert_eq!(
        before,
        calls.load(Ordering::SeqCst),
        "SSR must not call Stats"
    );
    // A later batch failing must not publish the earlier batch's changed counts.
    fail_at.store(before + 1, Ordering::SeqCst);
    assert!(stats::refresh(state, &endpoint).await.is_err());
    let unchanged: Vec<(Uuid, i64, Option<i16>)> = sqlx::query_as("SELECT item_id, views, hot_rank FROM read_articles WHERE views IS NOT NULL ORDER BY item_id")
        .fetch_all(&state.pool).await.unwrap();
    assert_eq!(unchanged, snapshot);
    let ranked = snapshot.iter().find(|row| row.2 == Some(1)).unwrap().0;
    sqlx::query("UPDATE items SET hidden=true WHERE id=$1")
        .bind(ranked)
        .execute(&state.pool)
        .await
        .unwrap();
    let hot = client
        .get(format!("{base}/reads?view=hot"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!hot.contains(&format!("/reads/{ranked}")));
    sqlx::query(
        "UPDATE items SET hidden=false, first_seen_at = NOW() - INTERVAL '31 days' WHERE id=$1",
    )
    .bind(ranked)
    .execute(&state.pool)
    .await
    .unwrap();
    let hot = client
        .get(format!("{base}/reads?view=hot"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!hot.contains(&format!("/reads/{ranked}")));
    sqlx::query("UPDATE topics SET enabled=false WHERE id='reads'")
        .execute(&state.pool)
        .await
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    stats::refresh(state, &endpoint).await.unwrap();
    assert_eq!(before, calls.load(Ordering::SeqCst));
    sqlx::query("UPDATE topics SET enabled=true WHERE id='reads'")
        .execute(&state.pool)
        .await
        .unwrap();
    server.abort();
}
