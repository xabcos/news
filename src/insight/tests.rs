use super::*;
use articles::StoryStatus;
use chrono::TimeZone;
use std::time::Duration;

fn analysis() -> InsightAnalysis {
    serde_json::from_value(json!({
        "hotspots": [], "headline": "新闻", "headline_en": "News",
        "overview": "", "overview_en": "", "changes": "", "changes_en": "",
        "wealth": [], "employment": [], "risks": [], "actions": []
    }))
    .unwrap()
}

fn source(index: usize, topic: &str, current: bool) -> InsightSource {
    serde_json::from_value(json!({
        "index": index, "current": current, "topic_id": topic,
        "title": format!("新闻{index}"), "title_en": "News", "summary": "内容", "summary_en": "Summary",
        "url": format!("https://example.com/{index}"), "source": "来源", "tags": [],
        "importance": 3, "first_seen_at": Utc::now(), "published_at": Utc::now(),
    })).unwrap()
}

fn story(index: usize, status: StoryStatus) -> Story {
    Story {
        title: "A < B & C".into(),
        title_en: "Title".into(),
        summary: "完整事实\n\n及适用范围。".into(),
        summary_en: "Summary".into(),
        short_summary: "紧凑事实及限制。".into(),
        status,
        source_indexes: vec![index],
    }
}

fn image(url: &str, origin: &str) -> NewsImage {
    serde_json::from_value(json!({ "url": url, "origin_url": origin,
        "source_url": "https://example.com/1", "caption": "演示现场", "caption_en": "Demo",
        "credit": "官方" }))
    .unwrap()
}

#[test]
fn analysis_input_keeps_facts_and_dates_without_storage_metadata() {
    let mut source = source(7, "gossip", false);
    source.published_at = None;
    source.image = Some(image(
        "https://cdn.example.com/image.jpg",
        "https://origin.example.com/image.jpg",
    ));
    let window = NewsWindow {
        start: Utc::now() - ChronoDuration::days(1),
        end: Utc::now(),
    };
    let task = insight_task(window, &[source.clone()], &json!({"current_items":0})).unwrap();
    let line = task
        .lines()
        .find(|line| line.starts_with("最近7天新闻原文摘录"))
        .unwrap();
    let input: Value = serde_json::from_str(line.split_once('：').unwrap().1).unwrap();
    assert_eq!(input[0]["index"], 7);
    assert_eq!(input[0]["current"], false);
    assert_eq!(input[0]["topic_id"], "gossip");
    assert_eq!(input[0]["summary"], source.summary);
    assert_eq!(input[0]["url"], source.url);
    assert_eq!(input[0]["published_at"], Value::Null);
    assert_eq!(input[0]["first_seen_at"], json!(source.first_seen_at));
    for key in [
        "title_en",
        "summary_en",
        "topic_en",
        "image",
        "last_seen_at",
        "seen_count",
    ] {
        assert!(
            input[0].get(key).is_none(),
            "unexpected storage field {key}"
        );
    }
    assert!(task.contains("\"current_items\":0"));
}

#[test]
fn closes_editions_at_0630_and_keeps_half_open_windows() {
    let cutoff = NaiveTime::from_hms_opt(6, 30, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 9, 20, 22, 30, 0).unwrap();
    assert_eq!(
        due_day(
            end - ChronoDuration::seconds(1),
            chrono_tz::Asia::Shanghai,
            cutoff
        )
        .to_string(),
        "2026-09-20"
    );
    assert_eq!(
        due_day(end, chrono_tz::Asia::Shanghai, cutoff).to_string(),
        "2026-09-21"
    );
    assert_eq!(
        due_day(
            end + ChronoDuration::hours(18),
            chrono_tz::Asia::Shanghai,
            cutoff
        )
        .to_string(),
        "2026-09-21"
    );
    let window = NewsWindow {
        start: end - ChronoDuration::hours(24),
        end,
    };
    assert!(window.contains(window.start));
    assert!(!window.contains(window.start - ChronoDuration::seconds(1)));
    assert!(!window.contains(end));
}

#[test]
fn rejects_historical_or_unconfirmed_stories_as_todays_headlines() {
    let sources = vec![
        source(0, "x", true),
        source(1, "gossip", true),
        source(2, "x", false),
    ];
    let mut draft = analysis();
    draft.hotspots = Some(vec![story(0, StoryStatus::Confirmed)]);
    draft.gossip = vec![story(1, StoryStatus::Unconfirmed)];
    assert!(validate_selection(&mut draft, &sources, &[]).is_ok());
    draft.hotspots = Some(vec![story(2, StoryStatus::Confirmed)]);
    assert!(validate_selection(&mut draft, &sources, &[]).is_err());
    draft.hotspots = Some(vec![story(1, StoryStatus::Unconfirmed)]);
    assert!(validate_selection(&mut draft, &sources, &[]).is_err());
    draft.hotspots = Some(vec![story(99, StoryStatus::Confirmed)]);
    assert!(validate_selection(&mut draft, &sources, &[]).is_err());
}

#[test]
fn key_data_requires_a_news_reference() {
    let sources = vec![source(0, "cases", true)];
    let mut draft = analysis();
    draft.data_signals = vec![serde_json::from_value(json!({
        "title":"单次任务成本下降", "title_en":"Lower cost per task",
        "summary":"厂商案例：同一流程单次成本从10元降至5元。", "summary_en":"Vendor case: cost fell from 10 to 5 for the same workflow.",
        "confidence":3, "source_indexes":[]
    })).unwrap()];
    assert!(validate_selection(&mut draft, &sources, &[]).is_err());
    draft.data_signals[0].source_indexes = vec![0];
    assert!(validate_selection(&mut draft, &sources, &[]).is_ok());
    draft.data_signals[0].source_indexes = vec![99];
    assert!(validate_selection(&mut draft, &sources, &[]).is_err());
}

#[test]
fn formats_consistent_articles_without_empty_html_paragraphs() {
    let sources = vec![source(0, "x", true), source(1, "gossip", true)];
    let mut draft = analysis();
    draft.hotspots = Some(vec![
        story(0, StoryStatus::Confirmed),
        story(0, StoryStatus::Confirmed),
    ]);
    draft.gossip = vec![story(1, StoryStatus::Unconfirmed)];
    draft.headline = "模型降价与工具开放，AI 应用的成本门槛下降".into();
    draft.changes = "第一条趋势。\n\n第二条趋势。".into();
    draft.changes_source_indexes = vec![0];
    let edition_label = "XAI 日报 · 2026-09-21";
    let articles = articles::render(edition_label, &draft, &sources, &[], vec![]);
    assert_eq!(articles.title, draft.headline);
    assert!(
        articles
            .wechat
            .starts_with(&format!("# {}\n\n", draft.headline))
    );
    assert!(articles.x.starts_with(&format!("{}\n\n", draft.headline)));
    assert!(
        articles
            .x_html
            .starts_with(&format!("<h1>{}</h1>", draft.headline))
    );
    for text in [&articles.wechat, &articles.x, &articles.x_html] {
        let sections = ["今日热点", "AI 圈热议", "趋势分析", "参考来源"]
            .map(|label| text.find(label).unwrap());
        assert!(sections.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!text.contains("图片新闻"));
        assert!(!text.contains("\n\n\n"));
        assert_eq!(text.matches(edition_label).count(), 1);
        assert!(text.find(&draft.headline) < text.find(edition_label));
        assert!(text.find(edition_label) < text.find("今日热点"));
        assert!(!text.contains("本站 AI 专栏样本"));
        assert!(!text.contains("新闻："));
        assert!(!text.contains("最近 7 天"));
    }
    assert!(articles.wechat.contains("完整事实 及适用范围。"));
    assert!(articles.x.contains("紧凑事实及限制。\n2."));
    assert!(articles.x.contains("【未证实】"));
    assert!(articles.x_html.contains("A &lt; B &amp; C"));
    assert!(!articles.x_html.contains("<p></p>"));
    assert!(!articles.x_html.contains("<br"));
    assert_eq!(
        articles
            .x_html
            .matches("href=\"https://example.com/0\"")
            .count(),
        1
    );
    assert_eq!(cited_sources(&draft, sources, &[]).len(), 2);
}

#[test]
fn image_selection_is_explicit_and_survives_mirror_url_changes() {
    let mut sources = vec![
        source(0, "p", true),
        source(1, "p", true),
        source(2, "p", false),
    ];
    sources[0].image = Some(image(
        "https://cdn.example.com/old",
        "https://origin.example.com/a",
    ));
    sources[1].image = Some(image(
        "https://cdn.example.com/new",
        "https://origin.example.com/a",
    ));
    sources[1].image.as_mut().unwrap().content_hash = Some("newly-computed-hash".into());
    sources[2].image = Some(image(
        "https://cdn.example.com/b",
        "https://origin.example.com/b",
    ));
    let mut draft = analysis();
    assert!(selected_article_images(&draft, &sources).is_empty());
    draft.cover_source_index = Some(1);
    draft.picture_source_indexes = vec![0, 2];
    let chosen = selected_article_images(&draft, &sources);
    assert_eq!(chosen.len(), 2);
    assert!(chosen[0].cover);
    assert_eq!(chosen[0].url, "https://cdn.example.com/new");
    assert!(chosen[1].archived);
    let articles = articles::render("XAI 日报 · 2026-09-21", &draft, &sources, &[], chosen);
    assert!(
        articles
            .wechat
            .contains("![演示现场](<https://cdn.example.com/new>)")
    );
    assert!(articles.wechat.contains("资料图"));
    assert!(articles.wechat.find("图片新闻") < articles.wechat.find("参考来源"));
}

#[test]
fn saved_structured_drafts_use_the_current_title_and_layout() {
    let mut draft = analysis();
    draft.headline = "模型降价与工具开放，AI 应用的成本门槛下降".into();
    draft.hotspots = Some(vec![story(0, StoryStatus::Confirmed)]);
    let stored: StoredInsight = serde_json::from_value(json!({
        "analysis": draft,
        "sources": [source(0, "x", true)],
        "stats": {"current_items":32,"seven_day_items":213,"observed_days":5},
        "articles": {
            "title": "XAI 日报 · 2026-09-21",
            "wechat": "# XAI 日报 · 2026-09-21\n\n新闻：09-20 06:30—09-21 06:30（Asia/Shanghai） · 趋势：最近 7 天\n\n本站 AI 专栏样本：本期 32 条 · 近7天 213 条（有收录 5 天，按原文链接去重）。",
            "x": "旧X稿及时间和采样说明",
            "x_html": "<h1>XAI 日报 · 2026-09-21</h1><p>旧时间和采样说明</p>"
        }
    })).unwrap();
    let articles = current_articles(&stored, NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
    assert_eq!(articles.title, stored.analysis.headline);
    for text in [&articles.wechat, &articles.x, &articles.x_html] {
        assert!(text.contains(&stored.analysis.headline));
        assert!(text.contains("XAI 日报 · 2026-09-21"));
        assert!(text.contains("今日热点"));
        assert!(!text.contains("旧"));
        assert!(!text.contains("本站 AI 专栏样本"));
        assert!(!text.contains("Asia/Shanghai"));
        assert!(!text.contains("最近 7 天"));
    }
    assert!(articles.wechat.contains("完整事实 及适用范围。"));
    assert!(articles.x.contains("紧凑事实及限制。"));
    assert_eq!(stored.stats.unwrap().current_items, 32);
}

#[test]
fn legacy_drafts_remain_readable() {
    let mut old = serde_json::to_value(analysis()).unwrap();
    old.as_object_mut().unwrap().remove("hotspots");
    let stored: StoredInsight = serde_json::from_value(json!({
        "analysis": old, "sources": [], "articles": { "wechat": "旧公众号稿", "x": "旧X稿" }
    }))
    .unwrap();
    assert!(stored.analysis.hotspots.is_none());
    assert_eq!(stored.articles.x, "旧X稿");
    assert!(stored.articles.x_html.is_empty());
    assert!(stored.stats.is_none());
    let articles = current_articles(&stored, NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
    assert_eq!(articles.wechat, "旧公众号稿");
    assert_eq!(articles.x, "旧X稿");
    assert!(articles.x_html.is_empty());
}

#[test]
fn only_actual_requested_research_citations_are_published() {
    let reply = json!({ "output": [{"type": "message", "content": [{"type": "output_text", "annotations": [
        {"type": "url_citation", "url": "https://example.com/research", "title": "Research"},
        {"type": "url_citation", "url": "https://example.com/research", "title": "Duplicate"},
        {"type": "url_citation", "url": "http://example.com/other", "title": "Other"}
    ]}]}] });
    let mut draft = analysis();
    draft.research_source_urls = vec![
        "https://example.com/research".into(),
        "https://example.com/invented".into(),
    ];
    let references = selected_research_sources(&mut draft, research_citations(&reply));
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].title, "Research");
}

#[test]
fn cache_refuses_renderings_from_before_an_invalidation() {
    let cache = InsightCache::default();
    let generation = cache.generation();
    cache.invalidate();
    cache.store(generation, Arc::new(Rendered::new("{}".into())));
    assert!(cache.get().is_none());
    cache.store(cache.generation(), Arc::new(Rendered::new("{}".into())));
    assert!(cache.get().is_some());
}

async fn insert_story(
    pool: &sqlx::PgPool,
    topic: &str,
    slug: &str,
    published: Option<DateTime<Utc>>,
    seen: DateTime<Utc>,
) {
    use sha2::{Digest, Sha256};
    let url = format!("https://example.com/{slug}");
    sqlx::query("INSERT INTO items (id,topic_id,day,fingerprint,title,summary,url,published_at,first_seen_at,tags)
        VALUES ($1,$2,$3,$4,$5,'事实摘要',$6,$7,$8,ARRAY['AI','AI'])")
        .bind(Uuid::new_v4()).bind(topic).bind(seen.date_naive())
        .bind(Sha256::digest(url.as_bytes()).to_vec()).bind(slug).bind(url).bind(published).bind(seen)
        .execute(pool).await.unwrap();
}

#[tokio::test]
#[ignore = "requires NEWS_DAILY_TEST_DATABASE_URL and local test configuration"]
async fn postgres_daily_pipeline_retries_with_one_slot_and_counts_full_seven_days() {
    use axum::{Router, routing::post};
    use std::sync::{Mutex, atomic::AtomicBool};
    let database_url =
        std::env::var("NEWS_DAILY_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_daily_test"));
    let mut config = crate::config::Config::from_env().unwrap();
    config.database_url = database_url;
    config.max_concurrent_runs = 1;
    config.tmpfile_base_url = None;
    config.redis_addr = None;
    let day = due_day(Utc::now(), config.timezone, config.briefing_time);
    let end = crate::topics::to_utc(config.timezone, day.and_time(config.briefing_time));
    let window = NewsWindow {
        start: end - ChronoDuration::days(1),
        end,
    };
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let fail_once = Arc::new(AtomicBool::new(true));
    let fail_analysis = Arc::new(AtomicBool::new(false));
    let handler_fail_analysis = fail_analysis.clone();
    let handler_calls = calls.clone();
    let app = Router::new().route("/v1/responses", post(move |Json(body): Json<Value>| {
        let calls = handler_calls.clone();
        let fail_once = fail_once.clone();
        let fail_analysis = handler_fail_analysis.clone();
        async move {
            calls.lock().unwrap().push(body.clone());
            if body["text"]["format"]["name"] == "news_insight" && fail_analysis.load(Ordering::Relaxed) {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "analysis fixture failure"})));
            }
            if body["text"]["format"]["name"] == "news_digest" && body["input"].as_str().unwrap().contains("栏目方向：github\n")
                && fail_once.swap(false, Ordering::Relaxed) {
                return (StatusCode::BAD_REQUEST, Json(json!({"error": "temporary fixture failure"})));
            }
            let payload = if body["text"]["format"]["name"] == "news_digest" {
                assert!(body["input"].as_str().unwrap().contains(&window.end.to_rfc3339()));
                let items = if body["input"].as_str().unwrap().contains("栏目方向：x\n") {
                    [("采集的新事实", end - ChronoDuration::hours(1)), ("截止后新闻", end), ("过期新闻", window.start - ChronoDuration::seconds(1))]
                    .into_iter().map(|(title, at)| json!({"title": title, "title_en": title,
                        "summary": "这是独立的新事实", "summary_en": "A distinct new fact", "source": "官方", "importance": 4,
                        "url": format!("https://example.com/collected/{}", at.timestamp()), "published_at": at,
                        "tags": ["AI"], "tags_en": ["AI"], "image": null })).collect::<Vec<_>>()
                } else { vec![] };
                json!({"items": items, "cover": null})
            } else {
                let text = body["input"][0]["content"][0]["text"].as_str().unwrap();
                let source_line = text.lines().find(|line| line.starts_with("最近7天新闻原文摘录")).unwrap();
                let sources: Vec<Value> = serde_json::from_str(source_line.split_once('：').unwrap().1).unwrap();
                let current = sources.iter().find(|s| s["current"] == true && s["topic_id"] == "x").unwrap();
                let old = sources.iter().find(|s| s["current"] == false && !s["published_at"].is_null()).unwrap();
                let gossip = sources.iter().find(|s| s["topic_id"] == "gossip").unwrap();
                let picture = sources.iter().find(|s| s["topic_id"] == "p").unwrap();
                assert_eq!(body["input"][0]["content"].as_array().unwrap().iter().filter(|part| part["type"] == "input_image").count(), 1);
                let mut draft = analysis();
                draft.hotspots = Some(vec![story(current["index"].as_u64().unwrap() as usize, StoryStatus::Confirmed)]);
                draft.gossip = vec![story(gossip["index"].as_u64().unwrap() as usize, StoryStatus::Unconfirmed)];
                draft.changes = "七天内的变化。".into();
                draft.changes_source_indexes = vec![old["index"].as_u64().unwrap() as usize];
                draft.cover_source_index = Some(picture["index"].as_u64().unwrap() as usize);
                serde_json::to_value(draft).unwrap()
            };
            (StatusCode::OK, Json(json!({"status": "completed", "id": "test-response",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": payload.to_string()}]}]})))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.ai_base_url = format!("http://{}", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut state = AppState::new(config).await.unwrap();
    sqlx::query("TRUNCATE items, runs, ai_insights CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET enabled = id IN ('x','github','p','gossip','events','codex'), brief=id, last_run_at=NULL, last_status=NULL, daily_run_cap=24")
        .execute(&state.pool).await.unwrap();
    let private_topic = format!("audit-{}", &Uuid::new_v4().simple().to_string()[..12]);
    let owner: i64 = sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(format!("{private_topic}@example.com"))
        .fetch_one(&state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO topics (id,owner_user_id,name,brief) VALUES ($1,$2,'Private','Private')",
    )
    .bind(&private_topic)
    .bind(owner)
    .execute(&state.pool)
    .await
    .unwrap();
    insert_story(
        &state.pool,
        &private_topic,
        "private-story",
        Some(end - ChronoDuration::hours(1)),
        end,
    )
    .await;
    insert_story(
        &state.pool,
        "x",
        "hidden-story",
        Some(end - ChronoDuration::hours(1)),
        end,
    )
    .await;
    sqlx::query("UPDATE items SET hidden=true WHERE title='hidden-story'")
        .execute(&state.pool)
        .await
        .unwrap();
    for (topic, slug, at) in [
        ("x", "today", end - ChronoDuration::hours(1)),
        ("github", "today", end - ChronoDuration::hours(1)),
        ("github", "zero-baseline", end - ChronoDuration::hours(2)),
        ("x", "start", window.start),
        ("x", "earlier", end - ChronoDuration::days(2)),
        ("x", "seven-day-start", end - ChronoDuration::days(7)),
        (
            "x",
            "too-old",
            end - ChronoDuration::days(7) - ChronoDuration::seconds(1),
        ),
        ("x", "future", end),
        ("gossip", "rumour", end - ChronoDuration::hours(3)),
        ("p", "picture", end - ChronoDuration::hours(4)),
        ("openai", "disabled", end - ChronoDuration::hours(5)),
        ("events", "world-event", end - ChronoDuration::hours(1)),
        ("events", "past-world-event", end - ChronoDuration::days(2)),
        ("codex", "feedback", end - ChronoDuration::hours(1)),
        ("codex", "past-feedback", end - ChronoDuration::days(2)),
        ("codex", "today", end - ChronoDuration::hours(1)),
    ] {
        insert_story(&state.pool, topic, slug, Some(at), at).await;
    }
    insert_story(
        &state.pool,
        "x",
        "undated",
        None,
        end - ChronoDuration::hours(2),
    )
    .await;
    // An undated AI-column copy must not pull a gossip report into the statistics.
    insert_story(
        &state.pool,
        "x",
        "rumour",
        None,
        end - ChronoDuration::hours(2),
    )
    .await;
    sqlx::query("UPDATE items SET image = $1 WHERE topic_id = 'p'")
        .bind(json!({"url": "https://cdn.example.com/robot.jpg", "source_url": "https://example.com/picture",
            "caption": "机器人演示（测试图）", "caption_en": "Robot demo (fixture)", "credit": "演示团队"}))
        .execute(&state.pool).await.unwrap();
    // Even a higher-ranked copy outside the scope must not displace an eligible source.
    sqlx::query("UPDATE items SET importance=5 WHERE topic_id='codex'")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE items SET tags=tags || ARRAY['Agent','agent','AI Agent','智能体','AI代理'] WHERE title IN ('today','earlier')")
        .execute(&state.pool).await.unwrap();
    let stats = collect_stats(&state, window).await.unwrap();
    assert_eq!(stats["topic_ids"], json!(STAT_TOPICS));
    assert_eq!(stats["seven_day_items"], 5);
    assert_eq!(stats["current_items"], 3);
    assert_eq!(stats["observed_days"], 3);
    assert_eq!(stats["daily"].as_array().unwrap().len(), 7);
    assert_eq!(stats["tags"][0]["current_items"], 3);
    let tags = stats["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 2);
    let agent = tags.iter().find(|tag| tag["tag"] == "Agent").unwrap();
    assert_eq!(agent["current_items"], 1);
    assert_eq!(agent["baseline_daily_average"], json!(0.17));
    assert!(
        stats["topics"]
            .as_array()
            .unwrap()
            .iter()
            .all(|topic| { STAT_TOPICS.contains(&topic["topic_id"].as_str().unwrap()) })
    );
    assert_eq!(stats["baseline_daily_average"], json!(0.33));
    let zero = stats["topics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|topic| topic["topic_id"] == "github")
        .unwrap();
    assert!(zero["change_percent"].is_null());
    let attempt = tokio::time::timeout(
        Duration::from_secs(10),
        runner::collect_briefing(&state, window),
    )
    .await
    .unwrap();
    assert!(attempt.is_err());
    tokio::time::timeout(
        Duration::from_secs(10),
        runner::collect_briefing(&state, window),
    )
    .await
    .unwrap()
    .unwrap();
    let completed = execute(&state, window.start, window.end).await.unwrap();
    let stored: StoredInsight = serde_json::from_value(completed.payload.clone()).unwrap();
    assert_eq!(stored.stats.as_ref().unwrap().current_items, 4);
    assert_eq!(stored.stats.as_ref().unwrap().seven_day_items, 6);
    assert_eq!(stored.articles.title, stored.analysis.headline);
    for text in [
        &stored.articles.wechat,
        &stored.articles.x,
        &stored.articles.x_html,
    ] {
        assert!(text.contains(&stored.analysis.headline));
        assert!(!text.contains("本站 AI 专栏样本"));
        assert!(!text.contains("新闻："));
        assert!(!text.contains("最近 7 天"));
    }
    assert_eq!(stored.analysis.hotspots.as_ref().unwrap().len(), 1);
    assert_eq!(stored.articles.images.len(), 1);
    assert!(stored.articles.images[0].cover);
    assert!(stored.articles.wechat.contains("图片新闻"));
    assert!(
        stored
            .sources
            .iter()
            .any(|s| s.title == "采集的新事实" && s.current)
    );
    assert!(!stored.sources.iter().any(|s| matches!(
        s.title.as_str(),
        "截止后新闻" | "过期新闻" | "private-story" | "hidden-story"
    )));
    assert!(
        stored
            .sources
            .iter()
            .any(|source| source.topic_id == "codex")
    );
    assert!(
        stored
            .sources
            .iter()
            .any(|source| source.topic_id == "events")
    );
    assert_eq!(calls.lock().unwrap().len(), 8); // Six columns, one failed-column retry, one analysis.
    // Drive the actual coordinator, including slot acquisition and atomic publication.
    let id = start_generation(&state, day, end).await.unwrap();
    assert!(matches!(
        start_generation(&state, day, end).await,
        Err(AppError::Conflict(_))
    ));
    wait_for_generation(&state).await;
    let status: String = sqlx::query_scalar("SELECT status FROM ai_insights WHERE id=$1")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(status, "ok");
    assert_eq!(state.run_slots.available_permits(), 1);
    assert_eq!(calls.lock().unwrap().len(), 9); // Manual regeneration reuses the same collection.
    let rendered = render_latest(&state).await.unwrap();
    let public: Value = serde_json::from_slice(&rendered.body).unwrap();
    assert_eq!(public["insight"]["window_end"], json!(end));
    assert_eq!(
        public["insight"]["trend_window_start"],
        json!(end - ChronoDuration::days(7))
    );
    assert_eq!(public["insight"]["hotspots"].as_array().unwrap().len(), 1);
    assert_eq!(public["insight"]["images"].as_array().unwrap().len(), 1);
    assert_eq!(
        public["insight"]["sampling"],
        json!({"current_items":4,"seven_day_items":6,"observed_days":3})
    );
    assert!(public["insight"].get("wechat_article").is_none());
    tick(&state).await.unwrap();
    assert!(!is_running(&state));
    assert_eq!(calls.lock().unwrap().len(), 9);

    // A failed manual regeneration keeps the previously published edition intact.
    fail_analysis.store(true, Ordering::Relaxed);
    let failed_id = start_generation(&state, day, end).await.unwrap();
    wait_for_generation(&state).await;
    let failed: String = sqlx::query_scalar("SELECT status FROM ai_insights WHERE id=$1")
        .bind(failed_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(failed, "failed");
    assert_eq!(render_latest(&state).await.unwrap().body, rendered.body);
    assert_eq!(calls.lock().unwrap().len(), 10);

    // Scheduler retries only after the cooldown and uses the original cutoff.
    sqlx::query("DELETE FROM ai_insights WHERE status='ok'")
        .execute(&state.pool)
        .await
        .unwrap();
    tick(&state).await.unwrap();
    assert!(!is_running(&state));
    assert_eq!(calls.lock().unwrap().len(), 10);
    sqlx::query("UPDATE ai_insights SET finished_at=NOW()-INTERVAL '11 minutes' WHERE id=$1")
        .bind(failed_id)
        .execute(&state.pool)
        .await
        .unwrap();
    fail_analysis.store(false, Ordering::Relaxed);
    tick(&state).await.unwrap();
    wait_for_generation(&state).await;
    assert_eq!(calls.lock().unwrap().len(), 11);
    let recovered_end: DateTime<Utc> =
        sqlx::query_scalar("SELECT window_end FROM ai_insights WHERE status='ok'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(recovered_end, end);

    // Daily collection must continue to respect the editor's configured cost cap.
    sqlx::query("UPDATE topics SET daily_run_cap=1 WHERE id='github'")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE runs SET triggered_by='manual', window_end=NULL WHERE topic_id='github'")
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(
        runner::collect_briefing(&state, window)
            .await
            .unwrap_err()
            .to_string()
            .contains("上限")
    );
    assert_eq!(calls.lock().unwrap().len(), 11);

    // A small input budget reproduces the same pressure as many full public columns.
    let sampled = sqlx::query_as::<_, SourceRow>(include_str!("sources.sql"))
        .bind(end - ChronoDuration::days(7))
        .bind(window.start)
        .bind(end)
        .bind(4_i64)
        .fetch_all(&state.pool)
        .await
        .unwrap();
    assert!(
        sampled
            .iter()
            .any(|s| s.published_at.is_some_and(|at| at < window.start)),
        "current news displaced all historical trend evidence"
    );
    assert!(
        sampled
            .iter()
            .any(|s| s.published_at.is_some_and(|at| window.contains(at)))
    );
    state.shutdown_workers().await;
    check_media_renewal(&mut state).await;
    check_media_limits(state).await;
    server.abort();
}

async fn check_media_renewal(state: &mut AppState) {
    Arc::get_mut(&mut state.config).unwrap().tmpfile_base_url =
        Some("https://mirror.example.com".parse().unwrap());
    let (id, mut payload): (Uuid, Value) =
        sqlx::query_as("SELECT id,payload FROM ai_insights WHERE status='ok'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    let index = payload["analysis"]["cover_source_index"].as_u64().unwrap() as usize;
    let mut old = media::stored_image(&payload["sources"][index]["image"]).unwrap();
    old.origin_url = Some(old.url.clone());
    old.url = "https://mirror.example.com/expired.jpg".into();
    old.expires_at = Some(Utc::now() - ChronoDuration::hours(1));
    let mut fresh = old.clone();
    fresh.url = "https://mirror.example.com/renewed.jpg".into();
    fresh.expires_at = Some(Utc::now() + ChronoDuration::days(1));
    fresh.caption = "Different story's caption".into();
    fresh.credit = "Different publisher".into();
    fresh.content_hash = Some("same-photo".into());
    payload["sources"][index]["image"] = json!(old);
    payload["articles"]["images"][0]["url"] = json!(old.url);
    payload["articles"]["wechat"] = json!(format!("![photo]({})", old.url));
    sqlx::query("UPDATE ai_insights SET payload=$2 WHERE id=$1")
        .bind(id)
        .bind(payload)
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE items SET image=$1 WHERE topic_id='p'")
        .bind(json!(old))
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE runs SET payload=jsonb_set(payload,'{cover}',$1) WHERE topic_id='p'")
        .bind(json!(fresh))
        .execute(&state.pool)
        .await
        .unwrap();
    let day: NaiveDate = sqlx::query_scalar("SELECT day FROM items WHERE topic_id='p'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    state.digest.refresh(state, "p", day).await.unwrap();
    let before_feed = state.digest.snapshot(state, "p", day).await.unwrap();
    let before = Arc::new(render_latest(state).await.unwrap());
    state
        .insight
        .store(state.insight.generation(), before.clone());

    // The fresh run cover renews both expired copies without any network transfer.
    tokio::time::timeout(Duration::from_secs(2), media::startup_sync(state))
        .await
        .unwrap()
        .unwrap();
    assert!(state.insight.get().is_none());
    let after_feed = state.digest.snapshot(state, "p", day).await.unwrap();
    assert_eq!(after_feed.revision, before_feed.revision + 1);
    let after = render_latest(state).await.unwrap();
    assert_ne!(before.etag, after.etag);
    let before: Value = serde_json::from_slice(&before.body).unwrap();
    let after: Value = serde_json::from_slice(&after.body).unwrap();
    assert_eq!(
        before["insight"]["generated_at"],
        after["insight"]["generated_at"]
    );
    assert_eq!(after["insight"]["images"][0]["url"], fresh.url);
    assert_eq!(after["insight"]["images"][0]["caption"], old.caption);
    assert_eq!(after["insight"]["images"][0]["credit"], old.credit);
    let stored: Value = sqlx::query_scalar("SELECT payload FROM ai_insights WHERE id=$1")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(
        stored["articles"]["wechat"]
            .as_str()
            .unwrap()
            .contains(&fresh.url)
    );
    media::startup_sync(state).await.unwrap();
    assert_eq!(
        state
            .digest
            .snapshot(state, "p", day)
            .await
            .unwrap()
            .revision,
        after_feed.revision
    );
}

async fn check_media_limits(mut state: AppState) {
    // A local socket that never completes TLS represents an unresponsive image host.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    state.image_http = reqwest::Client::builder()
        .no_proxy()
        .resolve("slow.example.com", addr)
        .build()
        .unwrap();
    Arc::get_mut(&mut state.config).unwrap().image_timeout = Duration::from_millis(100);
    let slow = |id| {
        let url = format!("https://slow.example.com:{}/image/{id}", addr.port());
        Some(image(&url, &url))
    };
    let mut duplicates = vec![slow(0), slow(0), slow(0)];
    tokio::time::timeout(Duration::from_secs(2), async {
        let (_, accepted) = tokio::join!(
            media::mirror_images(&state, &mut duplicates),
            listener.accept()
        );
        drop(accepted.unwrap());
    })
    .await
    .unwrap();
    assert!(duplicates.iter().all(Option::is_none));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(state.image_slots.available_permits(), 3);

    Arc::get_mut(&mut state.config).unwrap().image_timeout = Duration::from_secs(5);
    let mut tasks = tokio::task::JoinSet::new();
    for batch in 0..2 {
        let state = state.clone();
        let mut images = (0..4)
            .map(|index| slow(1 + batch * 4 + index))
            .collect::<Vec<_>>();
        tasks.spawn(async move {
            media::mirror_images(&state, &mut images).await;
            images
        });
    }
    let mut sockets = Vec::new();
    for _ in 0..3 {
        sockets.push(
            tokio::time::timeout(Duration::from_secs(1), listener.accept())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert_eq!(state.image_slots.available_permits(), 0);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    state.shutdown.cancel();
    while let Some(result) = tasks.join_next().await {
        assert!(result.unwrap().iter().all(Option::is_none));
    }
    drop(
        tokio::time::timeout(Duration::from_secs(1), state.image_slots.acquire_many(3))
            .await
            .unwrap()
            .unwrap(),
    );
    assert_eq!(state.image_slots.available_permits(), 3);
}

async fn wait_for_generation(state: &AppState) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while is_running(state) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
