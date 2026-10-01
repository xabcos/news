use super::{
    Band, Currency, Mode, Period, Price, Status, Tier,
    extract::{Draft, DraftPrice, Fact},
    headline, pages,
    source::{self, Kind, Page},
    verify,
};
use url::Url;

fn page(kind: Kind, currency: Option<Currency>, lines: &[&str]) -> Page {
    Page {
        source_id: 1,
        kind,
        currency,
        url: "https://api-docs.deepseek.com/zh-cn/quick_start/pricing".into(),
        lines: lines.iter().map(|line| (*line).to_owned()).collect(),
        links: Vec::new(),
        hash: Vec::new(),
    }
}

/// DeepSeek lists models as columns, with peak and off-peak rates on separate rows.
fn deepseek() -> Page {
    page(
        Kind::Pricing,
        Some(Currency::Cny),
        &[
            "# 模型 & 价格",
            "下表所列模型价格以“百万 tokens”为单位。",
            "| 模型 | deepseek-flash(1) | deepseek-v4-pro |",
            "| 上下文长度 | 1M |",
            "| 输出长度 | 最大 384K |",
            "| 价格(2) | 百万tokens输入（缓存命中） | 空闲时段 | 0.02元 | 0.15元 |",
            "| 高峰时段 | 0.04元 | 0.30元 |",
            "| 百万tokens输入（缓存未命中） | 空闲时段 | 1元 | 4.5元 |",
            "| 高峰时段 | 2元 | 9.0元 |",
            "| 百万tokens输出 | 空闲时段 | 4元 | 13.5元 |",
            "| 高峰时段 | 8元 | 27.0元 |",
        ],
    )
}

fn price_draft(period: Period, values: [f64; 3], quote: &str) -> DraftPrice {
    DraftPrice {
        source: 0,
        tier: Tier::Standard,
        label: String::new(),
        context: Band::All,
        context_threshold: None,
        period,
        mode: Mode::All,
        unit: 1_000_000,
        input: Some(values[0]),
        cached_input: Some(values[1]),
        cache_write: None,
        cache_write_1h: None,
        output: Some(values[2]),
        quote: quote.into(),
    }
}

fn draft(model_id: &str, prices: Vec<DraftPrice>) -> Draft {
    Draft {
        model_id: model_id.into(),
        name: "DeepSeek V4 Pro".into(),
        summary: "面向复杂推理与编程的旗舰模型。".into(),
        summary_en: "Flagship model for complex reasoning and coding.".into(),
        status: Status::Active,
        released_on: None,
        knowledge_cutoff: String::new(),
        context_tokens: Some(1_000_000),
        max_output_tokens: Some(393_216),
        input_modalities: vec!["text".into()],
        output_modalities: vec!["text".into()],
        features: vec!["reasoning".into(), "tools".into(), "teleport".into()],
        docs_link: None,
        announcement_link: None,
        facts: vec![
            Fact {
                source: 0,
                quote: "| 上下文长度 | 1M |".into(),
            },
            Fact {
                source: 0,
                quote: "| 输出长度 | 最大 384K |".into(),
            },
        ],
        prices,
    }
}

fn deepseek_prices() -> Vec<DraftPrice> {
    vec![
        price_draft(
            Period::Peak,
            [9.0, 0.30, 27.0],
            "| 高峰时段 | 0.04元 | 0.30元 |\n| 百万tokens输入（缓存未命中） | 空闲时段 | 1元 | 4.5元 |\n| 高峰时段 | 2元 | 9.0元 |\n| 百万tokens输出 | 空闲时段 | 4元 | 13.5元 |\n| 高峰时段 | 8元 | 27.0元 |",
        ),
        price_draft(
            Period::OffPeak,
            [4.5, 0.15, 13.5],
            "| 价格(2) | 百万tokens输入（缓存命中） | 空闲时段 | 0.02元 | 0.15元 | | 高峰时段 | 0.04元 | 0.30元 | | 百万tokens输入（缓存未命中） | 空闲时段 | 1元 | 4.5元 | | 高峰时段 | 2元 | 9.0元 | | 百万tokens输出 | 空闲时段 | 4元 | 13.5元 |",
        ),
    ]
}

#[test]
fn reads_main_content_as_rows_with_official_links_only() {
    let html = r#"<html><body><nav><a href="/docs/elsewhere">Sidebar</a> Navigation</nav>
      <main><h1>Pricing</h1><p>Prices per <b>1M</b> tokens. Every model below is available through the Responses API and the Chat Completions API, billed on input and output tokens.</p>
      <p>Cached input is billed when a request reuses a recent prompt prefix. Batch requests are processed asynchronously within a day at half the standard rate, and priority processing is billed above the standard rate for lower latency. Regional processing endpoints carry an uplift for newer models.</p>
      <table><thead><tr><th>Model</th><th>Input</th><th>Output</th></tr></thead>
      <tbody><tr><td><button type="button" aria-label="Copy model ID">gpt-6.1-sol</button></td><td><div>$2.00</div><div>short</div></td><td><del>$20.00</del> $10.00</td></tr></tbody></table>
      <p>See <a href="/api/docs/models/gpt-6.1-sol">GPT-6.1 Sol</a> or <a href="https://example.com/copy">a copy</a>.</p>
      <script>window.secret = "hidden";</script></main><footer>Footer links</footer></body></html>"#;
    let base = Url::parse("https://developers.openai.com/api/docs/pricing").unwrap();
    let (lines, links) = source::read(html, &base, &["openai.com".into()], 10_000);
    assert_eq!(lines[0], "# Pricing");
    assert_eq!(lines[3], "| Model | Input | Output |");
    // Button text is content: Claude's docs put model IDs inside copy buttons.
    assert_eq!(lines[4], "| gpt-6.1-sol | $2.00 short | $10.00 |");
    assert!(lines.iter().all(|line| !line.contains("secret")
        && !line.contains("Navigation")
        && !line.contains("Footer")));
    assert_eq!(links.len(), 1);
    assert_eq!(
        links[0].url,
        "https://developers.openai.com/api/docs/models/gpt-6.1-sol"
    );
    assert_eq!(links[0].text, "GPT-6.1 Sol");
    let (short, _) = source::read(html, &base, &["openai.com".into()], 40);
    assert!(short.len() < lines.len());
}

#[test]
fn official_hosts_are_the_provider_domains_and_their_subdomains() {
    let domains = ["openai.com".to_owned()];
    for (url, official) in [
        ("https://openai.com/api", true),
        ("https://developers.openai.com/api/docs", true),
        ("https://evilopenai.com/api", false),
        ("https://openai.com.example.net/api", false),
    ] {
        assert_eq!(
            source::official(&Url::parse(url).unwrap(), &domains),
            official,
            "{url}"
        );
    }
}

#[test]
fn names_match_only_as_whole_identifiers() {
    assert!(!verify::mentions("| gpt-6-astra | $10.00 |", "gpt-6"));
    assert!(!verify::mentions("gpt-6.1-sol", "gpt-6"));
    assert!(verify::mentions("GLM-5.3 costs less", "glm-5.3"));
    assert!(!verify::mentions("GLM-5.3-Flash", "glm-5.3"));
    assert!(!verify::mentions(
        "Claude Opus 5.5 is here",
        "Claude Opus 5"
    ));
    assert!(verify::mentions("Use deepseek-v4-pro.", "deepseek-v4-pro"));
}

#[test]
fn excerpts_confirm_prices_but_not_version_numbers() {
    let sol = ["gpt-6.1-sol".to_owned()];
    assert_eq!(
        verify::prices_in("| gpt-6.1-sol | $2.00 | $0.10 | $2.50 | $10.00 |", &sol),
        vec![2.0, 0.1, 2.5, 10.0]
    );
    assert_eq!(
        verify::prices_in("| 高峰时段 | 2元 | 9.0元 |", &[]),
        vec![2.0, 9.0]
    );
    let opus = ["Claude Opus 5.5".to_owned()];
    assert_eq!(
        verify::prices_in("Claude Opus 5.5 $4 / MTok, 1M tokens, V4 128K", &opus),
        vec![4.0]
    );
    assert!(verify::prices_in("| GLM-4.7-Flash | Free | Free |", &[]).contains(&0.0));
    assert!(verify::prices_in("$1,250.50 per 1,000,000", &[]).contains(&1250.5));
}

#[test]
fn token_counts_read_decimal_and_binary_suffixes() {
    let counts = verify::token_counts(
        "| 上下文长度 | 1M | 最大 384K | 1.05M | 500k tokens | 1,050,000 | 5min |",
    );
    for expected in [1_000_000, 1_048_576, 384_000, 393_216, 1_050_000, 500_000] {
        assert!(counts.contains(&expected), "{expected} in {counts:?}");
    }
    assert!(!counts.contains(&5));
}

fn row(tier: Tier, context: Band, period: Period, input: f64, output: f64) -> Price {
    Price {
        currency: Currency::Usd,
        tier,
        label: String::new(),
        context,
        context_threshold: None,
        period,
        mode: Mode::All,
        input: Some(input),
        cached_input: Some(input / 10.0),
        cache_write: None,
        cache_write_1h: None,
        output: Some(output),
    }
}

#[test]
fn price_rows_keep_the_orderings_of_real_tables() {
    let standard = row(Tier::Standard, Band::Short, Period::All, 2.0, 10.0);
    let long = row(Tier::Standard, Band::Long, Period::All, 4.0, 15.0);
    let batch = row(Tier::Batch, Band::Short, Period::All, 1.0, 5.0);
    let fast = row(Tier::Priority, Band::Short, Period::All, 4.0, 20.0);
    assert!(
        verify::consistent(&[standard.clone(), long.clone(), batch.clone(), fast.clone()]).is_ok()
    );
    let mut cached = standard.clone();
    cached.cached_input = Some(3.0);
    assert!(verify::consistent(&[cached]).is_err());
    assert!(
        verify::consistent(&[
            standard.clone(),
            row(Tier::Batch, Band::Short, Period::All, 3.0, 5.0)
        ])
        .is_err()
    );
    assert!(
        verify::consistent(&[
            standard.clone(),
            row(Tier::Standard, Band::Long, Period::All, 1.0, 15.0)
        ])
        .is_err()
    );
    assert!(
        verify::consistent(&[
            standard.clone(),
            row(Tier::Priority, Band::Short, Period::All, 1.0, 20.0)
        ])
        .is_err()
    );
    let peak = row(Tier::Standard, Band::All, Period::Peak, 9.0, 27.0);
    let off_peak = row(Tier::Standard, Band::All, Period::OffPeak, 4.5, 13.5);
    assert!(verify::consistent(&[peak.clone(), off_peak.clone()]).is_ok());
    assert!(
        verify::consistent(&[
            off_peak.clone(),
            row(Tier::Standard, Band::All, Period::Peak, 4.0, 13.5)
        ])
        .is_err()
    );
    assert!(verify::consistent(&[standard.clone(), standard]).is_err());
    assert_eq!(
        headline(&[off_peak, long, peak.clone()], Currency::Usd)
            .unwrap()
            .period,
        Period::Peak
    );
}

#[test]
fn column_tables_verify_each_rate_against_its_rows() {
    let pages = [deepseek()];
    let verified = verify::models(
        vec![draft("deepseek-v4-pro", deepseek_prices())],
        &pages,
        &[],
    );
    assert!(verified.rejected.is_empty(), "{:?}", verified.rejected);
    let model = &verified.models[0];
    let prices = model.prices.as_ref().unwrap();
    assert_eq!(prices.len(), 2);
    assert!(prices.iter().all(|price| price.currency == Currency::Cny));
    assert_eq!(model.context_tokens, Some(1_000_000));
    assert_eq!(model.max_output_tokens, Some(393_216));
    // Only listed capabilities survive.
    assert_eq!(model.features, vec!["reasoning", "tools"]);

    // A number the excerpt does not contain keeps the stored prices.
    let mut prices = deepseek_prices();
    prices[0].output = Some(28.0);
    let verified = verify::models(vec![draft("deepseek-v4-pro", prices)], &pages, &[]);
    assert!(verified.models[0].prices.is_none());
    assert_eq!(verified.rejected[0].reason, "价格数字不在摘录中");

    // Swapped peak and off-peak rates break the table's ordering.
    let mut prices = deepseek_prices();
    prices[0].period = Period::OffPeak;
    prices[1].period = Period::Peak;
    let verified = verify::models(vec![draft("deepseek-v4-pro", prices)], &pages, &[]);
    assert!(verified.models[0].prices.is_none());

    // An excerpt that is not on the page, and an ID the page never names.
    let mut prices = deepseek_prices();
    prices[0].quote = "| 高峰时段 | 2元 | 8.0元 |".into();
    assert!(
        verify::models(vec![draft("deepseek-v4-pro", prices)], &pages, &[]).models[0]
            .prices
            .is_none()
    );
    let verified = verify::models(vec![draft("deepseek-v5", deepseek_prices())], &pages, &[]);
    assert!(verified.models.is_empty());

    // Limits without an excerpt are left out instead of guessed.
    let mut unsupported = draft("deepseek-v4-pro", deepseek_prices());
    unsupported.context_tokens = Some(2_000_000);
    unsupported.facts.clear();
    let verified = verify::models(vec![unsupported], &pages, &[]);
    assert_eq!(verified.models[0].context_tokens, None);
    assert!(verified.models[0].features.is_empty());
}

#[test]
fn a_price_row_quoted_from_another_model_is_rejected() {
    let pages = [page(
        Kind::Pricing,
        Some(Currency::Usd),
        &[
            "## Flagship models",
            "| Model | Input | Cached input | Cache writes | Output |",
            "| gpt-6-astra | $10.00 | $1.00 | $12.50 | $50.00 |",
            "| gpt-6.1-sol | $2.00 | $0.10 | $2.50 | $10.00 |",
        ],
    )];
    let openai = |model_id: &str, name: &str, quote: &str, values: [f64; 4]| {
        let mut draft = draft(model_id, vec![]);
        draft.name = name.into();
        draft.facts.clear();
        draft.prices = vec![DraftPrice {
            cache_write: Some(values[2]),
            output: Some(values[3]),
            ..price_draft(Period::All, [values[0], values[1], values[3]], quote)
        }];
        draft
    };
    let astra_row = "| gpt-6-astra | $10.00 | $1.00 | $12.50 | $50.00 |";
    let verified = verify::models(
        vec![
            openai(
                "gpt-6-astra",
                "GPT-6 Astra",
                astra_row,
                [10.0, 1.0, 12.5, 50.0],
            ),
            openai(
                "gpt-6.1-sol",
                "GPT-6.1 Sol",
                astra_row,
                [10.0, 1.0, 12.5, 50.0],
            ),
        ],
        &pages,
        &[],
    );
    assert!(verified.models[0].prices.is_some());
    assert!(verified.models[1].prices.is_none());
    assert_eq!(verified.rejected[0].reason, "价格摘录属于其他模型");
}

#[test]
fn changes_ignore_how_a_read_labels_the_same_page() {
    use super::store::{limit_moved, price_moved, status_moved};
    let standard = row(Tier::Standard, Band::Short, Period::All, 2.0, 10.0);
    let batch = row(Tier::Batch, Band::Short, Period::All, 1.0, 5.0);
    // The same rates read as "non-thinking" instead of "all", without the batch row.
    let relabeled = Price {
        mode: Mode::NonThinking,
        context: Band::All,
        input: Some(2.0 + 1e-12),
        ..standard.clone()
    };
    assert!(!price_moved(
        &[standard.clone(), batch.clone()],
        &[relabeled]
    ));
    let mut raised = standard.clone();
    raised.output = Some(12.0);
    assert!(price_moved(&[standard.clone(), batch], &[raised]));
    // A read that misses the cached rate, or lists another currency, is not a change.
    let mut uncached = standard.clone();
    uncached.cached_input = None;
    assert!(!price_moved(std::slice::from_ref(&standard), &[uncached]));
    let cny = Price {
        currency: Currency::Cny,
        ..standard.clone()
    };
    assert!(!price_moved(std::slice::from_ref(&standard), &[cny]));
    assert!(!limit_moved(Some(1_000_000), Some(1_048_576)));
    assert!(!limit_moved(None, Some(1_000_000)));
    assert!(limit_moved(Some(256_000), Some(1_000_000)));
    assert!(!status_moved("legacy", Status::Active));
    assert!(!status_moved("active", Status::Legacy));
    assert!(status_moved("preview", Status::Active));
    assert!(status_moved("active", Status::Deprecated));
}

#[test]
fn catalogue_numbers_read_like_the_official_tables() {
    assert_eq!(pages::money(Currency::Usd, 2.0, false), "$2.00");
    assert_eq!(pages::money(Currency::Usd, 0.125, false), "$0.125");
    assert_eq!(pages::money(Currency::Usd, 0.003625, false), "$0.003625");
    assert_eq!(pages::money(Currency::Cny, 0.3, false), "¥0.30");
    assert_eq!(pages::money(Currency::Cny, 0.0, true), "Free");
    for (count, text) in [
        (1_050_000, "1.05M"),
        (1_048_576, "1M"),
        (1_000_000, "1M"),
        (131_072, "128K"),
        (393_216, "384K"),
        (200_000, "200K"),
        (500_000, "500K"),
    ] {
        assert_eq!(pages::tokens(count), text);
    }
    let cny = Price {
        currency: Currency::Cny,
        ..row(Tier::Standard, Band::All, Period::All, 9.0, 27.0)
    };
    let usd = row(Tier::Standard, Band::All, Period::All, 1.32, 3.96);
    assert_eq!(
        pages::primary("cn", &[usd.clone(), cny.clone()]),
        Currency::Cny
    );
    assert_eq!(
        pages::primary("cn", std::slice::from_ref(&usd)),
        Currency::Usd
    );
    assert_eq!(pages::primary("global", &[cny, usd]), Currency::Usd);
}

#[test]
fn the_request_schema_is_strict_and_bounded_by_the_inputs() {
    let vendor = super::store::Vendor {
        id: "deepseek".into(),
        name: "DeepSeek".into(),
        region: "cn".into(),
        homepage: "https://www.deepseek.com".into(),
        note: String::new(),
        domains: vec!["deepseek.com".into()],
        standing: super::store::Standing::Active,
        sources: Vec::new(),
        listed: 0,
    };
    let topic = crate::topics::Topic {
        id: "llm".into(),
        owner_user_id: None,
        plan_locked: false,
        name: "𝑳𝑳𝑴".into(),
        name_en: String::new(),
        description: String::new(),
        brief: "scope".into(),
        enabled: true,
        visible: true,
        interval_minutes: 1440,
        window_start: 4,
        window_start_minute: 30,
        window_end: 24,
        weekdays: vec![1, 2, 3, 4, 5, 6, 7],
        model: "grok-4.7".into(),
        reasoning_effort: "medium".into(),
        max_items: 30,
        lookback_hours: 24,
        daily_run_cap: 3,
        sort_weight: 81,
        next_run_at: None,
        last_run_at: None,
        last_status: None,
        consecutive_failures: 0,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let pages = [deepseek()];
    let body = super::extract::request(&topic, &vendor, &pages, &[], &["deepseek-v4-pro".into()]);
    assert_eq!(body["text"]["format"]["strict"], true);
    let model = &body["text"]["format"]["schema"]["properties"]["models"]["items"];
    fn all_required(schema: &serde_json::Value) {
        if let Some(properties) = schema["properties"].as_object() {
            let required = schema["required"].as_array().unwrap();
            assert_eq!(required.len(), properties.len());
            assert_eq!(schema["additionalProperties"], false);
            properties.values().for_each(all_required);
        }
        if schema["items"].is_object() {
            all_required(&schema["items"]);
        }
    }
    all_required(&body["text"]["format"]["schema"]);
    assert_eq!(model["properties"]["docs_link"]["type"], "null");
    assert_eq!(
        model["properties"]["prices"]["items"]["properties"]["source"]["enum"],
        serde_json::json!([0])
    );
    let input: serde_json::Value = serde_json::from_str(body["input"].as_str().unwrap()).unwrap();
    assert_eq!(input["known"][0], "deepseek-v4-pro");
    assert!(
        input["sources"][0]["text"]
            .as_str()
            .unwrap()
            .contains("| 高峰时段 | 8元 | 27.0元 |")
    );
}

/// Import, a price change, a withdrawal and a hidden model, as readers and the workspace
/// see them. Pages are given directly; fetching is covered by the unit tests above.
#[tokio::test]
#[ignore = "requires NEWS_LLM_TEST_DATABASE_URL and local test configuration"]
async fn postgres_llm_catalog_import_changes_and_pages() {
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    let database_url =
        std::env::var("NEWS_LLM_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(name.starts_with("news_llm_test"));
    pool.close().await;
    let mut config = crate::config::Config::from_env().unwrap();
    config.database_url = database_url;
    config.redis_addr = None;
    config.scheduler_enabled = false;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    config.public_base_url = base.parse().unwrap();
    let state = crate::state::AppState::new(config).await.unwrap();
    for reset in [
        "DELETE FROM llm_changes",
        "DELETE FROM llm_models",
        "DELETE FROM llm_vendors WHERE origin = 'discovered'",
        "UPDATE llm_vendors SET enabled = (id = 'deepseek'), held_since = NULL,
            state = CASE WHEN id = 'deepseek' THEN 'active' ELSE state END",
    ] {
        sqlx::query(reset).execute(&state.pool).await.unwrap();
    }
    sqlx::query("TRUNCATE sessions")
        .execute(&state.pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users")
        .execute(&state.pool)
        .await
        .unwrap();
    let admin: i64 = sqlx::query_scalar(
        "INSERT INTO users (email, role) VALUES ('admin@llm.example.test', 'admin') RETURNING id",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let token = format!("test-llm-{}", uuid::Uuid::new_v4());
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, NOW() + INTERVAL '1 day')")
        .bind(Sha256::digest(token.as_bytes()).to_vec())
        .bind(admin)
        .execute(&state.pool)
        .await
        .unwrap();
    let cookie = format!("{}={token}", state.config.session_cookie_name);
    let vendors = super::store::vendors(&state, super::store::Selection::Listed)
        .await
        .unwrap();
    let vendor = vendors
        .iter()
        .find(|vendor| vendor.id == "deepseek")
        .unwrap();
    let with_ids = |page: Page| Page {
        source_id: vendor.sources[0].id,
        ..page
    };
    let read = |pages: &[Page], prices: Vec<DraftPrice>| {
        verify::models(vec![draft("deepseek-v4-pro", prices)], pages, &[]).models
    };

    // The first read of a provider is an import, not a list of new releases.
    let pages = [with_ids(deepseek())];
    let changes = super::store::apply(
        &state,
        vendor,
        &pages,
        read(&pages, deepseek_prices()),
        true,
    )
    .await
    .unwrap();
    assert_eq!(changes, 0);
    let app = crate::web::router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    let html = http
        .get(format!("{base}/llm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("deepseek-v4-pro") && html.contains("¥9.00") && html.contains("¥27.00"));
    assert!(html.contains("峰谷计价") && html.contains("aria-current=\"true\">𝑳𝑳𝑴"));
    let data: Value = http
        .get(format!("{base}/llm.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(data["models"][0]["prices"].as_array().unwrap().len(), 2);
    assert_eq!(data["models"][0]["context_tokens"], 1_000_000);

    // A new peak output rate on the page becomes one price change.
    let mut raised = deepseek();
    raised.lines[10] = "| 高峰时段 | 8元 | 28.0元 |".into();
    let pages = [with_ids(raised)];
    let mut prices = deepseek_prices();
    prices[0].output = Some(28.0);
    prices[0].quote = prices[0].quote.replace("27.0元", "28.0元");
    let changes = super::store::apply(&state, vendor, &pages, read(&pages, prices), true)
        .await
        .unwrap();
    assert_eq!(changes, 1);
    let html = http
        .get(format!("{base}/llm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("调价") && html.contains("¥27.00 → ¥28.00（+4%）"),
        "{html}"
    );
    let feed = http
        .get(format!("{base}/llm/feed.xml"))
        .send()
        .await
        .unwrap();
    assert_eq!(feed.status(), StatusCode::OK);
    assert!(
        feed.text()
            .await
            .unwrap()
            .contains("deepseek-v4-pro · 调价")
    );

    // An unchanged read records nothing, and neither do rates a later read labels differently.
    assert_eq!(
        super::store::apply(
            &state,
            vendor,
            &pages,
            read(&pages, deepseek_prices_raised()),
            true
        )
        .await
        .unwrap(),
        0
    );
    let mut relabeled = deepseek_prices_raised();
    for price in &mut relabeled {
        price.mode = Mode::NonThinking;
    }
    assert_eq!(
        super::store::apply(&state, vendor, &pages, read(&pages, relabeled), true)
            .await
            .unwrap(),
        0
    );

    // A model the previous pages already named is not new when a read first lists it;
    // one that appears on the pages for the first time is.
    let flash = || Draft {
        name: "DeepSeek Flash".into(),
        ..draft("deepseek-flash", vec![])
    };
    let models = verify::models(
        vec![draft("deepseek-v4-pro", deepseek_prices_raised()), flash()],
        &pages,
        &[],
    )
    .models;
    assert_eq!(
        super::store::apply(&state, vendor, &pages, models, true)
            .await
            .unwrap(),
        0
    );
    let mut launched = pages[0].clone();
    launched.lines.push("| deepseek-v5-pro | 即将推出 |".into());
    let launched = [launched];
    let models = verify::models(
        vec![
            draft("deepseek-v4-pro", deepseek_prices_raised()),
            flash(),
            Draft {
                name: "DeepSeek V5 Pro".into(),
                ..draft("deepseek-v5-pro", vec![])
            },
        ],
        &launched,
        &[],
    )
    .models;
    assert_eq!(
        super::store::apply(&state, vendor, &launched, models, true)
            .await
            .unwrap(),
        1
    );

    // Hidden models leave the public page but stay in the workspace.
    let response = http
        .patch(format!(
            "{base}/api/admin/llm/models/deepseek/deepseek-v4-pro"
        ))
        .header("cookie", &cookie)
        .header("origin", &base)
        .json(&json!({"hidden": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = http
        .get(format!("{base}/llm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!html.contains("id=\"deepseek:deepseek-v4-pro\""));
    let overview: Value = http
        .get(format!("{base}/api/admin/llm"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        overview["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["model_id"] == "deepseek-v4-pro" && model["hidden"] == true)
    );
    assert!(
        http.get(format!("{base}/api/admin/llm"))
            .send()
            .await
            .unwrap()
            .status()
            .is_client_error()
    );

    // A model none of the pages mentions any more is withdrawn once, with a record.
    let mut withdrawn = launched[0].clone();
    withdrawn.lines[2] = "| 模型 | deepseek-flash(1) | deepseek-v5 |".into();
    let changes = super::store::apply(&state, vendor, &[withdrawn], Vec::new(), true)
        .await
        .unwrap();
    assert_eq!(changes, 1);
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM llm_changes ORDER BY id")
        .fetch_all(&state.pool)
        .await
        .unwrap();
    assert_eq!(kinds, ["price", "added", "removed"]);

    // Most of the catalogue leaving at once waits until the pages have left it out a week.
    let blank = with_ids(page(
        Kind::Pricing,
        Some(Currency::Cny),
        &["| 模型 | deepseek-v6 |"],
    ));
    let held = || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT held_since IS NOT NULL FROM llm_vendors WHERE id = 'deepseek'",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap()
    };
    let withdraw = || {
        super::store::apply(
            &state,
            vendor,
            std::slice::from_ref(&blank),
            Vec::new(),
            true,
        )
    };
    assert_eq!(withdraw().await.unwrap(), 0);
    assert!(held().await);
    sqlx::query(
        "UPDATE llm_vendors SET held_since = NOW() - INTERVAL '8 days' WHERE id = 'deepseek'",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    assert_eq!(withdraw().await.unwrap(), 2);
    assert!(!held().await);

    // A provider its pages have not confirmed for two weeks leaves with its models.
    sqlx::query(
        "UPDATE llm_vendors SET verified_at = NOW() - INTERVAL '15 days' WHERE id = 'deepseek'",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let stale = super::store::vendors(&state, super::store::Selection::Stale)
        .await
        .unwrap();
    assert_eq!(stale.len(), 1);
    super::store::retire(&state, &stale[0], "官网无法打开")
        .await
        .unwrap();
    assert!(
        super::store::vendors(&state, super::store::Selection::Listed)
            .await
            .unwrap()
            .is_empty()
    );
    let html = http
        .get(format!("{base}/llm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("官方页面连续两周无法核验") && !html.contains("¥9.00"));
    let overview: Value = http
        .get(format!("{base}/api/admin/llm"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let deepseek_status = overview["vendors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vendor| vendor["id"] == "deepseek")
        .unwrap();
    assert_eq!(deepseek_status["state"], "offline");
    assert_eq!(deepseek_status["error"], "官网无法打开");
    assert!(overview["changes"][0]["model_id"].is_null());

    // The weekly pass finds it again: it returns with the pages it was read from.
    sqlx::query("UPDATE llm_vendors SET checked_at = NULL WHERE id = 'deepseek'")
        .execute(&state.pool)
        .await
        .unwrap();
    let waiting = super::store::vendors(&state, super::store::Selection::Waiting)
        .await
        .unwrap();
    let offline = waiting
        .iter()
        .find(|vendor| vendor.id == "deepseek")
        .unwrap();
    assert_eq!(offline.standing, super::store::Standing::Offline);
    let pages = vec![deepseek()];
    let models = read(&pages, deepseek_prices());
    assert_eq!(
        super::store::publish(&state, offline, pages, models, true)
            .await
            .unwrap(),
        1
    );
    let html = http
        .get(format!("{base}/llm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("恢复收录 · 1 个模型") && html.contains("¥9.00"),
        "{html}"
    );
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM llm_changes ORDER BY id")
        .fetch_all(&state.pool)
        .await
        .unwrap();
    assert_eq!(kinds, ["vendor_removed", "vendor_added"]);
    let urls: Vec<String> =
        sqlx::query_scalar("SELECT url FROM llm_sources WHERE vendor_id = 'deepseek'")
            .fetch_all(&state.pool)
            .await
            .unwrap();
    assert_eq!(urls, [deepseek().url]);

    // A change is news for a day, then it is gone from the page, the JSON and the feed.
    sqlx::query("UPDATE llm_changes SET detected_at = detected_at - INTERVAL '25 hours'")
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(super::prune(&state).await.unwrap(), 2);
    let html = http
        .get(format!("{base}/llm"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!html.contains("llm-changes") && html.contains("¥9.00"));
    let data: Value = http
        .get(format!("{base}/llm.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(data["changes"].as_array().unwrap().is_empty());
    // A withdrawn model leaves with its last change.
    sqlx::query(
        "UPDATE llm_models SET removed_at = NOW() - INTERVAL '25 hours' WHERE vendor_id = 'deepseek'",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    assert_eq!(super::prune(&state).await.unwrap(), 1);

    // Providers from the news wait as candidates and are forgotten if never listed.
    let proposal = super::store::Proposal {
        id: "xiaomi-mimo".into(),
        name: "小米 MiMo".into(),
        name_en: "Xiaomi MiMo".into(),
        region: "cn",
        homepage: "https://mimo.xiaomi.com/".into(),
        pricing: Some("https://platform.xiaomimimo.com/docs/pricing".into()),
        domains: vec!["xiaomi.com".into(), "xiaomimimo.com".into()],
        note: String::new(),
    };
    assert!(super::store::propose(&state, &proposal).await.unwrap());
    assert!(!super::store::propose(&state, &proposal).await.unwrap());
    let waiting = super::store::vendors(&state, super::store::Selection::Waiting)
        .await
        .unwrap();
    let candidate = waiting
        .iter()
        .find(|vendor| vendor.id == "xiaomi-mimo")
        .unwrap();
    assert_eq!(candidate.standing, super::store::Standing::Candidate);
    let first: Vec<(Kind, &str, Option<Currency>)> = candidate
        .sources
        .iter()
        .map(|source| (source.kind, source.url.as_str(), source.currency))
        .collect();
    assert_eq!(
        first,
        [(
            Kind::Pricing,
            "https://platform.xiaomimimo.com/docs/pricing",
            Some(Currency::Cny)
        )]
    );
    sqlx::query(
        "UPDATE llm_vendors SET state_changed_at = NOW() - INTERVAL '57 days' WHERE id = 'xiaomi-mimo'",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    assert_eq!(super::prune(&state).await.unwrap(), 1);
    assert!(
        !super::store::known(&state)
            .await
            .unwrap()
            .iter()
            .any(|vendor| vendor.id == "xiaomi-mimo")
    );
    crate::reading::tests::pages(&state, &base, "llm", &["/llm".into(), "/llm.json".into()]).await;
    server.abort();
}

fn deepseek_prices_raised() -> Vec<DraftPrice> {
    let mut prices = deepseek_prices();
    prices[0].output = Some(28.0);
    prices[0].quote = prices[0].quote.replace("27.0元", "28.0元");
    prices
}

#[test]
fn context_tiers_rise_with_their_thresholds() {
    let tier = |threshold: Option<i64>, input: f64, output: f64| Price {
        context: if threshold.is_some() {
            Band::Long
        } else {
            Band::Short
        },
        context_threshold: threshold,
        ..row(Tier::Standard, Band::Short, Period::All, input, output)
    };
    // Qwen's tables step prices up at 32K and 128K input tokens.
    let tiers = [
        tier(None, 2.5, 10.0),
        tier(Some(32_000), 4.0, 16.0),
        tier(Some(128_000), 7.0, 28.0),
    ];
    assert!(verify::consistent(&tiers).is_ok());
    let dropping = [
        tier(None, 2.5, 10.0),
        tier(Some(32_000), 7.0, 28.0),
        tier(Some(128_000), 4.0, 16.0),
    ];
    assert_eq!(
        verify::consistent(&dropping),
        Err("长上下文价格低于短上下文")
    );
    assert_eq!(
        headline(&tiers, Currency::Usd).unwrap().context_threshold,
        None
    );
}

#[test]
fn a_neighbouring_models_column_does_not_verify() {
    // DeepSeek-V4-Flash's column read as DeepSeek-V4-Pro's: every number is on the rows.
    let mut prices = deepseek_prices();
    prices[0].input = Some(2.0);
    prices[0].cached_input = Some(0.04);
    prices[0].output = Some(8.0);
    prices[1].input = Some(1.0);
    prices[1].cached_input = Some(0.02);
    prices[1].output = Some(4.0);
    let result = verify::models(vec![draft("deepseek-v4-pro", prices)], &[deepseek()], &[]);
    assert!(
        result.models[0].prices.is_none(),
        "{:?}",
        result.models[0].prices
    );
}

#[test]
fn a_unit_the_page_does_not_state_does_not_verify() {
    let mut prices = deepseek_prices();
    for price in &mut prices {
        price.unit = 1000;
    }
    let result = verify::models(vec![draft("deepseek-v4-pro", prices)], &[deepseek()], &[]);
    assert!(
        result.models[0].prices.is_none(),
        "{:?}",
        result.models[0].prices
    );
    assert_eq!(result.rejected[0].reason, "价格单位与页面不符");
}

#[test]
fn limits_from_another_models_section_do_not_verify() {
    let pages = [page(
        Kind::Models,
        None,
        &[
            "## deepseek-v4-pro",
            "Model available via API.",
            "## another-model",
            "another-model has context 2M tokens and maximum output 64K tokens.",
        ],
    )];
    let mut model = draft("deepseek-v4-pro", vec![]);
    model.context_tokens = Some(2_000_000);
    model.max_output_tokens = Some(64_000);
    model.facts = vec![Fact {
        source: 0,
        quote: pages[0].lines[3].clone(),
    }];
    let result = verify::models(vec![model], &pages, &[]);
    assert!(result.models[0].context_tokens.is_none());
    assert!(result.models[0].max_output_tokens.is_none());
}

#[test]
fn a_batch_price_cut_is_a_change() {
    let standard = row(Tier::Standard, Band::All, Period::All, 2.0, 10.0);
    let old_batch = row(Tier::Batch, Band::All, Period::All, 1.0, 5.0);
    let new_batch = row(Tier::Batch, Band::All, Period::All, 0.5, 2.5);
    assert!(super::store::price_moved(
        &[standard.clone(), old_batch],
        &[standard, new_batch]
    ));
}

#[test]
fn identical_tables_verify_under_each_models_own_heading() {
    // Gemini repeats the same table under each Flash model's heading.
    let mut lines = vec![
        "# Gemini Developer API pricing".to_owned(),
        "Prices per 1M tokens.".to_owned(),
    ];
    for (name, id) in [
        ("Gemini 3.8 Flash", "gemini-3.8-flash"),
        ("Gemini 3.7 Flash", "gemini-3.7-flash"),
    ] {
        lines.extend([
            format!("## {name}"),
            id.to_owned(),
            "### Standard".to_owned(),
            "|  | Free Tier | Paid Tier, per 1M tokens in USD |".to_owned(),
            "| Input price | Free of charge | $0.75 |".to_owned(),
            "| Output price | Free of charge | $3.75 |".to_owned(),
        ]);
    }
    let pages = [Page {
        lines,
        ..page(Kind::Pricing, Some(Currency::Usd), &[])
    }];
    let gemini = |id: &str, name: &str| Draft {
        name: name.into(),
        facts: Vec::new(),
        prices: vec![DraftPrice {
            cached_input: None,
            ..price_draft(
                Period::All,
                [0.75, 0.0, 3.75],
                "| Input price | Free of charge | $0.75 | | Output price | Free of charge | $3.75 |",
            )
        }],
        ..draft(id, vec![])
    };
    let verified = verify::models(
        vec![
            gemini("gemini-3.8-flash", "Gemini 3.8 Flash"),
            gemini("gemini-3.7-flash", "Gemini 3.7 Flash"),
        ],
        &pages,
        &[],
    );
    assert!(verified.rejected.is_empty(), "{:?}", verified.rejected);
    assert!(verified.models.iter().all(|model| model.prices.is_some()));
}

#[test]
fn a_models_limits_come_from_its_own_column() {
    // Claude's overview lists models as columns; Haiku's 200K is not Opus's context.
    let pages = [page(
        Kind::Models,
        None,
        &[
            "## Compare models",
            "| Feature | Claude Opus 5.5 For long-running agentic coding | Claude Haiku 4.5 The fastest model |",
            "| Context window | 1M tokens | 200K tokens |",
            "| Max output | 128K tokens | 64K tokens |",
            "| Claude API ID | claude-opus-5-5 | claude-haiku-4-5 |",
        ],
    )];
    let claude = |id: &str, name: &str, context: i64, output: i64| {
        Draft {
        name: name.into(),
        context_tokens: Some(context),
        max_output_tokens: Some(output),
        facts: vec![Fact {
            source: 0,
            quote: "| Context window | 1M tokens | 200K tokens | | Max output | 128K tokens | 64K tokens |".into(),
        }],
        ..draft(id, vec![])
    }
    };
    let verified = verify::models(
        vec![
            claude("claude-opus-5-5", "Claude Opus 5.5", 1_000_000, 128_000),
            claude("claude-haiku-4-5", "Claude Haiku 4.5", 1_000_000, 128_000),
        ],
        &pages,
        &[],
    );
    assert_eq!(verified.models[0].context_tokens, Some(1_000_000));
    assert_eq!(verified.models[0].max_output_tokens, Some(128_000));
    // The numbers are on the row, but in Opus's column.
    assert_eq!(verified.models[1].context_tokens, None);
    assert_eq!(verified.models[1].max_output_tokens, None);
}

#[test]
fn lines_and_versions_come_from_model_ids() {
    use super::lineage;
    for (id, family, version) in [
        ("gpt-6.1-sol", "gpt-#-sol", vec![6, 1]),
        ("gpt-5.6-sol", "gpt-#-sol", vec![5, 6]),
        ("gpt-6-astra", "gpt-#-astra", vec![6]),
        ("claude-opus-5-5", "claude-opus-#", vec![5, 5]),
        ("claude-opus-5", "claude-opus-#", vec![5]),
        ("claude-haiku-4-5-20251001", "claude-haiku-#", vec![4, 5]),
        ("gemini-3.1-pro-preview", "gemini-#-pro", vec![3, 1]),
        ("gemini-3.5-flash-lite", "gemini-#-flash-lite", vec![3, 5]),
        ("qwen3.8-max", "qwen#-max", vec![3, 8]),
        ("qwen3.8-max-0902", "qwen#-max", vec![3, 8]),
        ("qwen3.7-max-2026-06-08", "qwen#-max", vec![3, 7]),
        ("qwen3.8-max-prime", "qwen#-max-prime", vec![3, 8]),
        ("qwen3-vl-plus", "qwen#-vl-plus", vec![3]),
        ("qwq-plus", "qwq-plus", vec![]),
        ("MiniMax-M2.7-highspeed", "minimax-m#-highspeed", vec![2, 7]),
        ("GLM-4.6V-FlashX", "glm-#v-flashx", vec![4, 6]),
        ("GLM-4-32B-0414-128K", "glm-#-32b", vec![4]),
        ("deepseek-v4-pro", "deepseek-v#-pro", vec![4]),
        ("deepseek-flash", "deepseek-flash", vec![]),
        ("ERNIE-4.5-Turbo-128K-Preview", "ernie-#-turbo", vec![4, 5]),
        ("ERNIE-5.0-Thinking-Latest", "ernie-#-thinking", vec![5, 0]),
        ("step-1o-turbo-vision", "step-#o-turbo-vision", vec![1]),
    ] {
        let line = lineage(id);
        assert_eq!(
            (line.family.as_str(), line.version),
            (family, version),
            "{id}"
        );
    }
    assert!(lineage("qwen3.8-max-0902").snapshot && !lineage("qwen3.8-max").snapshot);
    assert!(lineage("gemini-3.1-pro-preview").preview);
}

#[test]
fn readers_see_the_newest_priced_model_of_each_line() {
    let models = [
        ("qwen", "qwen3.8-max", "active", true),
        ("qwen", "qwen3.7-max", "active", true),
        ("qwen", "qwen3.8-max-0902", "active", true),
        ("qwen", "qwen3.7-max-preview", "preview", true),
        ("qwen", "qwen3.8-flash", "active", true),
        ("qwen", "qwen3.9-flash", "active", false),
        ("qwen", "qwen-max", "legacy", true),
        ("google", "gemini-3.1-pro-preview", "preview", true),
        ("google", "gemini-2.5-pro", "active", true),
        ("baidu", "ERNIE-5.0-Thinking-Preview", "preview", true),
        ("baidu", "ERNIE-5.0-Thinking-Latest", "active", true),
        ("openai", "gpt-5.3-codex", "deprecated", true),
    ];
    let shown = super::current(&models, |(vendor, id, status, priced)| {
        (*vendor, *id, *status, *priced)
    });
    let mut shown: Vec<_> = shown.into_iter().map(|(_, id)| id).collect();
    shown.sort_unstable();
    assert_eq!(
        shown,
        [
            "ERNIE-5.0-Thinking-Latest",
            "gemini-3.1-pro-preview",
            "qwen3.8-flash",
            "qwen3.8-max"
        ]
    );
}

#[test]
fn a_family_column_does_not_make_a_row_a_header() {
    // Baidu labels each row with the family, and names the models in the second cell.
    let pages = [page(
        Kind::Pricing,
        Some(Currency::Cny),
        &[
            "## 文心",
            "| 模型 | 版本 | 服务 | 计费项 | 价格 | Batch | 单位 |",
            "| ERNIE 5.0 | ERNIE-5.0 ERNIE-5.0-Thinking-Preview | 推理服务 | 输入 | 0.006 | - | 元/千tokens |",
            "| 输出 | 0.024 | - |",
        ],
    )];
    let ernie = |id: &str, name: &str| Draft {
        name: name.into(),
        facts: Vec::new(),
        prices: vec![DraftPrice {
            unit: 1000,
            cached_input: None,
            ..price_draft(
                Period::All,
                [0.006, 0.0, 0.024],
                "| ERNIE 5.0 | ERNIE-5.0 ERNIE-5.0-Thinking-Preview | 推理服务 | 输入 | 0.006 | - | 元/千tokens | | 输出 | 0.024 | - |",
            )
        }],
        ..draft(id, vec![])
    };
    let verified = verify::models(
        vec![
            ernie("ERNIE-5.0", "ERNIE 5.0"),
            ernie("ERNIE-5.0-Thinking-Preview", "ERNIE 5.0 Thinking"),
        ],
        &pages,
        &[],
    );
    assert!(verified.rejected.is_empty(), "{:?}", verified.rejected);
    // Per thousand tokens on the page, per million in the catalogue.
    assert_eq!(
        verified.models[1].prices.as_ref().unwrap()[0].output,
        Some(24.0)
    );
}

#[test]
fn an_unverifiable_extra_tier_is_dropped_alone() {
    let pages = [deepseek()];
    let mut prices = deepseek_prices();
    prices.push(DraftPrice {
        tier: Tier::Priority,
        label: "Prime".into(),
        quote: "| qwen3.8-max-prime | 24 元 | 72 元 |".into(),
        ..price_draft(Period::All, [24.0, 0.0, 72.0], "")
    });
    let verified = verify::models(vec![draft("deepseek-v4-pro", prices)], &pages, &[]);
    assert_eq!(verified.models[0].prices.as_ref().unwrap().len(), 2);
    assert!(verified.rejected.iter().all(|rejection| !rejection.fatal));
    assert_eq!(verified.rejected[0].reason, "摘录不在官方页面中");

    // A standard rate that fails sets the model's prices aside, keeping the stored ones.
    let mut prices = deepseek_prices();
    prices[0].output = Some(28.0);
    let verified = verify::models(vec![draft("deepseek-v4-pro", prices)], &pages, &[]);
    assert!(verified.models[0].prices.is_none());
    assert!(verified.rejected.iter().all(|rejection| rejection.fatal));
}

fn proposed(
    name_en: &str,
    homepage: &str,
    domains: &[&str],
    items: &[usize],
) -> super::lifecycle::Proposed {
    super::lifecycle::Proposed {
        name: name_en.into(),
        name_en: name_en.into(),
        region: "cn".into(),
        homepage: homepage.into(),
        pricing: String::new(),
        domains: domains.iter().map(|domain| (*domain).to_owned()).collect(),
        note: String::new(),
        items: items.to_vec(),
    }
}

#[test]
fn news_adds_a_provider_reported_twice_once_from_its_own_site() {
    let known = [super::store::Known {
        id: "qwen".into(),
        name: "通义千问".into(),
        name_en: "Qwen".into(),
        region: "cn".into(),
        domains: vec!["aliyun.com".into(), "qwen.ai".into()],
        urls: Vec::new(),
    }];
    let news = [
        "https://mimo.xiaomi.com/mimo-v2-6",
        "https://x.com/_LuoFuli/status/2102766365190901957",
        "https://www.ithome.com/1/006/390.htm",
        "https://www.inceptionlabs.ai/blog/introducing-mercury-voice",
        "https://help.aliyun.com/zh/model-studio/new",
        "https://github.com/jundot/omlx/releases/tag/v0.7.0rc1",
        "https://www.inceptionlabs.ai/blog/introducing-mercury-voice/",
    ];
    let mimo = super::lifecycle::Proposed {
        pricing: "https://platform.xiaomimimo.com/docs/pricing#text".into(),
        ..proposed(
            "Xiaomi MiMo",
            "https://mimo.xiaomi.com",
            &["Xiaomi.com", "xiaomimimo.com", "github.com"],
            &[0, 1, 5],
        )
    };
    // A price page off the provider's domains is not taken, but the provider still is.
    let lab = super::lifecycle::Proposed {
        pricing: "https://openrouter.ai/lab".into(),
        ..proposed(
            "Lab",
            "https://lab.example.com",
            &["lab.example.com"],
            &[7, 1],
        )
    };
    let mut news = news.to_vec();
    news.push("https://lab.example.com/launch");
    let (admitted, rejected) = super::lifecycle::admit(
        vec![
            mimo,
            lab,
            // The same article twice is one report.
            proposed(
                "Inception",
                "https://www.inceptionlabs.ai",
                &["inceptionlabs.ai"],
                &[3, 6],
            ),
            proposed(
                "Ant Ling",
                "https://ling.antgroup.com",
                &["antgroup.com"],
                &[1, 2],
            ),
            proposed(
                "Alibaba Model Studio",
                "https://help.aliyun.com",
                &["help.aliyun.com"],
                &[4, 2],
            ),
            proposed(
                "oMLX",
                "https://github.com/jundot/omlx",
                &["github.com"],
                &[5, 1],
            ),
            proposed(
                "Mercury",
                "https://mercury.example.org",
                &["inceptionlabs.ai"],
                &[3, 1],
            ),
        ],
        &news,
        &known,
    );
    assert_eq!(admitted.len(), 2, "{rejected:?}");
    assert_eq!(admitted[0].id, "xiaomi-mimo");
    assert_eq!(admitted[0].domains, ["xiaomi.com", "xiaomimimo.com"]);
    assert_eq!(admitted[0].homepage, "https://mimo.xiaomi.com/");
    assert_eq!(
        admitted[0].pricing.as_deref(),
        Some("https://platform.xiaomimimo.com/docs/pricing")
    );
    assert_eq!(admitted[1].id, "lab");
    assert_eq!(admitted[1].pricing, None);
    let reasons: Vec<&str> = rejected
        .iter()
        .map(|rejection| rejection["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        reasons,
        [
            "近两周报道不足两篇",
            "报道中没有来自其官网的",
            "已收录",
            "没有可用的官方域名",
            "官网不在其域名下",
        ]
    );
}

#[test]
fn at_most_three_providers_join_a_week_the_most_reported_first() {
    let news: Vec<String> = (0..12)
        .map(|index| format!("https://news{index}.example.com/story"))
        .collect();
    let news: Vec<&str> = news.iter().map(String::as_str).collect();
    let provider = |index: usize, reports: &[usize]| {
        let domain = format!("news{index}.example.com");
        proposed(
            &format!("Lab {index}"),
            &format!("https://{domain}"),
            &[domain.as_str()],
            reports,
        )
    };
    let (admitted, rejected) = super::lifecycle::admit(
        vec![
            provider(0, &[0, 11]),
            provider(1, &[1, 9, 10]),
            provider(2, &[2, 10]),
            provider(3, &[3, 8, 9, 10]),
            // The same provider under a second name.
            proposed(
                "Lab Three",
                "https://news3.example.com",
                &["news3.example.com"],
                &[3, 11],
            ),
        ],
        &news,
        &[],
    );
    let ids: Vec<&str> = admitted
        .iter()
        .map(|proposal| proposal.id.as_str())
        .collect();
    assert_eq!(ids, ["lab-3", "lab-1", "lab-0"]);
    let reasons: Vec<&str> = rejected
        .iter()
        .map(|rejection| rejection["reason"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["本周名额已满", "与本周另一家重复"]);
}

#[test]
fn provider_domains_ids_and_currencies_are_read_strictly() {
    use super::lifecycle::{currency_of, registrable, slug};
    assert_eq!(
        registrable(" WWW.Example.COM. ").as_deref(),
        Some("example.com")
    );
    for shared in [
        "mp.weixin.qq.com",
        "com.cn",
        "co.uk",
        "localhost",
        "https://a.com",
        "a_b.com",
        "user.github.io",
    ] {
        assert_eq!(registrable(shared), None, "{shared}");
    }
    assert_eq!(slug("Xiaomi MiMo").as_deref(), Some("xiaomi-mimo"));
    assert_eq!(slug("Z.ai").as_deref(), Some("z-ai"));
    assert_eq!(slug("月之暗面"), None);
    assert_eq!(
        slug("An Unusually Long Laboratory Name Inc.").as_deref(),
        Some("an-unusually-long-laboratory-nam")
    );
    assert_eq!(
        currency_of("| 输入 | 2元/百万tokens |"),
        Some(Currency::Cny)
    );
    assert_eq!(currency_of("Input $2.50 / 1M tokens"), Some(Currency::Usd));
    assert_eq!(currency_of("¥18 or $2.50"), None);
    assert_eq!(currency_of("Contact sales"), None);
}

#[test]
fn chosen_links_become_pages_without_fragments_or_repeats() {
    use super::lifecycle::{Pick, PricedLink, picked};
    let link = |url: &str| super::source::Link {
        text: "Pricing".into(),
        url: url.into(),
    };
    let links = [
        link("https://platform.example.com/docs/pricing#text-models"),
        link("https://platform.example.com/docs/pricing#vision"),
        link("https://platform.example.com/docs/models"),
        link("http://platform.example.com/insecure"),
    ];
    let sources = picked(
        &Pick {
            pricing: vec![
                PricedLink {
                    link: 0,
                    currency: Currency::Usd,
                },
                PricedLink {
                    link: 1,
                    currency: Currency::Usd,
                },
            ],
            models: vec![2, 3, 9],
            open: Vec::new(),
        },
        &links,
    );
    let pages: Vec<(Kind, &str, Option<Currency>)> = sources
        .iter()
        .map(|source| (source.kind, source.url.as_str(), source.currency))
        .collect();
    assert_eq!(
        pages,
        [
            (
                Kind::Pricing,
                "https://platform.example.com/docs/pricing",
                Some(Currency::Usd)
            ),
            (
                Kind::Models,
                "https://platform.example.com/docs/models",
                None
            ),
        ]
    );
}

#[test]
fn each_context_tier_is_quoted_from_its_own_row() {
    // Qwen names the model once and lists one context tier per row below it.
    let pages = [page(
        Kind::Pricing,
        Some(Currency::Cny),
        &[
            "## 通义千问Coder",
            "| 模型名称 | 单次请求的输入Token范围 | 输入单价（每百万Token） | 输出单价（每百万Token） | 免费额度 |",
            "| qwen3-coder-plus 当前能力等同于 qwen3-coder-plus-2025-09-23 上下文缓存 享有折扣 | 0<Token≤32K | 4 元 | 16 元 | 100 万 Token |",
            "| 32K<Token≤128K | 6 元 | 24 元 |",
            "| 128K<Token≤256K | 10 元 | 40 元 |",
            "| qwen3-coder-flash | 0<Token≤32K | 1 元 | 4 元 | 100 万 Token |",
            "| 32K<Token≤128K | 1.5 元 | 6 元 |",
        ],
    )];
    let tier = |threshold: Option<i64>, input: f64, output: f64, quote: &str| DraftPrice {
        context: if threshold.is_some() {
            Band::Long
        } else {
            Band::Short
        },
        context_threshold: threshold,
        cached_input: None,
        ..price_draft(Period::All, [input, 0.0, output], quote)
    };
    let rows = |quotes: [&str; 3]| {
        vec![
            tier(None, 4.0, 16.0, quotes[0]),
            tier(Some(32_000), 6.0, 24.0, quotes[1]),
            tier(Some(128_000), 10.0, 40.0, quotes[2]),
        ]
    };
    let named = "| qwen3-coder-plus 当前能力等同于 qwen3-coder-plus-2025-09-23 上下文缓存 享有折扣 | 0<Token≤32K | 4 元 | 16 元 | 100 万 Token |";
    let verified = verify::models(
        vec![draft(
            "qwen3-coder-plus",
            rows([
                named,
                "| 32K<Token≤128K | 6 元 | 24 元 |",
                "| 128K<Token≤256K | 10 元 | 40 元 |",
            ]),
        )],
        &pages,
        &[],
    );
    assert!(verified.rejected.is_empty(), "{:?}", verified.rejected);
    assert_eq!(verified.models[0].prices.as_ref().unwrap().len(), 3);

    // The name row stitched to a later tier, skipping the row between, is not an excerpt.
    let stitched = format!("{named} | 128K<Token≤256K | 10 元 | 40 元 |");
    let verified = verify::models(
        vec![draft(
            "qwen3-coder-plus",
            rows([named, "| 32K<Token≤128K | 6 元 | 24 元 |", &stitched]),
        )],
        &pages,
        &[],
    );
    assert!(verified.models[0].prices.is_none());
    assert_eq!(verified.rejected[0].reason, "摘录不在官方页面中");
}

/// Volcengine's model price document as its documentation API serves it.
const DOUBAO_MARKDOWN: &str = r#"不同模型服务支持的能力及单价各不相同，本文为您介绍各模型的计费公式及单价。

# 大语言模型

## 在线推理（常规）

<div data-tips="true" data-tips-type="warning">空闲时段：除上述高峰时段外，其余均为空闲时段。</div>

<span aceTableMode="list" aceTableWidth="1.5,1.5,1,1,1"></span>
|模型名称 |条件<br><br>输入长度：千 token |输入(非音频)<br><br>元/百万token |缓存存储<br><br>元/百万token/小时 |缓存命中(非音频)<br><br>元/百万token |输出<br><br>元/百万token |
|---|---|---|---|---|---|
|doubao\-seed\-2.1\-pro |输入长度 [0, 1024] |6.00 |0.017 |1.20 |30.00 |
|doubao\-seed\-2.0\-pro |输入长度 [0, 32] |3.2 |0.017 |0.64 |16.0 |
||输入长度 (32, 128] |4.8 |0.017 |0.96 |24.0 |
||输入长度 (128, 256] |9.6 |0.017 |1.92 |48.0 |
|deepseek\-v4\-flash正式版<br><br>> 调整前价格，2026\-08\-21 起不适用 |\- |1.00 |0.017 |0.20 |2.00 |

> * 分段计费：部分模型适用，不同输入长度，token单价不同。
~~旧价 9.00~~ 详见[计费说明](/docs/82379/1099320)。
"#;

/// Kimi's price page as the Markdown version its HTML links.
const KIMI_MARKDOWN: &str = r#"# 模型推理价格说明

export const DocTable = ({columns = [], rows = []}) => {
  return <div className="doc-table-wrap">
      <table className="doc-table">
        {rows.map((row, rowIndex) => <tr key={rowIndex}></tr>)}
      </table>
    </div>;
};

## 模型定价

### K2 系列模型

<DocTable
  columns={[
{ title: "模型", width: "24%" },
{ title: "计费单位", width: "12%" },
{ title: "输入价格（缓存命中）", width: "16%" },
{ title: "输出价格", width: "14%" },
]}
  rows={[
["kimi-k2.7-code", "1M tokens", "¥1.30", "¥27.00"],
["kimi-k2.6", "1M tokens", "¥1.10", "¥27.00"],
]}
/>

<Note>
此处 1M = 1,000,000，表格中的价格代表每消耗 1M tokens 的价格。
</Note>
"#;

#[test]
fn documentation_markdown_reads_like_a_page() {
    let base = url::Url::parse("https://www.volcengine.com/docs/82379/1544106").unwrap();
    let (lines, links) =
        super::source::markdown::read(DOUBAO_MARKDOWN, &base, &["volcengine.com".into()], 150_000);
    assert_eq!(
        lines[1..8],
        [
            "# 大语言模型",
            "## 在线推理（常规）",
            "空闲时段：除上述高峰时段外，其余均为空闲时段。",
            "| 模型名称 | 条件 输入长度：千 token | 输入(非音频) 元/百万token | 缓存存储 元/百万token/小时 | 缓存命中(非音频) 元/百万token | 输出 元/百万token |",
            "| doubao-seed-2.1-pro | 输入长度 [0, 1024] | 6.00 | 0.017 | 1.20 | 30.00 |",
            "| doubao-seed-2.0-pro | 输入长度 [0, 32] | 3.2 | 0.017 | 0.64 | 16.0 |",
            "| 输入长度 (32, 128] | 4.8 | 0.017 | 0.96 | 24.0 |",
        ]
    );
    assert_eq!(
        lines[9],
        "| deepseek-v4-flash正式版 > 调整前价格，2026-08-21 起不适用 | - | 1.00 | 0.017 | 0.20 | 2.00 |"
    );
    assert_eq!(
        lines[10],
        "- 分段计费：部分模型适用，不同输入长度，token单价不同。"
    );
    // Struck-through text is not read; links on the provider's domains are kept.
    assert_eq!(lines[11], "详见计费说明。");
    assert_eq!(
        links[0].url,
        "https://www.volcengine.com/docs/82379/1099320"
    );

    let base = url::Url::parse("https://platform.kimi.com/docs/pricing/chat").unwrap();
    let (lines, _) =
        super::source::markdown::read(KIMI_MARKDOWN, &base, &["kimi.com".into()], 150_000);
    assert_eq!(
        lines,
        [
            "# 模型推理价格说明",
            "## 模型定价",
            "### K2 系列模型",
            "| 模型 | 计费单位 | 输入价格（缓存命中） | 输出价格 |",
            "| kimi-k2.7-code | 1M tokens | ¥1.30 | ¥27.00 |",
            "| kimi-k2.6 | 1M tokens | ¥1.10 | ¥27.00 |",
            "此处 1M = 1,000,000，表格中的价格代表每消耗 1M tokens 的价格。",
        ]
    );
}

#[test]
fn a_doubao_price_read_from_its_documentation_verifies() {
    let base = url::Url::parse("https://www.volcengine.com/docs/82379/1544106").unwrap();
    let (lines, _) =
        super::source::markdown::read(DOUBAO_MARKDOWN, &base, &["volcengine.com".into()], 150_000);
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let pages = [page(Kind::Pricing, Some(Currency::Cny), &lines)];
    let tier = |threshold: Option<i64>, values: [f64; 3], quote: &str| DraftPrice {
        context: if threshold.is_some() {
            Band::Long
        } else {
            Band::Short
        },
        context_threshold: threshold,
        ..price_draft(Period::All, values, quote)
    };
    let verified = verify::models(
        vec![draft(
            "doubao-seed-2.0-pro",
            vec![
                tier(
                    None,
                    [3.2, 0.64, 16.0],
                    "| doubao-seed-2.0-pro | 输入长度 [0, 32] | 3.2 | 0.017 | 0.64 | 16.0 |",
                ),
                tier(
                    Some(32_000),
                    [4.8, 0.96, 24.0],
                    "| 输入长度 (32, 128] | 4.8 | 0.017 | 0.96 | 24.0 |",
                ),
                tier(
                    Some(128_000),
                    [9.6, 1.92, 48.0],
                    "| 输入长度 (128, 256] | 9.6 | 0.017 | 1.92 | 48.0 |",
                ),
            ],
        )],
        &pages,
        &[],
    );
    assert!(verified.rejected.is_empty(), "{:?}", verified.rejected);
    // Lengths are given in thousands of tokens: (32, 128] is the tier from 32K.
    let thresholds: Vec<Option<i64>> = verified.models[0]
        .prices
        .as_ref()
        .unwrap()
        .iter()
        .map(|price| price.context_threshold)
        .collect();
    assert_eq!(thresholds, [None, Some(32_000), Some(128_000)]);
}

#[test]
fn volcengine_documents_are_read_through_their_documentation_api() {
    let api = |url: &str| {
        super::source::volcengine_doc(&url::Url::parse(url).unwrap()).map(|api| api.to_string())
    };
    assert_eq!(
        api("https://console.volcengine.com/ark/region:cn-beijing/docs/ark/model-pricing?lang=zh")
            .as_deref(),
        Some(
            "https://docs.volcengine.com/api/doc/getDocDetail?LibraryCode=ark&DocumentCode=model-pricing&type=online"
        )
    );
    assert_eq!(
        api("https://www.volcengine.com/docs/82379/1544106").as_deref(),
        Some(
            "https://docs.volcengine.com/api/doc/getDocDetail?LibraryID=82379&DocumentID=1544106&type=online"
        )
    );
    assert_eq!(api("https://www.volcengine.com/product/ark"), None);
    assert_eq!(api("https://docs.example.com/docs/ark/model-pricing"), None);
}

#[test]
fn official_pages_can_be_named_in_the_brief() {
    let brief = "中文：收录主流大语言模型。官方价格页：豆包 https://console.volcengine.com/ark/region:cn-beijing/docs/ark/model-pricing?lang=zh；Kimi（https://platform.kimi.com/docs/pricing/chat）。http://insecure.example.com https://platform.kimi.com/docs/pricing/chat.";
    let urls: Vec<String> = super::lifecycle::hinted(brief)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        urls,
        [
            "https://console.volcengine.com/ark/region:cn-beijing/docs/ark/model-pricing?lang=zh",
            "https://platform.kimi.com/docs/pricing/chat",
        ]
    );
}
