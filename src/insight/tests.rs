use super::*;
use articles::StoryStatus;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::TimeZone;
use std::time::Duration;

fn analysis() -> InsightAnalysis {
    serde_json::from_value(json!({
        "hotspots": [], "headline": "新闻", "headline_en": "News",
        "gossip": [], "picture_source_indexes": [], "changes": "", "changes_en": "",
        "figures": [], "watch_signals": [], "changes_source_indexes": [], "outlook": []
    }))
    .unwrap()
}

fn source(index: usize, topic: &str, current: bool) -> InsightSource {
    serde_json::from_value(json!({
        "index": index, "current": current, "topic_id": topic, "topic": topic, "topic_en": topic,
        "last_seen_at": Utc::now(), "seen_count": 1,
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
        angle: String::new(),
        angle_en: String::new(),
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
fn equity_runs_twice_daily_across_dst_changes() {
    let timezone = chrono_tz::Asia::Shanghai;
    let time = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    for (date, previous_hour, hour) in [
        ("2026-03-07", 13, 13),
        ("2026-03-08", 13, 12),
        ("2026-09-29", 12, 12),
        ("2026-11-01", 12, 13),
        ("2026-11-02", 13, 13),
    ] {
        let day: NaiveDate = date.parse().unwrap();
        let morning = crate::topics::to_utc(timezone, day.and_time(time));
        let evening = day.and_hms_opt(hour, 45, 0).unwrap().and_utc();
        let previous = day
            .pred_opt()
            .unwrap()
            .and_hms_opt(previous_hour, 45, 0)
            .unwrap()
            .and_utc();
        for (now, expected) in [
            (morning - ChronoDuration::seconds(1), previous),
            (morning, morning),
            (evening - ChronoDuration::seconds(1), morning),
            (evening, evening),
            (
                morning + ChronoDuration::hours(24) - ChronoDuration::seconds(1),
                evening,
            ),
        ] {
            assert_eq!(Kind::Equity.cutoff(now, timezone, time), expected, "{now}");
        }
        assert_eq!(Kind::Ai.cutoff(evening, timezone, time), morning);
    }
}

#[test]
fn closes_editions_at_0600_and_keeps_half_open_windows() {
    let cutoff = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 9, 20, 22, 0, 0).unwrap();
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
    draft.hotspots = vec![story(0, StoryStatus::Confirmed)];
    draft.gossip = vec![story(1, StoryStatus::Unconfirmed)];
    assert!(validate_selection(&draft, &sources).is_ok());
    draft.hotspots = vec![story(2, StoryStatus::Confirmed)];
    assert!(validate_selection(&draft, &sources).is_err());
    draft.hotspots = vec![story(1, StoryStatus::Unconfirmed)];
    assert!(validate_selection(&draft, &sources).is_err());
    draft.hotspots = vec![story(99, StoryStatus::Confirmed)];
    assert!(validate_selection(&draft, &sources).is_err());
}

#[test]
fn figures_require_a_news_reference_that_states_their_numbers() {
    let mut sources = vec![source(0, "cases", true), source(1, "biz", true)];
    sources[0].summary = "厂商案例：同一流程单次成本从10元降至5.5元，覆盖1,000多名用户。".into();
    let figure = |value: &str, label: &str, source_indexes: &[usize]| -> InsightFigure {
        serde_json::from_value(json!({ "value": value, "value_en": value, "label": label,
            "label_en": label, "source_indexes": source_indexes }))
        .unwrap()
    };
    let mut draft = analysis();
    draft.figures = vec![figure("5.5 元", "同一流程的单次成本，原为10元", &[])];
    let error = validate_selection(&draft, &sources)
        .unwrap_err()
        .to_string();
    assert!(error.contains("figures[0].source_indexes"));
    assert!(error.contains("来源数=2"));
    draft.figures = vec![
        figure("5.5 元", "同一流程的单次成本，原为10元", &[0]),
        // Numbers stand whole: 5 is not 5.5, and the other source does not state it.
        figure("5 元", "同一流程的单次成本", &[0]),
        figure("5.5 元", "同一流程的单次成本", &[1]),
        figure("1,000 多名", "用户", &[0]),
        // A label's numbers are checked too, and a figure must state one.
        figure("5.5 元", "比去年的20元低", &[0]),
        figure("约一半", "成本降幅", &[0]),
        figure("1,000 多名", "用户", &[99]),
    ];
    normalize_figures(&mut draft, &sources);
    assert_eq!(
        draft
            .figures
            .iter()
            .map(|figure| figure.value.as_str())
            .collect::<Vec<_>>(),
        ["5.5 元", "1,000 多名"]
    );
    assert!(validate_selection(&draft, &sources).is_ok());
}

#[test]
fn optional_signal_citations_must_reference_available_sources() {
    let sources = vec![source(0, "x", true)];
    let signal = json!({
        "title":"观察", "title_en":"Watch", "summary":"后续观察", "summary_en":"Follow-up",
        "kind":"risk", "source_indexes":[]
    });
    for field in ["watch_signals", "outlook"] {
        let mut payload = serde_json::to_value(analysis()).unwrap();
        payload[field] = json!([signal]);
        let draft = serde_json::from_value(payload.clone()).unwrap();
        validate_selection(&draft, &sources).unwrap();
        payload[field][0]["source_indexes"] = json!([99]);
        let draft = serde_json::from_value(payload).unwrap();
        let error = validate_selection(&draft, &sources)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("{field}[0].source_indexes")));
    }
    let schema = schema(&sources, &[]);
    let outlook = &schema["properties"]["outlook"]["items"];
    assert_eq!(outlook["properties"]["kind"]["enum"], json!(OUTLOOK_KINDS));
    assert_eq!(outlook["properties"]["source_indexes"]["minItems"], 1);
    assert!(
        schema["properties"]["watch_signals"]["items"]["properties"]
            .get("kind")
            .is_none()
    );
}

#[test]
fn invalid_optional_images_do_not_discard_valid_prose() {
    let sources = vec![
        source(0, "x", true),
        source(7, "p", true),
        source(9, "p", true),
    ];
    let mut draft = analysis();
    draft.hotspots = vec![story(0, StoryStatus::Confirmed)];
    draft.cover_source_index = Some(999);
    draft.picture_source_indexes = vec![0, 999, 7, 7, 9];
    normalize_images(&mut draft, &[7, 9]);
    validate_selection(&draft, &sources).unwrap();
    assert_eq!(draft.cover_source_index, None);
    assert_eq!(draft.picture_source_indexes, vec![7, 9]);
    assert_eq!(draft.hotspots[0].source_indexes, vec![0]);

    draft.cover_source_index = Some(7);
    normalize_images(&mut draft, &[7, 9]);
    assert_eq!(draft.cover_source_index, Some(7));
    assert_eq!(draft.picture_source_indexes, vec![9]);

    normalize_images(&mut draft, &[]);
    assert_eq!(draft.cover_source_index, None);
    assert!(draft.picture_source_indexes.is_empty());
    draft.changes_source_indexes = vec![999];
    let error = validate_selection(&draft, &sources)
        .unwrap_err()
        .to_string();
    assert!(error.contains("changes_source_indexes"));
    assert!(error.contains("999"));
}

#[test]
fn schema_limits_citations_to_original_indexes_and_available_images() {
    let sources = vec![
        source(2, "x", true),
        source(5, "gossip", true),
        source(7, "x", false),
        source(9, "p", true),
    ];
    let schema = schema(&sources, &[9]);
    assert_eq!(schema["$defs"]["source_index"]["enum"], json!([2, 5, 7, 9]));
    let properties = &schema["properties"];
    assert_eq!(
        properties["hotspots"]["items"]["properties"]["source_indexes"]["items"]["enum"],
        json!([2])
    );
    assert_eq!(
        properties["gossip"]["items"]["properties"]["source_indexes"]["items"]["enum"],
        json!([5])
    );
    assert_eq!(properties["cover_source_index"]["enum"], json!([9, null]));
    assert_eq!(
        properties["picture_source_indexes"]["items"]["enum"],
        json!([9])
    );
    assert_eq!(schema["additionalProperties"], false);
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("outlook"))
    );
    let required = |list: &str| properties[list]["items"]["required"].clone();
    assert!(
        required("hotspots")
            .as_array()
            .unwrap()
            .contains(&json!("angle_en"))
    );
    assert!(
        !required("gossip")
            .as_array()
            .unwrap()
            .contains(&json!("angle"))
    );
    assert!(
        required("outlook")
            .as_array()
            .unwrap()
            .contains(&json!("kind"))
    );
    assert!(
        !required("watch_signals")
            .as_array()
            .unwrap()
            .contains(&json!("kind"))
    );
    assert_eq!(
        properties["figures"]["items"]["additionalProperties"],
        false
    );
}

#[test]
fn schema_allows_quiet_days_without_inventing_sources_or_images() {
    for sources in [vec![], vec![source(7, "x", false)]] {
        let schema = schema(&sources, &[]);
        let properties = &schema["properties"];
        assert_eq!(properties["hotspots"]["maxItems"], 0);
        assert_eq!(properties["gossip"]["maxItems"], 0);
        assert_eq!(properties["cover_source_index"]["enum"], json!([null]));
        assert_eq!(properties["picture_source_indexes"]["maxItems"], 0);
        assert!(!schema.to_string().contains("\"enum\":[]"));
        if sources.is_empty() {
            for field in [
                "figures",
                "watch_signals",
                "outlook",
                "changes_source_indexes",
            ] {
                assert_eq!(properties[field]["maxItems"], 0, "{field}");
            }
        }
        validate_selection(&analysis(), &sources).unwrap();
    }
}

#[test]
fn formats_consistent_articles_without_empty_html_paragraphs() {
    let sources = vec![
        source(0, "x", true),
        source(1, "gossip", true),
        source(2, "cases", true),
        source(3, "biz", true),
    ];
    let mut draft = analysis();
    draft.hotspots = vec![
        story(0, StoryStatus::Confirmed),
        story(0, StoryStatus::Confirmed),
    ];
    draft.gossip = vec![story(1, StoryStatus::Unconfirmed)];
    draft.hotspots[0].angle = "调用门槛随之下降。".into();
    draft.headline = "模型降价与工具开放，AI 应用的成本门槛下降".into();
    draft.changes = "第一条趋势。\n\n第二条趋势。".into();
    draft.changes_source_indexes = vec![0];
    draft.figures = vec![
        serde_json::from_value(json!({ "value": "5.5 元", "value_en": "5.5 yuan",
            "label": "同一流程的单次成本", "label_en": "Cost per run", "source_indexes": [3] }))
        .unwrap(),
    ];
    draft.outlook = vec![
        serde_json::from_value(json!({ "kind": "risk", "title": "旧接口下线",
            "title_en": "Old API retires", "summary": "依赖旧接口的应用需要迁移。",
            "summary_en": "Apps on the old API must migrate.", "source_indexes": [2] }))
        .unwrap(),
    ];
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
        let sections = [
            "趋势分析",
            "今日热点",
            "今日数字",
            "AI 圈热议",
            "机会与风险",
            "参考来源",
        ]
        .map(|label| text.find(label).unwrap());
        assert!(sections.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!text.contains("图片新闻"));
        assert!(!text.contains("\n\n\n"));
        assert_eq!(text.matches(edition_label).count(), 1);
        assert!(text.find(&draft.headline) < text.find(edition_label));
        assert!(text.find(edition_label) < text.find("今日热点"));
        assert!(!text.contains("采样说明"));
        assert!(!text.contains("新闻："));
        assert!(!text.contains("最近 7 天"));
    }
    assert!(
        articles
            .wechat
            .contains("完整事实 及适用范围。 看点：调用门槛随之下降。")
    );
    assert!(
        articles
            .x
            .contains("紧凑事实及限制。 看点：调用门槛随之下降。\n2.")
    );
    assert!(articles.x.contains("紧凑事实及限制。\n\n今日数字"));
    assert!(articles.x.contains("【未证实】"));
    assert!(articles.x.contains("1. 5.5 元 — 同一流程的单次成本"));
    assert!(
        articles
            .x
            .contains("1. 【风险】旧接口下线 — 依赖旧接口的应用需要迁移。")
    );
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
    // References follow reading order: trend and headlines, figures, buzz, then the outlook.
    let references = &articles.x[articles.x.find("参考来源").unwrap()..];
    let order = [0, 3, 1, 2].map(|index| references.find(&format!("新闻{index}：")).unwrap());
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(cited_sources(&draft, sources, &[]).len(), 4);
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
fn structured_facts_render_both_article_formats() {
    let mut draft = analysis();
    draft.headline = "模型降价与工具开放，AI 应用的成本门槛下降".into();
    draft.hotspots = vec![story(0, StoryStatus::Confirmed)];
    let stored: StoredInsight = serde_json::from_value(json!({
        "analysis": draft, "sources": [source(0, "x", true)], "research_sources": [],
        "stats": {"current_items":32,"seven_day_items":213,"observed_days":5}
    }))
    .unwrap();
    let articles = render_articles(&stored, NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
    assert_eq!(articles.title, stored.analysis.headline);
    for text in [&articles.wechat, &articles.x, &articles.x_html] {
        assert!(text.contains(&stored.analysis.headline));
        assert!(text.contains("XAI 日报 · 2026-09-21"));
        assert!(text.contains("今日热点"));
        assert!(!text.contains("Asia/Shanghai"));
        assert!(!text.contains("最近 7 天"));
    }
    assert!(articles.wechat.contains("完整事实 及适用范围。"));
    assert!(articles.x.contains("紧凑事实及限制。"));
    assert_eq!(stored.stats.current_items, 32);
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
    let mut citations = research_citations(&reply);
    citations.extend(citations.clone()); // The correction may cite the same URL again.
    let references = selected_research_sources(&mut draft, citations);
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

async fn image_origin(state: &mut AppState) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, rustls};

    let host = "origin.example.test";
    let certified = rcgen::generate_simple_self_signed(vec![host.into()]).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    state.origin_http = reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([reqwest::Certificate::from_der(certified.cert.der()).unwrap()])
        .resolve(host, addr)
        .build()
        .unwrap();
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
    let bytes = images::fixture_image(image::ImageFormat::WebP, 640);
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = acceptor.accept(stream).await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(stream.read_u8().await.unwrap());
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.contains("accept: image/jpeg,image/png,image/webp,image/gif\r\n"));
            let body = if request.starts_with("GET /corrupt") {
                b"invalid image".as_slice()
            } else {
                bytes.as_slice()
            };
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: image/webp\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
            stream.write_all(body).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    (format!("https://{host}:{}", addr.port()), server)
}

#[tokio::test]
#[ignore = "requires NEWS_DAILY_TEST_DATABASE_URL and local test configuration"]
async fn postgres_daily_pipeline_retries_with_one_slot_and_counts_full_seven_days() {
    use axum::{Router, routing::post};
    use std::sync::{Mutex, atomic::AtomicBool};
    #[derive(Clone, Copy)]
    enum CitationReply {
        Valid,
        InvalidImages,
        InvalidThenCorrected,
        AlwaysInvalid,
    }
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
    config.tmpfile_base_url = "https://cdn.example.com".parse().unwrap();
    config.redis_addr = None;
    let day = due_day(Utc::now(), config.timezone, config.briefing_time);
    let end = crate::topics::to_utc(config.timezone, day.and_time(config.briefing_time));
    let window = NewsWindow {
        start: end - ChronoDuration::days(1),
        end,
    };
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let call_count = || calls.lock().unwrap().len();
    let fail_once = Arc::new(AtomicBool::new(true));
    let fail_analysis = Arc::new(AtomicBool::new(false));
    let citation_mode = Arc::new(Mutex::new(CitationReply::Valid));
    let handler_fail_analysis = fail_analysis.clone();
    let handler_citation_mode = citation_mode.clone();
    let handler_calls = calls.clone();
    let app = Router::new().route("/v1/responses", post(move |Json(body): Json<Value>| {
        let calls = handler_calls.clone();
        let fail_once = fail_once.clone();
        let fail_analysis = handler_fail_analysis.clone();
        let citation_mode = handler_citation_mode.clone();
        async move {
            calls.lock().unwrap().push(body.clone());
            let correcting = body["input"][1]["role"] == "assistant";
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
                let images = body["input"][0]["content"].as_array().unwrap().iter()
                    .filter(|part| part["type"] == "input_image").collect::<Vec<_>>();
                assert_eq!(images.len(), 1);
                let data = images[0]["image_url"].as_str().unwrap().strip_prefix("data:image/jpeg;base64,").unwrap();
                let bytes = STANDARD.decode(data).unwrap();
                assert_eq!(image::guess_format(&bytes).unwrap(), image::ImageFormat::Jpeg);
                assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 640);
                let mut draft = analysis();
                draft.hotspots = vec![story(current["index"].as_u64().unwrap() as usize, StoryStatus::Confirmed)];
                draft.hotspots[0].angle = "看点".into();
                draft.hotspots[0].angle_en = "Why it matters".into();
                draft.gossip = vec![story(gossip["index"].as_u64().unwrap() as usize, StoryStatus::Unconfirmed)];
                draft.changes = "七天内的变化。".into();
                draft.changes_source_indexes = vec![old["index"].as_u64().unwrap() as usize];
                draft.cover_source_index = Some(picture["index"].as_u64().unwrap() as usize);
                if correcting {
                    assert_eq!(body["tool_choice"], "none");
                    let feedback = body["input"][2]["content"].as_str().unwrap();
                    assert!(feedback.contains("changes_source_indexes"));
                    assert!(feedback.contains("9999"));
                    let previous: Value = serde_json::from_str(body["input"][1]["content"].as_str().unwrap()).unwrap();
                    assert_eq!(previous["changes_source_indexes"], json!([9999]));
                    assert_eq!(previous["research_source_urls"], json!(["https://example.com/research"]));
                }
                let invalid_citation = match *citation_mode.lock().unwrap() {
                    CitationReply::InvalidImages => {
                        draft.cover_source_index = Some(9999);
                        // An existing news source is still not an image candidate.
                        draft.picture_source_indexes = vec![current["index"].as_u64().unwrap() as usize];
                        // Nor does a figure stand that its source does not state.
                        draft.figures = vec![serde_json::from_value(json!({ "value": "99 款", "value_en": "99 apps",
                            "label": "未经资料证实", "label_en": "Unsupported", "source_indexes": [current["index"]] })).unwrap()];
                        false
                    }
                    CitationReply::InvalidThenCorrected => !correcting,
                    CitationReply::AlwaysInvalid => true,
                    CitationReply::Valid => false,
                };
                if invalid_citation {
                    draft.changes_source_indexes = vec![9999];
                }
                let mut payload = serde_json::to_value(draft).unwrap();
                payload["research_source_urls"] = json!(["https://example.com/research"]);
                payload
            };
            (StatusCode::OK, Json(json!({"status": "completed", "id": if correcting { "test-correction" } else { "test-response" },
                "usage": {"input_tokens":10,"input_tokens_details":{"cached_tokens":3},
                    "output_tokens":20,"output_tokens_details":{"reasoning_tokens":5},"cost_in_usd_ticks":100,
                    "server_side_tool_usage_details":{"web_search_calls":if correcting {0} else {1},"x_search_calls":0}},
                "output": [{"type": "message", "content": [{"type": "output_text", "text": payload.to_string(),
                    "annotations":if correcting { json!([]) } else {
                        json!([{"type":"url_citation","url":"https://example.com/research","title":"Research"}])
                    }}]}]})))
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
    let (image_origin, image_server) = image_origin(&mut state).await;
    let original = image(
        "https://cdn.example.com/robot.avif",
        &format!("{image_origin}/robot.jpg"),
    );
    let invalid = image(
        "https://cdn.example.com/corrupt.avif",
        &format!("{image_origin}/corrupt.jpg"),
    );
    let prepared = images::prepare(&state, vec![(7, original.clone()), (9, invalid)]).await;
    assert_eq!(prepared.len(), 1);
    assert!(prepared.contains_key(&7));
    assert_eq!(original.url, "https://cdn.example.com/robot.avif");
    // The first request is usable without waiting for background cache warming.
    let http_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_request_url = format!("http://{}", http_listener.local_addr().unwrap());
    let router = crate::web::router(state.clone());
    let http_server =
        tokio::spawn(async move { axum::serve(http_listener, router).await.unwrap() });
    let first: Value = state
        .http
        .get(format!("{first_request_url}/topics"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["topics"].as_array().unwrap().len(), 20);
    state
        .http
        .get(format!("{first_request_url}/digest?topic=x"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    http_server.abort();
    let catalog: Vec<(String, String)> = sqlx::query_as(
        "SELECT id,name FROM topics WHERE owner_user_id IS NULL ORDER BY sort_weight, created_at",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        catalog
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        [
            "x",
            "p",
            "w",
            "travel",
            "movie",
            "music",
            "game",
            "china-ai",
            "book",
            "overseas-ai",
            "gossip",
            "github",
            "events",
            "cases",
            "biz",
            "issues",
            "eink",
            "ai-hardware",
            "digital",
            "llm",
            "fashion",
            "reads",
            "equity"
        ]
    );
    assert_eq!(
        catalog
            .iter()
            .find(|(id, _)| id == "overseas-ai")
            .unwrap()
            .1,
        "海外 AI"
    );
    sqlx::migrate::Migrator::new(state.config.migrations_dir.clone())
        .await
        .unwrap()
        .run(&state.pool)
        .await
        .unwrap();
    let migrations: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&state.pool)
            .await
            .unwrap();
    assert_eq!(migrations, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    sqlx::query("INSERT INTO topics (id,name,brief,enabled) VALUES ('inactive','Inactive','Inactive',false)")
        .execute(&state.pool).await.unwrap();
    sqlx::query("TRUNCATE items, runs, ai_insights CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET enabled = id IN ('x','p','china-ai','overseas-ai','gossip','github','events','cases','issues'), brief=id, last_run_at=NULL, last_status=NULL, daily_run_cap=24")
        .execute(&state.pool).await.unwrap();
    state.digest.reload_topics(&state).await.unwrap();
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
        (
            "overseas-ai",
            "zero-baseline",
            end - ChronoDuration::hours(2),
        ),
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
        ("china-ai", "china-release", end - ChronoDuration::hours(4)),
        ("cases", "case-study", end - ChronoDuration::hours(4)),
        ("inactive", "disabled", end - ChronoDuration::hours(5)),
        ("events", "world-event", end - ChronoDuration::hours(1)),
        ("events", "past-world-event", end - ChronoDuration::days(2)),
        ("issues", "feedback", end - ChronoDuration::hours(1)),
        ("issues", "past-feedback", end - ChronoDuration::days(2)),
        ("issues", "today", end - ChronoDuration::hours(1)),
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
    // An undated duplicate cannot displace the dated Gossip source.
    insert_story(
        &state.pool,
        "x",
        "rumour",
        None,
        end - ChronoDuration::hours(2),
    )
    .await;
    sqlx::query("UPDATE items SET image = $1 WHERE topic_id = 'p'")
        .bind(json!({"url": original.url, "origin_url": original.origin_url, "source_url": "https://example.com/picture",
            "caption": "机器人演示（测试图）", "caption_en": "Robot demo (fixture)", "credit": "演示团队",
            "expires_at": Utc::now() + ChronoDuration::days(1)}))
        .execute(&state.pool).await.unwrap();
    // Even a higher-ranked copy outside the scope must not displace an eligible source.
    sqlx::query("UPDATE items SET importance=5 WHERE topic_id='issues'")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE items SET tags=tags || ARRAY['Agent','agent','AI Agent','智能体','AI代理'] WHERE title IN ('today','earlier')")
        .execute(&state.pool).await.unwrap();
    let stats = collect_stats(&state, window).await.unwrap();
    assert_eq!(
        stats["topic_ids"],
        json!(["x", "p", "china-ai", "overseas-ai", "gossip", "cases"])
    );
    assert_eq!(stats["seven_day_items"], 9);
    assert_eq!(stats["current_items"], 7);
    assert_eq!(stats["observed_days"], 3);
    assert_eq!(stats["daily"].as_array().unwrap().len(), 7);
    assert_eq!(stats["tags"][0]["current_items"], 7);
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
        .find(|topic| topic["topic_id"] == "overseas-ai")
        .unwrap();
    assert!(zero["change_percent"].is_null());
    let before = call_count();
    let attempt = tokio::time::timeout(
        Duration::from_secs(10),
        runner::collect_briefing(&state, window, None),
    )
    .await
    .unwrap();
    assert!(attempt.is_err());
    tokio::time::timeout(
        Duration::from_secs(10),
        runner::collect_briefing(&state, window, None),
    )
    .await
    .unwrap()
    .unwrap();
    let completed = execute(&state, window.start, window.end).await.unwrap();
    let stored: StoredInsight = serde_json::from_value(completed.payload.clone()).unwrap();
    assert_eq!(stored.stats.current_items, 8);
    assert_eq!(stored.stats.seven_day_items, 10);
    let articles = render_articles(&stored, day);
    assert_eq!(articles.title, stored.analysis.headline);
    for text in [&articles.wechat, &articles.x, &articles.x_html] {
        assert!(text.contains(&stored.analysis.headline));
        assert!(!text.contains("采样说明"));
        assert!(!text.contains("新闻："));
        assert!(!text.contains("最近 7 天"));
    }
    assert_eq!(stored.analysis.hotspots.len(), 1);
    assert_eq!(articles.images.len(), 1);
    assert!(articles.images[0].cover);
    assert_eq!(articles.images[0].url, "https://cdn.example.com/robot.avif");
    assert!(articles.wechat.contains("图片新闻"));
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
            .any(|source| source.topic_id == "issues")
    );
    assert!(
        stored
            .sources
            .iter()
            .any(|source| source.topic_id == "events")
    );
    assert_eq!(call_count() - before, 11); // Nine columns, one failed-column retry, one analysis.
    // Drive the actual coordinator, including slot acquisition and atomic publication.
    let before = call_count();
    let id = start_generation(&state, day, end, Kind::Ai).await.unwrap();
    assert!(matches!(
        start_generation(&state, day, end, Kind::Ai).await,
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
    assert_eq!(call_count() - before, 1); // Manual regeneration reuses the same collection.
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
        json!({"current_items":8,"seven_day_items":10,"observed_days":3})
    );
    assert!(public["insight"].get("wechat_article").is_none());
    let before = call_count();
    tick(&state).await.unwrap();
    assert!(!is_running(&state));
    assert_eq!(call_count() - before, 0);

    // A failed manual regeneration keeps the previously published edition intact.
    let before = call_count();
    fail_analysis.store(true, Ordering::Relaxed);
    let failed_id = start_generation(&state, day, end, Kind::Ai).await.unwrap();
    wait_for_generation(&state).await;
    let failed: String = sqlx::query_scalar("SELECT status FROM ai_insights WHERE id=$1")
        .bind(failed_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(failed, "failed");
    assert_eq!(render_latest(&state).await.unwrap().body, rendered.body);
    assert_eq!(call_count() - before, 1);

    // Scheduler retries only after the cooldown and uses the original cutoff.
    let before = call_count();
    sqlx::query("DELETE FROM ai_insights WHERE status='ok'")
        .execute(&state.pool)
        .await
        .unwrap();
    tick(&state).await.unwrap();
    assert!(!is_running(&state));
    assert_eq!(call_count() - before, 0);
    sqlx::query("UPDATE ai_insights SET finished_at=NOW()-INTERVAL '11 minutes' WHERE id=$1")
        .bind(failed_id)
        .execute(&state.pool)
        .await
        .unwrap();
    // Three failures of one edition end the automatic retries.
    let earlier = [Uuid::new_v4(), Uuid::new_v4()];
    for id in earlier {
        sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,error) SELECT $1,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,'earlier failure' FROM ai_insights WHERE id=$2")
            .bind(id).bind(failed_id).execute(&state.pool).await.unwrap();
    }
    tick(&state).await.unwrap();
    assert!(!is_running(&state));
    assert_eq!(call_count() - before, 0);
    sqlx::query("DELETE FROM ai_insights WHERE id=ANY($1)")
        .bind(&earlier[..])
        .execute(&state.pool)
        .await
        .unwrap();
    fail_analysis.store(false, Ordering::Relaxed);
    tick(&state).await.unwrap();
    wait_for_generation(&state).await;
    assert_eq!(call_count() - before, 1);
    let recovered_end: DateTime<Utc> =
        sqlx::query_scalar("SELECT window_end FROM ai_insights WHERE status='ok'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(recovered_end, end);

    // Bad optional images and figures must publish valid prose without an extra AI request.
    let before = call_count();
    *citation_mode.lock().unwrap() = CitationReply::InvalidImages;
    let without_images = execute(&state, window.start, window.end).await.unwrap();
    let stored: StoredInsight = serde_json::from_value(without_images.payload).unwrap();
    assert_eq!(stored.analysis.hotspots.len(), 1);
    assert!(stored.analysis.cover_source_index.is_none());
    assert!(stored.analysis.picture_source_indexes.is_empty());
    assert!(stored.analysis.figures.is_empty());
    assert!(selected_article_images(&stored.analysis, &stored.sources).is_empty());
    assert_eq!(call_count() - before, 1);

    // Citation errors receive exactly one targeted correction with the original
    // context, while both requests' usage and verified research citations survive.
    let before = call_count();
    *citation_mode.lock().unwrap() = CitationReply::InvalidThenCorrected;
    let corrected = execute(&state, window.start, window.end).await.unwrap();
    assert_eq!(corrected.response_id.as_deref(), Some("test-correction"));
    assert_eq!(corrected.usage.input_tokens, 20);
    assert_eq!(corrected.usage.input_tokens_details.cached_tokens, 6);
    assert_eq!(corrected.usage.output_tokens, 40);
    assert_eq!(corrected.usage.output_tokens_details.reasoning_tokens, 10);
    assert_eq!(corrected.usage.cost_in_usd_ticks, 200);
    assert_eq!(
        corrected
            .usage
            .server_side_tool_usage_details
            .web_search_calls,
        1
    );
    let stored: StoredInsight = serde_json::from_value(corrected.payload).unwrap();
    assert_eq!(stored.research_sources.len(), 1);
    assert_eq!(
        stored.research_sources[0].url,
        "https://example.com/research"
    );
    assert_eq!(call_count() - before, 2);

    // A second invalid draft fails once, records the exact citation, and leaves
    // the previously published daily edition available.
    let before = call_count();
    let published = render_latest(&state).await.unwrap();
    *citation_mode.lock().unwrap() = CitationReply::AlwaysInvalid;
    let invalid_id = start_generation(&state, day, end, Kind::Ai).await.unwrap();
    wait_for_generation(&state).await;
    let (status, error): (String, String) =
        sqlx::query_as("SELECT status,error FROM ai_insights WHERE id=$1")
            .bind(invalid_id)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert!(error.contains("changes_source_indexes"));
    assert!(error.contains("9999"));
    assert_eq!(render_latest(&state).await.unwrap().body, published.body);
    assert_eq!(call_count() - before, 2);
    *citation_mode.lock().unwrap() = CitationReply::Valid;

    // Fail the first completion write after the model has returned. The sequence
    // increment survives rollback, so the next write can persist the same result.
    sqlx::raw_sql(
        "CREATE SEQUENCE test_insight_finish_attempt; \
        CREATE FUNCTION test_insight_finish_fault() RETURNS trigger LANGUAGE plpgsql AS $$ \
        BEGIN IF NEW.status='ok' AND nextval('test_insight_finish_attempt')=1 THEN \
        RAISE EXCEPTION 'injected completion write failure'; END IF; RETURN NEW; END $$; \
        CREATE TRIGGER test_insight_finish_fault BEFORE UPDATE ON ai_insights \
        FOR EACH ROW EXECUTE FUNCTION test_insight_finish_fault();",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let before = call_count();
    let retry_id = start_generation(&state, day, end, Kind::Ai).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let failed_once: bool =
                sqlx::query_scalar("SELECT is_called FROM test_insight_finish_attempt")
                    .fetch_one(&state.pool)
                    .await
                    .unwrap();
            if failed_once {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(is_running(&state));
    tick(&state).await.unwrap();
    wait_for_generation(&state).await;
    let recovered: String = sqlx::query_scalar("SELECT status FROM ai_insights WHERE id=$1")
        .bind(retry_id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(recovered, "ok");
    assert_eq!(call_count() - before, 1);
    sqlx::raw_sql(
        "DROP TRIGGER test_insight_finish_fault ON ai_insights; \
        DROP FUNCTION test_insight_finish_fault(); DROP SEQUENCE test_insight_finish_attempt;",
    )
    .execute(&state.pool)
    .await
    .unwrap();

    // Daily collection must continue to respect the editor's configured cost cap.
    let before = call_count();
    sqlx::query("UPDATE topics SET daily_run_cap=1 WHERE id='github'")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE runs SET triggered_by='manual', window_end=NULL WHERE topic_id='github'")
        .execute(&state.pool)
        .await
        .unwrap();
    assert!(
        runner::collect_briefing(&state, window, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("上限")
    );
    assert_eq!(call_count() - before, 0);

    // A small input budget reproduces the same pressure as many full public columns.
    let sampled = sqlx::query_as::<_, SourceRow>(include_str!("sources.sql"))
        .bind(end - ChronoDuration::days(7))
        .bind(window.start)
        .bind(end)
        .bind(4_i64)
        .bind(super::outside_briefing())
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
    // A repeated URL outside the 80-item prefilter must not count as an insert.
    sqlx::query(
        "INSERT INTO items (id,topic_id,day,fingerprint,title,summary,url,first_seen_at) \
        SELECT gen_random_uuid(),'x',$1,decode(lpad(i::text,64,'0'),'hex'), \
        'fixture-'||md5(i::text),'prefilter fixture','https://example.com/prefilter/'||i, \
        NOW()+INTERVAL '1 hour'+i*INTERVAL '1 second' FROM generate_series(1,81) i",
    )
    .bind(state.digest.today())
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE topics SET daily_run_cap=24 WHERE id='github'")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE runs SET triggered_by='briefing',window_end=$1 WHERE topic_id='github' AND status='ok'")
        .bind(end).execute(&state.pool).await.unwrap();
    sqlx::query("DELETE FROM runs WHERE topic_id='x'")
        .execute(&state.pool)
        .await
        .unwrap();
    let count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM items")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let revision_before: i64 = sqlx::query_scalar("SELECT feed_revision FROM topics WHERE id='x'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    runner::collect_briefing(&state, window, None)
        .await
        .unwrap();
    let counts: (i32, i32, i32) =
        sqlx::query_as("SELECT new_count,dup_count,dropped_count FROM runs WHERE topic_id='x'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(counts, (0, 1, 2));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM items")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        count_before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT feed_revision FROM topics WHERE id='x'")
            .fetch_one(&state.pool)
            .await
            .unwrap(),
        revision_before
    );
    // Fashion stays outside the briefing even once readers see it; business is collected.
    sqlx::query(
        "UPDATE topics SET enabled = (id IN ('fashion', 'biz')),
         visible = visible OR id IN ('fashion', 'biz')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    runner::collect_briefing(&state, window, None)
        .await
        .unwrap();
    let collected: Vec<String> =
        sqlx::query_scalar("SELECT topic_id FROM runs WHERE topic_id IN ('fashion', 'biz')")
            .fetch_all(&state.pool)
            .await
            .unwrap();
    assert_eq!(collected, ["biz"]);
    // And read, while fashion stories are not.
    let at = window.end - ChronoDuration::hours(1);
    insert_story(&state.pool, "biz", "biz-story", Some(at), at).await;
    insert_story(&state.pool, "fashion", "fashion-story", Some(at), at).await;
    let read = sqlx::query_as::<_, SourceRow>(include_str!("sources.sql"))
        .bind(window.end - ChronoDuration::days(7))
        .bind(window.start)
        .bind(window.end)
        .bind(1000_i64)
        .bind(super::outside_briefing())
        .fetch_all(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        read.iter()
            .map(|row| row.title.as_str())
            .collect::<Vec<_>>(),
        ["biz-story"]
    );

    state.shutdown_workers().await;
    image_server.abort();
    check_media_renewal(&mut state).await;
    check_media_limits(state).await;
    server.abort();
}

async fn check_media_renewal(state: &mut AppState) {
    Arc::get_mut(&mut state.config).unwrap().tmpfile_base_url =
        "https://mirror.example.com".parse().unwrap();
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
    assert!(stored.get("articles").is_none());
    let stored: StoredInsight = serde_json::from_value(stored).unwrap();
    let articles = render_articles(&stored, day);
    assert!(articles.wechat.contains(&fresh.url));
    assert!(!articles.wechat.contains(&old.url));
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
    state.origin_http = reqwest::Client::builder()
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
    assert_eq!(state.image_slots.available_permits(), media::IMAGE_SLOTS);

    // Two batches together ask for two transfers more than the slots allow.
    Arc::get_mut(&mut state.config).unwrap().image_timeout = Duration::from_secs(5);
    let per_batch = media::IMAGE_SLOTS / 2 + 1;
    let mut tasks = tokio::task::JoinSet::new();
    for batch in 0..2 {
        let state = state.clone();
        let mut images = (0..per_batch)
            .map(|index| slow(1 + batch * per_batch + index))
            .collect::<Vec<_>>();
        tasks.spawn(async move {
            media::mirror_images(&state, &mut images).await;
            images
        });
    }
    let mut sockets = Vec::new();
    for _ in 0..media::IMAGE_SLOTS {
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
        tokio::time::timeout(
            Duration::from_secs(1),
            state.image_slots.acquire_many(media::IMAGE_SLOTS as u32),
        )
        .await
        .unwrap()
        .unwrap(),
    );
    assert_eq!(state.image_slots.available_permits(), media::IMAGE_SLOTS);
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
