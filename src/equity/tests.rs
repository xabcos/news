use super::*;

fn draft() -> Analysis {
    serde_json::from_value(json!({
        "headline":"跟踪订单交付进度","headline_en":"Watching order delivery",
        "summary":"订单进入交付阶段，观察回款。","summary_en":"Orders are entering delivery; watch cash collection.","reviews":[],
        "stocks":[{"name":"英伟达","name_en":"NVIDIA","symbol":"NVDA","market":"us","signal":"catalyst",
            "reason":"订单开始交付，可能带来收入增长。","reason_en":"Delivery may support revenue growth.",
            "watch":"下一次财报是否确认收入。","watch_en":"Look for revenue confirmation in the next earnings report.",
            "risk":"交付延迟可能推翻判断。","risk_en":"Delivery delays may invalidate the assessment.",
            "events":[{"fact":"公司公告订单开始交付。","fact_en":"The company announced the start of delivery.",
                "quote":"订单开始交付","source_indexes":[0]}]}]
    })).unwrap()
}

fn source() -> Source {
    Source {
        title: "公司公告".into(),
        summary: "英伟达（NVDA）公告订单开始交付。SECRET INTERNAL".into(),
        source: "公司公告".into(),
        url: "https://example.com/disclosure".into(),
        published_at: Utc::now(),
    }
}

fn window() -> NewsWindow {
    NewsWindow {
        start: Utc::now() - Duration::days(7),
        end: Utc::now() + Duration::hours(1),
    }
}

fn baseline() -> history::Baseline {
    let mut stock = public_stock(&draft().stocks[0], &[source()], chrono_tz::Asia::Shanghai);
    stock["reason"] = json!("上期原始判断：观察订单能否进入交付。");
    history::Baseline {
        id: uuid::Uuid::new_v4(),
        day: Utc::now().date_naive().pred_opt().unwrap(),
        cutoff: Utc::now() - Duration::days(1),
        stocks: vec![stock],
        source_urls: vec!["https://example.com/previous".into()],
    }
}

fn review() -> history::Review {
    history::Review {
        previous_index: 0,
        status: "partial".into(),
        assessment: "交付进展支持原判断，收入确认仍待财报验证。".into(),
        assessment_en:
            "Delivery supports the thesis; revenue recognition still awaits earnings confirmation."
                .into(),
        events: draft().stocks.remove(0).events,
    }
}

#[test]
fn rejects_invented_symbols_quotes_and_duplicate_evidence() {
    let sources = [source()];
    let mut analysis = draft();
    assert!(validate(&analysis, &sources, window()).is_ok());
    analysis.stocks[0].symbol = "AAPL".into();
    assert!(validate(&analysis, &sources, window()).is_err());
    analysis = draft();
    analysis.stocks[0].events[0].quote = "股价上涨".into();
    assert!(validate(&analysis, &sources, window()).is_err());
    analysis = draft();
    analysis.stocks[0].events[0].source_indexes = vec![0, 0];
    assert!(validate(&analysis, &sources, window()).is_err());
    analysis.stocks[0].events[0].source_indexes = vec![99];
    assert!(validate(&analysis, &sources, window()).is_err());
    assert!(symbol_in("300308", "中际旭创（300308.SZ）"));
    assert!(symbol_in("BRK.B", "NYSE: BRK.B"));
    assert!(!symbol_in("BA", "DATABASE"));
}

#[test]
fn published_cards_keep_verified_dates_without_raw_material() {
    let sources = [source()];
    let analysis = draft();
    let stock = public_stock(&analysis.stocks[0], &sources, chrono_tz::Asia::Shanghai);
    assert_eq!(
        stock["events"][0]["date"],
        json!(
            sources[0]
                .published_at
                .with_timezone(&chrono_tz::Asia::Shanghai)
                .date_naive()
        )
    );
    assert!(!stock.to_string().contains("SECRET INTERNAL"));
    assert!(stock.get("quote").is_none());
}

#[test]
fn feedback_requires_new_evidence_and_complete_prior_coverage() {
    let mut baseline = baseline();
    let mut sources = [source()];
    let mut reviews = vec![review()];
    let end = window().end;
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_ok());
    sources[0].published_at = baseline.cutoff - Duration::seconds(1);
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    sources[0].published_at = baseline.cutoff;
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_ok());
    sources[0].published_at = end;
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    sources[0].published_at = end - Duration::seconds(1);
    baseline.source_urls.push(sources[0].url.clone());
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    baseline.source_urls.pop();
    reviews[0].events[0].quote = "未经披露的收入增长".into();
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    reviews[0] = review();
    reviews[0].events[0].source_indexes = vec![99];
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    reviews[0] = review();
    reviews[0].events.clear();
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    reviews[0].status = "pending".into();
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_ok());
    assert!(history::validate(&[], Some(&baseline), &sources, end).is_err());
    baseline.stocks.push(baseline.stocks[0].clone());
    reviews = vec![review(), review()];
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    reviews[1].previous_index = 1;
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_ok());
    reviews[1].previous_index = 2;
    assert!(history::validate(&reviews, Some(&baseline), &sources, end).is_err());
    assert!(history::validate(&reviews, None, &sources, end).is_err());
}

#[test]
fn feedback_keeps_original_thesis_and_reports_omitted_stocks() {
    let mut baseline = baseline();
    baseline.stocks[0]["signal"] = json!("watch");
    let mut omitted = baseline.stocks[0].clone();
    omitted["market"] = json!("hk");
    omitted["symbol"] = json!("00700");
    baseline.stocks.push(omitted);
    let mut reviews = history::no_progress(Some(&baseline));
    reviews[0] = review();
    let sources = [source()];
    let mut stocks = vec![public_stock(
        &draft().stocks[0],
        &sources,
        chrono_tz::Asia::Shanghai,
    )];
    let followups = history::attach(
        &mut stocks,
        &reviews,
        Some(&baseline),
        &sources,
        chrono_tz::Asia::Shanghai,
    );
    assert_eq!(stocks[0]["change"], "changed");
    assert_eq!(stocks[0]["previous_signal"], "watch");
    assert_eq!(
        stocks[0]["feedback"]["previous"]["reason"],
        baseline.stocks[0]["reason"]
    );
    assert_ne!(stocks[0]["reason"], baseline.stocks[0]["reason"]);
    assert_eq!(stocks[0]["feedback"]["previous_id"], json!(baseline.id));
    assert_eq!(stocks[0]["feedback"]["status"], "partial");
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0]["symbol"], "00700");
    assert_eq!(followups[0]["feedback"]["status"], "insufficient");
    assert!(
        followups[0]["feedback"]["assessment"]
            .as_str()
            .unwrap()
            .contains("暂无可验证进展")
    );
    let public = json!({"stocks":stocks,"followups":followups}).to_string();
    assert!(!public.contains("SECRET INTERNAL"));
    assert!(!public.contains("source_indexes"));
    assert!(!public.contains("quote"));
}

#[test]
fn intervening_evidence_can_review_but_cannot_enter_current_watchlist() {
    let mut baseline = baseline();
    let window = window();
    baseline.cutoff = window.start - Duration::days(3);
    let mut sources = [source()];
    sources[0].published_at = window.start - Duration::days(1);
    assert!(history::validate(&[review()], Some(&baseline), &sources, window.end).is_ok());
    assert!(validate(&draft(), &sources, window).is_err());
}

#[test]
fn hong_kong_padding_does_not_create_a_new_thesis_or_duplicate_stock() {
    let mut baseline = baseline();
    baseline.stocks[0]["market"] = json!("hk");
    baseline.stocks[0]["symbol"] = json!("00700");
    let mut analysis = draft();
    analysis.stocks[0].market = "hk".into();
    analysis.stocks[0].symbol = "0700".into();
    let mut sources = [source()];
    sources[0].summary = "示例港股（0700.HK）公告订单开始交付。".into();
    assert!(validate(&analysis, &sources, window()).is_ok());
    let reviews = vec![review()];
    assert!(history::validate(&reviews, Some(&baseline), &sources, window().end).is_ok());
    let mut stocks = vec![public_stock(
        &analysis.stocks[0],
        &sources,
        chrono_tz::Asia::Shanghai,
    )];
    assert!(
        history::attach(
            &mut stocks,
            &reviews,
            Some(&baseline),
            &sources,
            chrono_tz::Asia::Shanghai
        )
        .is_empty()
    );
    assert_eq!(stocks[0]["change"], "continuing");
    assert_eq!(stocks[0]["feedback"]["previous"]["symbol"], "00700");
    let mut duplicate = draft().stocks.remove(0);
    duplicate.market = "hk".into();
    duplicate.symbol = "00700".into();
    analysis.stocks.push(duplicate);
    assert!(validate(&analysis, &sources, window()).is_err());
    assert!(!same_security("cn", "00700", &baseline.stocks[0]));
    assert!(!symbol_in("0700", "7000700"));
}

#[tokio::test]
#[ignore = "requires NEWS_EQUITY_TEST_DATABASE_URL and local test configuration"]
async fn postgres_equity_visibility_and_note_lifecycle() {
    use axum::{Router, routing::post};
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;
    let database_url =
        std::env::var("NEWS_EQUITY_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(name.starts_with("news_equity_test"));
    pool.close().await;
    let mut config = crate::config::Config::from_env().unwrap();
    config.database_url = database_url;
    config.redis_addr = None;
    config.scheduler_enabled = false;
    config.max_concurrent_runs = 1;
    config.timezone = chrono_tz::Asia::Shanghai;
    config.briefing_time = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = fetches.clone();
    let mock = Router::new().route("/v1/responses", post(move |Json(body):Json<Value>| {
        let counter = counter.clone();
        async move {
            let payload = if body["text"]["format"]["name"]=="news_digest" {
                counter.fetch_add(1, Ordering::Relaxed);
                json!({"items":[],"cover":null})
            } else {
                assert_eq!(body["text"]["format"]["name"], "equity_insight");
                let input: Value = serde_json::from_str(body["input"].as_str().unwrap()).unwrap();
                assert_eq!(input["sources"].as_array().unwrap().len(), 2);
                assert!(input["sources"][0]["material"]["summary"].as_str().unwrap().contains("SECRET INTERNAL"));
                assert_eq!(input["sources"][0]["in_current_window"], true);
                assert_eq!(input["sources"][1]["in_current_window"], false);
                assert_eq!(input["sources"][1]["new_since_previous"], true);
                assert_eq!(input["previous"]["stocks"].as_array().unwrap().len(), 2);
                assert_eq!(input["previous"]["stocks"][0]["thesis"]["reason"], "上期原始判断：观察订单能否进入交付。");
                assert!(input["previous"]["stocks"][0]["thesis"].get("feedback").is_none());
                let mut analysis = draft();
                let mut pending = review();
                pending.previous_index = 1;
                pending.status = "insufficient".into();
                pending.events.clear();
                analysis.reviews = vec![review(), pending];
                serde_json::to_value(analysis).unwrap()
            };
            Json(json!({"status":"completed","id":"equity-test","output":[{"type":"message","content":[{"type":"output_text","text":payload.to_string()}]}]}))
        }
    }));
    let ai_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    config.ai_base_url = format!("http://{}", ai_listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let ai_server = tokio::spawn(async move { axum::serve(ai_listener, mock).await.unwrap() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    config.public_base_url = base.parse().unwrap();
    let state = AppState::new(config).await.unwrap();
    sqlx::query("TRUNCATE items,runs,ai_insights,sessions CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users")
        .execute(&state.pool)
        .await
        .unwrap();
    let initial: (bool, bool) =
        sqlx::query_as("SELECT enabled,visible FROM topics WHERE id='equity'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(initial, (true, false));
    let end = Kind::Equity.cutoff(
        Utc::now(),
        state.config.timezone,
        state.config.briefing_time,
    );
    let day = end.with_timezone(&state.config.timezone).date_naive();
    for (published, hidden, tag) in [
        (end - Duration::hours(1), false, "valid"),
        (end, false, "future"),
        (end - Duration::days(8), false, "old"),
        (end - Duration::hours(2), true, "hidden"),
    ] {
        sqlx::query("INSERT INTO items (id,topic_id,day,fingerprint,title,summary,url,source,published_at,hidden) VALUES ($1,'equity',$2,$3,'公司公告',$4,$5,'公司公告',$6,$7)")
            .bind(Uuid::new_v4()).bind(state.digest.today()).bind(Sha256::digest(tag.as_bytes()).to_vec())
            .bind(source().summary).bind(format!("https://example.com/{tag}")).bind(published).bind(hidden)
            .execute(&state.pool).await.unwrap();
    }
    let mut cookies = Vec::new();
    for role in ["admin", "user"] {
        let id: i64 =
            sqlx::query_scalar("INSERT INTO users (email,role) VALUES ($1,$2) RETURNING id")
                .bind(format!("{role}@equity.example.test"))
                .bind(role)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        let token = format!("test-{role}-{}", Uuid::new_v4());
        sqlx::query("INSERT INTO sessions (token_hash,user_id,expires_at) VALUES ($1,$2,NOW()+INTERVAL '1 day')")
            .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(id).execute(&state.pool).await.unwrap();
        cookies.push(format!("{}={token}", state.config.session_cookie_name));
    }
    let app = crate::web::router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    for (enabled, visible) in [
        (true, true),
        (true, false),
        (false, true),
        (false, false),
        (true, true),
    ] {
        let response = http
            .patch(format!("{base}/api/admin/topics/equity"))
            .header("cookie", &cookies[0])
            .header("origin", &base)
            .json(&json!({"enabled":enabled,"visible":visible}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let topic: Value = response.json().await.unwrap();
        assert_eq!(topic["visible"], visible);
        for cookie in ["", cookies[0].as_str(), cookies[1].as_str()] {
            let topics: Value = http
                .get(format!("{base}/topics"))
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(
                topics["topics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["id"] == "equity"),
                enabled && visible
            );
            for path in ["/digest?topic=equity", "/feed.xml?topic=equity"] {
                let result = http
                    .get(format!("{base}{path}"))
                    .header("cookie", cookie)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(result.status().is_success(), enabled && visible);
            }
            let all: Value = http
                .get(format!("{base}/digest?limit=128"))
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(
                all["topics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["id"] == "equity"),
                enabled && visible
            );
        }
        let private = http
            .get(format!("{base}/api/me/feed?topic=equity"))
            .header("cookie", &cookies[1])
            .send()
            .await
            .unwrap();
        assert_eq!(private.status().is_success(), enabled && visible);
        let days: Value = http
            .get(format!("{base}/days?topic=equity"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(
            !days["days"].as_array().unwrap().is_empty(),
            enabled && visible
        );
    }
    http.patch(format!("{base}/api/admin/topics/equity"))
        .header("cookie", &cookies[0])
        .header("origin", &base)
        .json(&json!({"visible":false}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let old_day = day - Duration::days(10);
    let mut prior = baseline();
    prior.day = old_day;
    prior.cutoff = end - Duration::days(10);
    let mut omitted = prior.stocks[0].clone();
    omitted["market"] = json!("hk");
    omitted["symbol"] = json!("00700");
    prior.stocks.push(omitted);
    prior.stocks[0]["feedback"] = json!({"previous":{"feedback":{"previous":"older snapshot"}}});
    let old_payload = json!({"note":{"briefing_day":old_day,"stocks":prior.stocks},"sources":[{"url":"https://example.com/previous"}]});
    sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,payload) VALUES ($1,'equity','ok',$2,$3,$4,NOW(),NOW(),'test',$5)")
        .bind(prior.id).bind(old_day).bind(prior.cutoff-Duration::days(7)).bind(prior.cutoff).bind(&old_payload)
        .execute(&state.pool).await.unwrap();
    for attempt in 0..2 {
        let run = insight::trigger_kind(&state, Kind::Equity).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while insight::is_running(&state) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let status: (String, Option<String>) =
            sqlx::query_as("SELECT status,error FROM ai_insights WHERE id=$1")
                .bind(run)
                .fetch_one(&state.pool)
                .await
                .unwrap();
        assert_eq!(status.0, "ok", "{:?}", status.1);
        let response = http
            .get(format!("{base}/equity-insight"))
            .send()
            .await
            .unwrap();
        let etag = response.headers()["etag"].clone();
        let public: Value = response.json().await.unwrap();
        assert_eq!(public["note"]["stocks"][0]["symbol"], "NVDA");
        let feedback = &public["note"]["stocks"][0]["feedback"];
        assert_eq!(feedback["previous_id"], json!(prior.id));
        assert_eq!(feedback["previous_day"], json!(old_day));
        assert_eq!(
            feedback["previous"]["reason"],
            old_payload["note"]["stocks"][0]["reason"]
        );
        assert_eq!(feedback["status"], "partial");
        assert!(feedback["previous"].get("feedback").is_none());
        assert_eq!(public["note"]["followups"][0]["symbol"], "00700");
        assert_eq!(
            public["note"]["followups"][0]["feedback"]["status"],
            "insufficient"
        );
        assert!(!public.to_string().contains("SECRET INTERNAL"));
        let unchanged = http
            .get(format!("{base}/equity-insight"))
            .header("if-none-match", etag)
            .send()
            .await
            .unwrap();
        assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
        if attempt == 0 {
            // A newer version of the earlier date must not rewrite today's baseline.
            sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,payload) SELECT $1,kind,status,briefing_day,window_start,window_end,NOW(),NOW(),model,jsonb_set(payload,'{note,stocks,0,reason}',to_jsonb('事后修改的观点'::text)) FROM ai_insights WHERE id=$2")
                .bind(Uuid::new_v4()).bind(prior.id).execute(&state.pool).await.unwrap();
        }
    }
    assert_eq!(
        fetches.load(Ordering::Relaxed),
        1,
        "regeneration reuses completed collection"
    );
    let versions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ai_insights WHERE kind='equity' AND status='ok' AND briefing_day=$1",
    )
    .bind(day)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        versions, 2,
        "same-day equity versions must survive regeneration"
    );
    let historical: Value = http
        .get(format!("{base}/equity-insight?day={old_day}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(historical["note"]["briefing_day"], json!(old_day));
    assert_eq!(
        listed(&historical),
        [json!(end), json!(prior.cutoff)],
        "the latest days are listed, and so is the older edition being read"
    );
    let original: Value = sqlx::query_scalar("SELECT payload FROM ai_insights WHERE id=$1")
        .bind(prior.id)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(
        original, old_payload,
        "published prior thesis must remain unchanged"
    );
    let next = history::baseline(&state, end + Duration::days(1))
        .await
        .unwrap()
        .unwrap();
    assert!(
        next.stocks[0].get("feedback").is_none(),
        "snapshots must not grow recursively"
    );
    sqlx::query("UPDATE items SET hidden=true WHERE topic_id='equity'")
        .execute(&state.pool)
        .await
        .unwrap();
    let quiet = execute(
        &state,
        NewsWindow {
            start: end,
            end: end + Duration::days(1),
        },
    )
    .await
    .unwrap();
    assert!(
        quiet.response_id.is_none(),
        "no model call when no material exists"
    );
    assert!(
        quiet.payload["note"]["stocks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        quiet.payload["note"]["followups"][0]["feedback"]["status"],
        "insufficient"
    );
    let first_day = old_day.pred_opt().unwrap();
    let first_end = prior.cutoff - Duration::days(1);
    let first = execute(
        &state,
        NewsWindow {
            start: first_end - Duration::days(1),
            end: first_end,
        },
    )
    .await
    .unwrap();
    assert!(first.payload["baseline"].is_null());
    assert!(
        first.payload["note"]["followups"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,payload) VALUES ($1,'equity','ok',$2,$3,$4,NOW(),NOW(),'test',$5)")
        .bind(Uuid::new_v4()).bind(first_day).bind(first_end-Duration::days(7)).bind(first_end).bind(first.payload)
        .execute(&state.pool).await.unwrap();
    sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,payload) SELECT $1,kind,status,$2,$3,$4,NOW(),NOW(),model,payload FROM ai_insights WHERE id=$5")
        .bind(Uuid::new_v4()).bind(first_day.pred_opt().unwrap()).bind(first_end-Duration::days(2)).bind(first_end-Duration::days(1)).bind(prior.id).execute(&state.pool).await.unwrap();
    assert!(
        history::baseline(&state, first_end)
            .await
            .unwrap()
            .is_none(),
        "an edition without a prior baseline keeps that decision on regeneration"
    );
    check_two_editions(&state, &http, &base, &cookies[0]).await;
    check_retention(&state).await;
    state.shutdown.cancel();
    state.shutdown_workers().await;
    server.abort();
    ai_server.abort();
    state.pool.close().await;
}

/// The cutoffs a page offers as past editions, newest first.
fn listed(page: &Value) -> Vec<Value> {
    page["editions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["window_end"].clone())
        .collect()
}

async fn check_two_editions(state: &AppState, http: &reqwest::Client, base: &str, cookie: &str) {
    use uuid::Uuid;
    async fn page(http: &reqwest::Client, url: String) -> Value {
        let response = http.get(url).send().await.unwrap();
        response.error_for_status().unwrap().json().await.unwrap()
    }
    async fn scheduled(state: &AppState, now: DateTime<Utc>) {
        insight::tick_kind(state, Kind::Equity, now).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while insight::is_running(state) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    // The existing fixtures are hidden: quiet editions need no paid analysis call.
    sqlx::query("TRUNCATE ai_insights,runs CASCADE")
        .execute(&state.pool)
        .await
        .unwrap();
    let morning: DateTime<Utc> = "2026-09-28T22:00:00Z".parse().unwrap();
    let evening: DateTime<Utc> = "2026-09-29T12:45:00Z".parse().unwrap();
    let day = morning.with_timezone(&state.config.timezone).date_naive();
    scheduled(state, morning - Duration::seconds(1)).await;
    scheduled(state, morning).await;
    scheduled(state, evening - Duration::seconds(1)).await;
    let morning_id: Uuid =
        sqlx::query_scalar("SELECT id FROM ai_insights WHERE window_end=$1 AND status='ok'")
            .bind(morning)
            .fetch_one(&state.pool)
            .await
            .unwrap();
    // A failed morning regeneration cannot delay the evening edition.
    sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,error) VALUES ($1,'equity','failed',$2,$3,$4,NOW(),NOW(),'test','morning failure')")
        .bind(Uuid::new_v4()).bind(day).bind(morning-Duration::days(1)).bind(morning).execute(&state.pool).await.unwrap();
    scheduled(state, evening).await;
    scheduled(state, evening + Duration::hours(1)).await;
    let latest: Value = http
        .get(format!("{base}/equity-insight"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(latest["note"]["edition"], "evening");
    assert_eq!(latest["note"]["window_end"], json!(evening));
    assert_eq!(latest["editions"].as_array().unwrap().len(), 3);
    let evening_id = history::baseline(state, morning + Duration::days(1))
        .await
        .unwrap()
        .unwrap()
        .id;
    let comparison = history::baseline(state, evening).await.unwrap().unwrap();
    assert_eq!(
        comparison.id, morning_id,
        "evening compares with this morning"
    );
    let morning_comparison = history::baseline(state, morning).await.unwrap().unwrap();
    assert_eq!(morning_comparison.cutoff, evening - Duration::days(1));
    let (collected, correct_window): (i64, bool) = sqlx::query_as("SELECT COUNT(*),bool_and(i.window_start=r.window_end-INTERVAL '24 hours') FROM runs r JOIN ai_insights i ON i.window_end=r.window_end AND i.kind='equity' AND i.status='ok' WHERE r.triggered_by='briefing' AND r.status='ok'")
        .fetch_one(&state.pool).await.unwrap();
    assert_eq!(
        collected, 3,
        "each cutoff collects once; repeated ticks reuse it"
    );
    assert!(
        correct_window,
        "evening collection uses its own 24-hour window"
    );
    let mut at_url = url::Url::parse(&format!("{base}/equity-insight")).unwrap();
    at_url
        .query_pairs_mut()
        .append_pair("at", &morning.to_rfc3339());
    let historical: Value = http
        .get(at_url.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(historical["note"]["edition"], "morning");
    assert_eq!(historical["note"]["window_end"], json!(morning));
    let by_day: Value = http
        .get(format!("{base}/equity-insight?day={day}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        by_day["note"], latest["note"],
        "day links select that day's latest edition"
    );
    at_url
        .query_pairs_mut()
        .clear()
        .append_pair("at", &(evening + Duration::minutes(1)).to_rfc3339());
    let missing = http.get(at_url).send().await.unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let admin: Value = http
        .get(format!("{base}/api/admin/equity-insight"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(admin["briefing"]["edition"], "evening");
    // Backfilling an older edition must neither replace the latest note nor change a fixed comparison.
    sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,payload) SELECT $1,kind,status,briefing_day,window_start,window_end,NOW(),NOW(),model,payload FROM ai_insights WHERE id=$2")
        .bind(Uuid::new_v4()).bind(morning_id).execute(&state.pool).await.unwrap();
    state.equity.invalidate();
    let after: Value = http
        .get(format!("{base}/equity-insight"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        after, latest,
        "regeneration is grouped into its original edition"
    );
    assert_eq!(
        history::baseline(state, evening).await.unwrap().unwrap().id,
        morning_id
    );
    scheduled(state, morning + Duration::days(1)).await;
    assert_eq!(
        history::baseline(state, morning + Duration::days(1))
            .await
            .unwrap()
            .unwrap()
            .id,
        evening_id
    );
    // Readers are offered the latest edition's day and the two days before it. The days count
    // from the latest published edition: neither a failed edition nor an XAI 日报 moves them.
    let newest = morning + Duration::days(1);
    let (older, oldest) = (newest - Duration::days(3), newest - Duration::days(4));
    for (kind, status, cutoff) in [
        ("equity", "ok", older),
        ("equity", "ok", oldest),
        ("equity", "failed", newest + Duration::days(5)),
        ("ai", "ok", newest + Duration::days(5)),
    ] {
        sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,payload) SELECT $1,$2,$3,$4,window_start,$5,NOW(),NOW(),model,payload FROM ai_insights WHERE id=$6")
            .bind(Uuid::new_v4()).bind(kind).bind(status)
            .bind(cutoff.with_timezone(&state.config.timezone).date_naive()).bind(cutoff).bind(morning_id)
            .execute(&state.pool).await.unwrap();
    }
    state.equity.invalidate();
    let recent = [
        json!(newest),
        json!(evening),
        json!(morning),
        json!(evening - Duration::days(1)),
    ];
    assert_eq!(
        listed(&page(http, format!("{base}/equity-insight")).await),
        recent
    );
    // An older edition opened by a link or by its day is listed with them, once.
    let mut at_url = url::Url::parse(&format!("{base}/equity-insight")).unwrap();
    at_url
        .query_pairs_mut()
        .append_pair("at", &older.to_rfc3339());
    let day = older.with_timezone(&state.config.timezone).date_naive();
    for url in [
        at_url.to_string(),
        format!("{base}/equity-insight?day={day}"),
    ] {
        let opened = page(http, url).await;
        assert_eq!(opened["note"]["window_end"], json!(older));
        assert_eq!(listed(&opened), [&recent[..], &[json!(older)]].concat());
    }
}

/// Notes of both kinds leave 32 days after they were started; a generation still running stays.
async fn check_retention(state: &AppState) {
    use uuid::Uuid;
    let mut kept = Vec::new();
    for (kind, status, days, stays) in [
        ("equity", "ok", 33, false),
        ("equity", "ok", 31, true),
        ("equity", "running", 40, true),
        ("ai", "ok", 33, false),
    ] {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO ai_insights (id,kind,status,briefing_day,window_start,window_end,started_at,finished_at,model,payload) VALUES ($1,$2,$3,CURRENT_DATE,NOW(),NOW(),NOW()-$4*INTERVAL '1 day',CASE WHEN $3='running' THEN NULL ELSE NOW() END,'test','{}')")
            .bind(id).bind(kind).bind(status).bind(days)
            .execute(&state.pool).await.unwrap();
        if stays {
            kept.push(id);
        }
    }
    crate::runner::cleanup_insights(state).await.unwrap();
    let left: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM ai_insights WHERE started_at < NOW() - INTERVAL '30 days' ORDER BY started_at DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert_eq!(left, kept);
}
