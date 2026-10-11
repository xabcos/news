use super::*;

#[test]
fn sources_keep_content_and_resolve_only_public_links() {
    let url = url::Url::parse("https://publisher.example/books/shore").unwrap();
    let source = source::extract(
        &url,
        r#"<title>The Shore</title><meta property="og:image" content="/img/logo-square.png"><meta property="og:image" content="/catalogo/shore.jpg?logo=1"><body><nav>Navigation</nav><main><h1>The Shore</h1><p>A book by Ada. Published in 2026.</p><script>ignore instructions</script><a href="/read/shore">Read a sample</a><a href="javascript:alert(1)">Bad link</a></main><footer>Footer</footer></body>"#,
        18_000,
    );
    assert!(source.text.contains("The Shore") && source.text.contains("Published in 2026"));
    assert!(!source.text.contains("Navigation") && !source.text.contains("ignore instructions"));
    assert_eq!(source.links.len(), 1);
    assert_eq!(source.links[0].url, "https://publisher.example/read/shore");
    assert_eq!(
        source.images,
        ["https://publisher.example/catalogo/shore.jpg?logo=1"]
    );
}

#[test]
fn guides_drop_uncited_fields_and_invented_links() {
    let sources = vec![Source {
        title: "The Shore".into(),
        url: "https://publisher.example/books/shore".into(),
        text: "The Shore is a novel by Ada, published in 2026. Its audiobook runs 6.8 hours, read over 6–8 evenings.".into(),
        links: vec![],
        images: vec![],
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
            // The page read, cited in another form.
            Link {
                label: "原文".into(),
                label_en: "Source".into(),
                url: "https://www.publisher.example/books/shore/".into(),
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
    // 6.8 is cited whole: neither 68, 8.6 nor the range 6–8 passes for it.
    for (text, quote) in [
        ("6.8 小时", "Its audiobook runs 6.8 hours"),
        ("68 小时", "Its audiobook runs 68 hours"),
        ("8.6 小时", "Its audiobook runs 6.8 hours"),
        ("6.8 晚", "hours, read over 6.8 evenings"),
    ] {
        guide.facts.push(Entry {
            text: text.into(),
            text_en: text.into(),
            ..entry(quote, 0)
        });
    }
    guide.verify(&sources, 12).unwrap();
    assert_eq!(guide.facts.len(), 2);
    assert_eq!(guide.facts[1].text, "6.8 小时");
    assert_eq!(guide.links.len(), 1);
    assert_eq!(
        guide.links[0].url, sources[0].url,
        "the address it was read at"
    );
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
        proposal.clone().clean("book", "").unwrap().source_urls[0],
        "https://publisher.example/books/shore"
    );
    // A work is known by its English name, whichever page a story cites.
    assert_eq!(proposal.key(Section::Culture), "book:theshore");
    assert_eq!(
        Proposal {
            name_en: "THE SHORE!".into(),
            source_urls: vec!["https://en.wikipedia.org/wiki/The_Shore".into()],
            ..proposal.clone()
        }
        .key(Section::Culture),
        "book:theshore"
    );
    assert!(
        Proposal {
            name_en: "……".into(),
            ..proposal.clone()
        }
        .clean("book", "")
        .is_none()
    );
    assert!(proposal.clone().clean("movie", "").is_none());
    assert!(
        Proposal {
            source_urls: vec!["https://publisher.example/".into()],
            ..proposal.clone()
        }
        .clean("book", "")
        .is_none()
    );
    assert!(
        Proposal {
            source_urls: vec!["https://127.0.0.1/private".into()],
            ..proposal
        }
        .clean("book", "")
        .is_none()
    );
}

#[test]
fn products_keep_official_sources_and_are_known_by_brand_and_model() {
    let brief = "官方来源：amazon.com aboutamazon.com；官方账号：@AmazonKindle。";
    let proposal = Proposal {
        name: "Kindle Colorsoft".into(),
        name_en: "Amazon Kindle Colorsoft".into(),
        creator: "Amazon".into(),
        kind: "reader".into(),
        source_urls: vec![
            "https://www.amazon.com/dp/B0DK".into(),
            "https://shop.example/kindle-colorsoft".into(),
            "https://x.com/AmazonKindle/status/2105624259129127408".into(),
        ],
    };
    let product = proposal.clone().clean("eink", brief).unwrap();
    assert_eq!(
        product.source_urls,
        [
            "https://www.amazon.com/dp/B0DK",
            "https://x.com/AmazonKindle/status/2105624259129127408"
        ]
    );
    assert_eq!(product.key(Section::Gadgets), "amazon:kindlecolorsoft");
    // A page is kept at the address given, once however many forms it is cited in.
    let forms = Proposal {
        source_urls: vec![
            "https://www.amazon.com/dp/B0DK/#specs".into(),
            "https://amazon.com/dp/B0DK".into(),
        ],
        ..proposal.clone()
    };
    assert_eq!(
        forms.clean("eink", brief).unwrap().source_urls,
        ["https://www.amazon.com/dp/B0DK/"]
    );
    let renamed = Proposal {
        name_en: "Kindle Colorsoft".into(),
        ..proposal.clone()
    };
    assert_eq!(
        renamed.clean("eink", brief).unwrap().key(Section::Gadgets),
        "amazon:kindlecolorsoft"
    );
    let plus = Proposal {
        name_en: "Kindle Colorsoft+".into(),
        ..proposal.clone()
    };
    assert_eq!(plus.key(Section::Gadgets), "amazon:kindlecolorsoftplus");
    // A product without an official page still collects its reviews, but no guide is written.
    let unofficial = Proposal {
        source_urls: vec![
            "https://notamazon.com/kindle".into(),
            "https://amazon.com/".into(),
            "https://community.amazon.com/kindle/thread".into(),
            "https://amazon.com/gp/customer-reviews/R1".into(),
            "https://x.com/reseller/status/2105624259129127408".into(),
        ],
        ..proposal.clone()
    };
    assert!(
        unofficial
            .clean("eink", brief)
            .unwrap()
            .source_urls
            .is_empty()
    );
    assert!(proposal.clone().clean("digital", brief).is_none());
    assert!(
        Proposal {
            creator: String::new(),
            ..proposal
        }
        .clean("eink", brief)
        .is_none()
    );
    // `brand.*` stands for the brand's sites in every country, and only for that brand; an
    // official shop's product page counts. A product's own brand needs no mention: its name,
    // folded, is a site's name in any country.
    let regional = "官方来源：sony.* apple.com vmall.com。";
    for (brand, url, official) in [
        (
            "Sony",
            "https://www.sony.jp/headphone/products/WH-1000XM6/",
            true,
        ),
        (
            "Sony",
            "https://www.sony.co.uk/electronics/headband-headphones/wh-1000xm6",
            true,
        ),
        ("Sony", "https://store.sony.com.cn/product/wh-1000xm6", true),
        ("Sony", "https://www.sony.com/electronics/support", true),
        ("Sony", "https://www.sony.jp/", false),
        ("Sony", "https://sony.example.com/wh-1000xm6", false),
        ("Sony", "https://notsony.jp/wh-1000xm6", false),
        ("Sony", "https://www.apple.com.cn/iphone/", false),
        (
            "HUAWEI",
            "https://item.vmall.com/product/comdetail/index.html?prdId=10086172940673",
            true,
        ),
        (
            "Cambridge Audio",
            "https://www.cambridgeaudio.com/gbr/en/products/headphones/melomania-a100-se",
            true,
        ),
        (
            "MemoMind",
            "https://www.memo-mind.com/pages/memomind-one",
            true,
        ),
        (
            "Cambridge Audio",
            "https://cambridgeaudio.shop/melomania-a100-se",
            false,
        ),
        ("Cambridge Audio", "https://www.cambridgeaudio.com/", false),
    ] {
        assert_eq!(
            super::gadgets::official(regional, brand, url),
            official,
            "{brand} {url}"
        );
    }
}

#[test]
fn gadgets_runs_look_for_each_kind_in_turn() {
    let kinds = Section::Gadgets.kinds("digital");
    let rotation = |turn| super::gadgets::rotation("digital", turn);
    let seen = (0..kinds.len() as i64)
        .filter_map(rotation)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(seen.len(), kinds.len());
    assert_eq!(rotation(-1), rotation(kinds.len() as i64 - 1));
    assert_eq!(rotation(3).as_deref(), Some("显示器（monitor）"));
    assert_eq!(super::gadgets::rotation("book", 0), None);
}

#[test]
fn sources_name_works_whole_and_products_word_by_word() {
    let entry = |name_en: &str, creator: &str| Pending {
        id: Uuid::nil(),
        name: name_en.into(),
        name_en: name_en.into(),
        creator: creator.into(),
        kind: String::new(),
        source_urls: sqlx::types::Json(Vec::new()),
        source_hash: None,
        images: sqlx::types::Json(Vec::new()),
    };
    let page = "Meet the all-new Kindle. Available in 2026 in Black and Raspberry.";
    assert!(named(
        Section::Gadgets,
        &entry("Kindle (2026)", "Amazon"),
        page
    ));
    assert!(named(
        Section::Gadgets,
        &entry("Amazon Kindle", "Amazon"),
        page
    ));
    assert!(!named(
        Section::Gadgets,
        &entry("Kindle Scribe (2026)", "Amazon"),
        page
    ));
    assert!(named(Section::Culture, &entry("Kindle", ""), page));
    assert!(!named(Section::Culture, &entry("Kindle (2026)", ""), page));
    assert!(!named(Section::Culture, &entry("Kindle", "Ada"), page));
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
                } else if path == "/products/colorsoft" {
                    ("text/html; charset=utf-8", format!("<title>Kindle Colorsoft — Amazon</title><meta property=\"og:image\" content=\"{url}/cover.jpg\"><main><h1>Kindle Colorsoft</h1><p>Kindle Colorsoft is the first color Kindle. It has a 7-inch Colorsoft display with 300 ppi in black and white, a front light that adjusts its warmth, and a battery that lasts up to eight weeks. It is waterproof and ships with 16 GB of storage.</p></main>").into_bytes())
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

/// Waits until the run is recorded and its worker has let the topic go: the worker records the
/// run before it refreshes the pages, and a manual run of the topic is refused until then.
async fn finished(state: &crate::state::AppState, run: Uuid) {
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let (topic, status): (String, String) =
                sqlx::query_as("SELECT topic_id, status FROM runs WHERE id=$1")
                    .bind(run)
                    .fetch_one(&state.pool)
                    .await
                    .unwrap();
            if status != "running" && !state.in_flight().contains(&topic) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires NEWS_CULTURE_TEST_DATABASE_URL and local test configuration"]
async fn postgres_catalog_publishes_news_then_sourced_guides_and_keeps_the_briefing_separate() {
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
            let input = request["input"].as_str().unwrap_or_default();
            let payload=if request["text"]["format"]["name"]=="official_pages" {
                // One product's official page is found, beside a reseller's; the others have none.
                if input.contains("Talked one") {
                    json!({"urls":[format!("{base}/products/talked-one"),"https://reseller.example/talked-one"]})
                } else {
                    json!({"urls":[]})
                }
            } else if request["text"]["format"]["name"]=="catalog_guide" && input.contains("Kindle Colorsoft") {
                assert!(input.contains("7-inch Colorsoft display"));
                json!({"intro":"亚马逊首款彩色 Kindle 阅读器。","intro_en":"Amazon's first color Kindle.",
                    "facts":[{"label":"屏幕","label_en":"Display","text":"7 英寸，300 ppi","text_en":"7-inch, 300 ppi","source":0,"quote":"7-inch Colorsoft display with 300 ppi"},
                        {"label":"存储","label_en":"Storage","text":"32 GB","text_en":"32 GB","source":0,"quote":"ships with 16 GB of storage"}],
                    "sections":[],"links":[]})
            } else if request["instructions"].as_str().unwrap_or_default().contains("𝑮𝒂𝒅𝒈𝒆𝒕𝒔") {
                assert_eq!(request["text"]["format"]["schema"]["properties"]["items"]["items"]["properties"]["role"]["enum"], json!(["review","share","news"]));
                let now = chrono::Utc::now() - chrono::Duration::hours(1);
                let product = json!({"name":"Kindle Colorsoft","name_en":"Amazon Kindle Colorsoft","creator":"Amazon","kind":"reader",
                    "source_urls":[format!("{base}/products/colorsoft"),"https://shop.example/colorsoft"]});
                let picture = json!({"url":format!("{base}/cover.jpg"),"source_url":format!("{base}/review/colorsoft"),"caption":"Kindle Colorsoft","caption_en":"Kindle Colorsoft","credit":"Reviewer"});
                let share_url = "https://x.com/reader/status/2105624259129127409";
                json!({"items":[
                    {"title":"Kindle Colorsoft 一个月实测","title_en":"A month with Kindle Colorsoft","summary":"连续阅读一个月后，彩色漫画清晰，续航约六周。","summary_en":"After a month, color comics look sharp and the battery lasts about six weeks.","url":format!("{base}/review/colorsoft"),"source":"Reviewer","published_at":now,"importance":4,"tags":["评测"],"tags_en":["Review"],"image":picture,"image_page_url":null,"role":"review","work":product},
                    {"title":"彩色 Kindle 到手","title_en":"My color Kindle arrived","summary":"读者晒出开箱照片。","summary_en":"A reader shares unboxing photos.","url":share_url,"source":"reader","published_at":now,"importance":3,"tags":["开箱"],"tags_en":["Unboxing"],"image":null,"image_page_url":share_url,"role":"share","work":{"name":"Kindle Colorsoft","name_en":"Kindle Colorsoft","creator":"Amazon","kind":"reader","source_urls":[]}},
                    {"title":"传闻：新款 Kindle Scribe 将有彩色版","title_en":"Rumor: a color Kindle Scribe is coming","summary":"爆料者称新款 Scribe 将配备彩色屏幕。","summary_en":"A leaker says the next Scribe gets a color screen.","url":format!("{base}/rumor/scribe"),"source":"Leaker","published_at":now,"importance":2,"tags":["传闻"],"tags_en":["Rumor"],"image":picture,"image_page_url":null,"role":"news","work":null}
                ],"cover":null})
            } else if request["text"]["format"]["name"]=="catalog_guide" {
                let published:i64=sqlx::query_scalar("SELECT COUNT(*) FROM items WHERE topic_id='book'").fetch_one(&pool).await.unwrap();
                assert_eq!(published,3,"stories with and without pictures are already published while the guide is written");
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
                let item=json!({"role":"news","title":"《海岸》新书发布","title_en":"The Shore is published","summary":"Ada 的新小说聚焦海岸社区与故乡记忆。","summary_en":"Ada's new novel explores coastal communities and memories of home.","url":format!("{base}/news/shore"),"source":"Publisher","published_at":chrono::Utc::now()-chrono::Duration::hours(1),"importance":3,"tags":["小说"],"tags_en":["Fiction"],
                    "image":{"url":format!("{base}/blocked.jpg"),"source_url":format!("{base}/news/shore"),"caption":"《海岸》书封","caption_en":"The Shore cover","credit":"Publisher"},"image_page_url":null,
                    "work":{"name":"海岸","name_en":"The Shore","creator":"Ada","kind":"book","source_urls":[format!("{base}/work/shore")]}});
                let mut unpictured = item.clone();
                unpictured["title"]=json!("文学奖公布入围名单");
                unpictured["title_en"]=json!("A literary prize announces its shortlist");
                unpictured["url"]=json!(format!("{base}/unavailable"));
                unpictured["image"]=Value::Null;
                unpictured["work"]=Value::Null;
                // A review cites another page about the same work under another English title;
                // its name joins them.
                let mut review = item.clone();
                review["title"]=json!("书评：《海岸》写尽故乡的退潮");
                review["title_en"]=json!("Review: The Shore and the ebb of home");
                review["url"]=json!(format!("{base}/review/shore"));
                review["image"]=Value::Null;
                review["role"]=json!("review");
                review["work"]["name_en"]=json!("Shore: A Novel");
                review["work"]["source_urls"]=json!([format!("{base}/review/shore")]);
                json!({"items":[item.clone(),item,unpictured,review],"cover":null})
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
    sqlx::query("TRUNCATE catalog_items, catalog_entries, items, runs CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    let everything: Vec<&str> = Section::ALL
        .iter()
        .flat_map(|section| section.topics())
        .copied()
        .collect();
    sqlx::query("UPDATE topics SET enabled=true,visible=true,last_run_at=NULL,last_status=NULL WHERE id=ANY($1)").bind(&everything).execute(&pool).await.unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    let topics: Vec<Topic> = sqlx::query_as("SELECT * FROM topics WHERE id=ANY($1)")
        .bind(Section::Culture.topics())
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
    finished(&state, run).await;
    let (new,dup,status):(i32,i32,String)=sqlx::query_as("SELECT new_count,dup_count,status FROM runs WHERE topic_id='book' ORDER BY started_at DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!((new, dup, status.as_str()), (3, 1, "ok"));
    let (id, slug, key): (Uuid, String, String) = sqlx::query_as(
        "SELECT id,slug,key FROM catalog_entries WHERE topic_id='book' AND guide IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(key, "book:theshore");
    let entries: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM catalog_entries WHERE topic_id='book'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(entries, 1, "the review joins the work its news named");
    // The next run looks into the work with the fewest items, here the only one; Culture has no
    // kind to look for in turn.
    let window = NewsWindow {
        start: chrono::Utc::now() - chrono::Duration::hours(24),
        end: chrono::Utc::now(),
    };
    assert_eq!(
        focus(&state, topic, window).await.unwrap(),
        "\n本轮深挖：海岸（The Shore，Ada）"
    );
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
        page.contains("catalog-book") && page.contains(&slug) && page.contains("/files/cover.jpg")
    );
    // A shelf of works heads the list; a collection of one kind has no filters.
    assert!(page.contains("<section class=catalog-shelf><h2>近期有动态 <a class=catalog-all"));
    assert!(page.contains("<span class=\"badge catalog-role\">评论</span>"));
    assert!(!page.contains("catalog-kinds"));
    // One tab leads into the section, named in the page's language like the equity notes.
    assert_eq!(
        page.matches("<a class=tab href=\"/culture\" data-last=\"/culture/book\" aria-current=\"true\">文娱</a>")
            .count(),
        1
    );
    assert!(page.contains(">股市</a>"));
    let detail = get(format!("/culture/book/{slug}"))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        detail.contains("内容与背景") && detail.contains("/sample") && detail.contains("source-0")
    );
    assert!(detail.contains("<h2>评论</h2>") && detail.contains("书评：《海岸》"));
    assert!(detail.contains("<p class=catalog-eyebrow>图书</p>"));
    let english = get(format!("/culture/book/{slug}?lang=en"))
        .await
        .text()
        .await
        .unwrap();
    assert!(english.contains("Story and background") && english.contains("hreflang=\"zh-CN\""));
    assert!(english.contains(">𝑪𝒖𝒍𝒕𝒖𝒓𝒆</a>") && english.contains(">𝑬</a>"));
    // The section's own tabs name its topics in the page's language too.
    assert!(page.contains("aria-current=\"true\">图书</a>"));
    assert!(english.contains("aria-current=\"true\">𝑩𝒐𝒐𝒌</a>") && !english.contains(">图书</a>"));
    // Search finds works by name, English name or creator and stories by title across the
    // section, reading its words literally; its pages stay out of search engines. A topic
    // without words lists all of its works, and nothing at all leads back to the section.
    assert!(
        page.contains("<form class=catalog-search action=\"/culture/search\" role=search>")
            && page.contains("href=\"/culture/search?topic=book\">全部作品 →</a>")
    );
    let found = get("/culture/search?q=%E6%B5%B7%E5%B2%B8".into()).await;
    assert_eq!(found.headers()["x-robots-tag"], "noindex");
    let found = found.text().await.unwrap();
    assert!(
        found.contains(&format!("href=\"/culture/book/{slug}\""))
            && found.contains("《海岸》新书发布")
            && found.contains("value=\"海岸\"")
    );
    for words in ["ADA", "the%20shore"] {
        let page = get(format!("/culture/search?q={words}"))
            .await
            .text()
            .await
            .unwrap();
        assert!(
            page.contains(&format!("href=\"/culture/book/{slug}\"")),
            "{words}"
        );
    }
    // The catalogue matches its words as one phrase.
    assert!(
        !get("/culture/search?q=shore%20the".into())
            .await
            .text()
            .await
            .unwrap()
            .contains(&format!("href=\"/culture/book/{slug}\""))
    );
    // A work is found by its stories' titles too, while they are visible.
    let found_works = |page: String| {
        page.split_once("<section class=\"catalog-shelf catalog-found\">")
            .is_some_and(|(_, rest)| {
                rest.split_once("</section>")
                    .unwrap()
                    .0
                    .contains(&format!("href=\"/culture/book/{slug}\""))
            })
    };
    let by_story = "/culture/search?q=%E6%96%B0%E4%B9%A6%E5%8F%91%E5%B8%83";
    assert!(found_works(
        get(by_story.into()).await.text().await.unwrap()
    ));
    sqlx::query("UPDATE items SET hidden = NOT hidden WHERE url LIKE '%/news/shore'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!found_works(
        get(by_story.into()).await.text().await.unwrap()
    ));
    sqlx::query("UPDATE items SET hidden = NOT hidden WHERE url LIKE '%/news/shore'")
        .execute(&pool)
        .await
        .unwrap();
    let literal = get("/culture/search?q=%25".into())
        .await
        .text()
        .await
        .unwrap();
    assert!(literal.contains("没有找到") && !literal.contains(&slug));
    let all = get("/culture/search?topic=book".into()).await;
    assert!(all.headers().get("x-robots-tag").is_none());
    assert!(
        all.text()
            .await
            .unwrap()
            .contains(&format!("href=\"/culture/book/{slug}\""))
    );
    let nothing = get("/culture/search?q=%20".into()).await;
    assert_eq!(nothing.status(), 307);
    assert_eq!(nothing.headers()["location"], "/culture");
    assert!(
        get("/culture/search?q=shore&lang=en".into())
            .await
            .text()
            .await
            .unwrap()
            .contains("<input type=hidden name=lang value=en>")
    );
    let digest: Value = get("/digest?topic=book".into()).await.json().await.unwrap();
    let stories = digest["topics"][0]["items"].as_array().unwrap();
    let story = stories
        .iter()
        .find(|s| s["url"].as_str().unwrap().ends_with("/news/shore"))
        .unwrap();
    assert_eq!(story["catalog"]["name"], "海岸");
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
    sqlx::query("UPDATE catalog_entries SET checked_at=NOW()-INTERVAL '8 days' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let results = guides(
        &state,
        topic,
        Section::Culture,
        &mut Usage::default(),
        Instant::now() + Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(results[0]["unchanged"], true);
    assert_eq!(calls.lock().unwrap().len(), before);
    // An unavailable source keeps the published guide and counts as an attempt.
    sqlx::query("UPDATE catalog_entries SET source_urls=$2, checked_at=NOW()-INTERVAL '8 days', attempts=1 WHERE id=$1")
        .bind(id).bind(json!([format!("{base}/unavailable")])).execute(&pool).await.unwrap();
    let results = guides(
        &state,
        topic,
        Section::Culture,
        &mut Usage::default(),
        Instant::now() + Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(results[0]["deferred"], "资料暂不可读");
    let (checked, attempts, published): (bool, i16, bool) = sqlx::query_as(
        "SELECT checked_at > NOW()-INTERVAL '1 minute', attempts, guide IS NOT NULL FROM catalog_entries WHERE id=$1",
    ).bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!((checked, attempts, published), (true, 2, true));
    assert_eq!(calls.lock().unwrap().len(), before);
    // The briefing has no work to collect when only Culture is enabled.
    sqlx::query("UPDATE topics SET enabled=(id=ANY($1))")
        .bind(Section::Culture.topics())
        .execute(&pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
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
    state.digest.reload_topics(&state).await.unwrap();
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
            "/culture/search?topic=book".into(),
        ],
    )
    .await;
    // A preview links to the pages' own addresses.
    assert!(previews[0].contains(&format!("href=\"/culture/book/{slug}\"")));
    assert!(previews[2].contains("/culture/book/"));
    assert!(
        previews
            .iter()
            .all(|body| !body.contains(crate::reading::PREFIX))
    );
    // A section readers can't see at all has no search either, so an administrator is sent on.
    sqlx::query("UPDATE topics SET visible=false WHERE id=ANY($1)")
        .bind(Section::Culture.topics())
        .execute(&pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    assert_eq!(get("/culture/search?q=book".into()).await.status(), 404);
    let admin = crate::reading::tests::session(&state, "admin").await;
    let response = http
        .get(format!("{app_url}/culture/search?q=book"))
        .header("cookie", &admin)
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 307);
    assert_eq!(
        response.headers()["location"],
        format!("{}/culture/search?q=book", crate::reading::PREFIX)
    );
    sqlx::query("UPDATE topics SET visible=(id<>'book') WHERE id=ANY($1)")
        .bind(Section::Culture.topics())
        .execute(&pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    // An entry leads on to more by its creator, then more of its kind.
    sqlx::query("UPDATE topics SET visible=true WHERE id='book'")
        .execute(&pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    sqlx::query(
        "INSERT INTO catalog_entries (id, topic_id, key, slug, name, name_en, creator, kind, source_urls, image, guide, guide_at)
         SELECT gen_random_uuid(), topic_id, 'book:tidewater', 'tidewater-00000000', '潮水', 'Tidewater', creator, kind,
             source_urls, image, guide, guide_at FROM catalog_entries WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    // With two works, a run looks into both: the one with fewer items second.
    sqlx::query(
        "INSERT INTO catalog_items (item_id, entry_id, role)
         SELECT i.id, e.id, 'news' FROM items i, catalog_entries e
         WHERE i.url LIKE '%/unavailable' AND e.slug = 'tidewater-00000000'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        focus(&state, topic, window).await.unwrap(),
        "\n本轮深挖：海岸（The Shore，Ada）；潮水（Tidewater，Ada）"
    );
    // The shelf leads with the work whose latest story was published last, even when another
    // work's older story was collected later.
    sqlx::query(
        "UPDATE items SET published_at = NOW() - INTERVAL '20 minutes',
             first_seen_at = NOW() - INTERVAL '10 minutes'
         WHERE url LIKE '%/unavailable'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let page = get("/culture/book".into()).await.text().await.unwrap();
    let shelf = page
        .split_once("<section class=catalog-shelf>")
        .and_then(|(_, rest)| rest.split_once("</section>"))
        .unwrap()
        .0;
    assert!(shelf.find("tidewater-00000000").unwrap() < shelf.find(&slug).unwrap());
    let detail = get(format!("/culture/book/{slug}"))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        detail.contains("<section class=catalog-onward><h2>Ada 的更多作品</h2>")
            && detail.contains("href=\"/culture/book/tidewater-00000000\"")
            && !detail.contains("<h2>更多图书</h2>")
    );
    let english = get(format!("/culture/book/{slug}?lang=en"))
        .await
        .text()
        .await
        .unwrap();
    assert!(english.contains("<h2>More from Ada</h2>") && english.contains("<h2>Reviews</h2>"));
    // Drafts with items are looked into too when no work has a guide yet.
    sqlx::query("UPDATE catalog_entries SET guide = NULL, guide_at = NULL WHERE topic_id = 'book'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        focus(&state, topic, window).await.unwrap(),
        "\n本轮深挖：海岸（The Shore，Ada）；潮水（Tidewater，Ada）"
    );
    // Without its guide a work keeps its page, gathered from its stories, and its place on the
    // shelf; only the guide's sections are missing.
    let draft = get(format!("/culture/book/{slug}")).await;
    assert_eq!(draft.status(), 200);
    let draft = draft.text().await.unwrap();
    assert!(
        draft.contains("书评：《海岸》")
            && draft.contains("海岸：评测、用户分享与最新动态")
            && !draft.contains("catalog-sources")
    );
    let list = get("/culture/book".into()).await.text().await.unwrap();
    assert!(
        list.split_once("<section class=catalog-shelf>")
            .unwrap()
            .1
            .split_once("</section>")
            .unwrap()
            .0
            .contains(&format!("href=\"/culture/book/{slug}\""))
    );
    // Culture's reviews leave with the news, and so do entries without items.
    crate::runner::cleanup_items(&state, state.digest.today() + chrono::Duration::days(1))
        .await
        .unwrap();
    assert_eq!(prune(&state).await.unwrap(), 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM catalog_entries WHERE topic_id='book'")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    // 𝑮𝒂𝒅𝒈𝒆𝒕𝒔: a review and a reader's post join an official product, a rumor stays news.
    sqlx::query("UPDATE topics SET enabled=true, visible=true, brief = brief || ' culture.example' WHERE id=ANY($1)")
        .bind(Section::Gadgets.topics()).execute(&pool).await.unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    let eink: Topic = sqlx::query_as("SELECT * FROM topics WHERE id='eink'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((eink.interval_minutes, eink.max_items), (360, 12));
    let revision = || async {
        sqlx::query_scalar::<_, i64>("SELECT feed_revision FROM topics WHERE id='eink'")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let before = revision().await;
    let run = crate::runner::start(&state, &eink, "manual").await.unwrap();
    finished(&state, run).await;
    let (new, status): (i32, String) =
        sqlx::query_as("SELECT new_count, status FROM runs WHERE id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((new, status.as_str()), (3, "ok"));
    // The feed changes three times: the stories, their links to the product, and its guide; the
    // links alone tell readers' pages to reload, whether or not a guide follows.
    assert_eq!(revision().await, before + 3);
    let (key, sources, pictured, guide, slug): (String, Value, bool, Value, String) = sqlx::query_as(
        "SELECT key, source_urls, image->>'url' LIKE '%/files/cover.jpg', guide, slug FROM catalog_entries WHERE topic_id='eink'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(key, "amazon:kindlecolorsoft");
    assert_eq!(sources, json!([format!("{base}/products/colorsoft")]));
    assert!(
        pictured,
        "the official page's picture is the product's cover"
    );
    assert_eq!(
        guide["facts"].as_array().unwrap().len(),
        1,
        "a specification its quote does not support is dropped"
    );
    let roles: Vec<(String, String)> = sqlx::query_as(
        "SELECT i.title, c.role FROM catalog_items c JOIN items i ON i.id = c.item_id WHERE i.topic_id='eink' ORDER BY c.role",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        roles,
        [
            (
                "Kindle Colorsoft 一个月实测".to_owned(),
                "review".to_owned()
            ),
            ("彩色 Kindle 到手".to_owned(), "share".to_owned())
        ]
    );
    // The most talked-about shelf: products with at least two reviews and posts in 30 days, the
    // most first and the more recent among equals, shown once three have them. Colorsoft has a
    // review and a post from an hour ago; the others get stories from the fixture below and no
    // guide. Hidden stories, news and anything older than 30 days do not count.
    let add = |name: &'static str,
               n: usize,
               role: &'static str,
               age: &'static str,
               hidden: bool,
               picture: bool| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "WITH item AS (
                     INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url, published_at, hidden, image)
                     VALUES (gen_random_uuid(), 'eink', CURRENT_DATE, sha256(convert_to($1, 'UTF8')),
                         $1, $1, 'https://reviews.example/' || $1, NOW() - $3::interval, $4,
                         CASE WHEN $5 THEN jsonb_build_object('url', 'https://media.example/' || $1 || '.jpg') END)
                     RETURNING id)
                 INSERT INTO catalog_items (item_id, entry_id, role)
                 SELECT item.id, e.id, $2 FROM item, catalog_entries e WHERE e.slug = 'talked-' || $6",
            )
            .bind(format!("talked-{name}-{n}"))
            .bind(role)
            .bind(age)
            .bind(hidden)
            .bind(picture)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    let talked = || async {
        let page = get("/gadgets/eink".into()).await.text().await.unwrap();
        page.split_once("<section class=catalog-shelf><h2>最受关注</h2>")
            .map(|(_, rest)| rest.split_once("</section>").unwrap().0.to_owned())
    };
    assert!(talked().await.is_none());
    for name in ["one", "two", "three"] {
        sqlx::query(
            "INSERT INTO catalog_entries (id, topic_id, key, slug, name, name_en, creator, kind, source_urls)
             VALUES (gen_random_uuid(), 'eink', 'amazon:talked' || $1, 'talked-' || $1,
                 'Talked ' || $1, 'Talked ' || $1, 'Amazon', 'reader', '[]')",
        )
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    for n in 0..2 {
        add("one", n, "review", "1 day", false, false).await;
    }
    add("one", 2, "review", "1 day", true, false).await;
    add("one", 3, "review", "40 days", false, false).await;
    add("one", 4, "news", "1 day", false, false).await;
    // Two talked-about products are not yet a shelf; a third makes one.
    assert!(talked().await.is_none());
    for n in 0..3 {
        add("two", n, "review", "1 day", false, false).await;
    }
    let shelf = talked().await.unwrap();
    assert!(shelf.contains("2 条评测与分享") && shelf.contains("3 条评测与分享"));
    // Three's picture is its newest visible review's: not a newer post's, nor a hidden one's.
    for n in 0..3 {
        add("three", n, "review", "2 days", false, n == 1).await;
    }
    add("three", 3, "review", "1 day", false, false).await;
    add("three", 4, "share", "1 hour", false, true).await;
    add("three", 5, "review", "1 hour", true, true).await;
    let shelf = talked().await.unwrap();
    let at = |slug: &str| shelf.find(&format!("/gadgets/eink/{slug}\"")).unwrap();
    assert!(
        shelf.contains("5 条评测与分享")
            && at("talked-three") < at("talked-two")
            && at("talked-two") < at(&slug)
            && at(&slug) < at("talked-one")
    );
    // Two has no picture but an owner's photograph, which then pictures the product.
    add("two", 3, "share", "1 hour", false, true).await;
    assert!(
        get("/gadgets/eink/talked-two".into())
            .await
            .text()
            .await
            .unwrap()
            .contains("class=catalog-cover src=\"https://media.example/talked-two-3.jpg\"")
    );
    let page = get("/gadgets/eink".into()).await.text().await.unwrap();
    let recent = page
        .split_once("<section class=catalog-shelf><h2>近期有动态 ")
        .unwrap()
        .1;
    assert!(recent.contains("talked-one"));
    let unguided = get("/gadgets/eink/talked-three".into()).await;
    assert_eq!(unguided.status(), 200);
    let unguided = unguided.text().await.unwrap();
    assert!(
        unguided.contains("talked-three-0")
            && unguided
                .contains("class=catalog-cover src=\"https://media.example/talked-three-1.jpg\"")
            && !unguided.contains("catalog-specs")
    );
    for statement in [
        "DELETE FROM items WHERE url LIKE 'https://reviews.example/%'",
        "DELETE FROM catalog_entries WHERE slug LIKE 'talked-%'",
    ] {
        sqlx::query(statement).execute(&pool).await.unwrap();
    }
    assert!(
        page.contains("catalog-shelf")
            && page.contains("7 英寸，300 ppi")
            && page.contains("传闻：新款 Kindle Scribe")
            && page.contains("catalog-role")
    );
    assert_eq!(
        page.matches("<a class=tab href=\"/gadgets\" data-last=\"/gadgets/digital\" aria-current=\"true\">数码</a>")
            .count(),
        1
    );
    let panels = get("/gadgets/eink?kind=panel".into())
        .await
        .text()
        .await
        .unwrap();
    assert!(!panels.contains(&slug) && panels.contains("新故事正在路上"));
    let product = get(format!("/gadgets/eink/{slug}"))
        .await
        .text()
        .await
        .unwrap();
    assert!(
        product.contains("catalog-specs")
            && product.contains("真实测评")
            && product.contains("用户分享")
            && product.contains("官方资料")
    );
    // The weekly check brings the official pictures back when their words have not changed,
    // without asking the model; a product whose sources failed three times waits a week.
    let pictured = || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT image->>'url' LIKE '%/files/%' AND jsonb_array_length(images) > 0
             FROM catalog_entries WHERE topic_id = 'eink'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let check = || async {
        guides(
            &state,
            &eink,
            Section::Gadgets,
            &mut Usage::default(),
            Instant::now() + Duration::from_secs(120),
        )
        .await
        .unwrap()
    };
    sqlx::query(
        "UPDATE catalog_entries SET image = NULL, images = '[]', checked_at = NOW() - INTERVAL '8 days'
         WHERE topic_id = 'eink'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let before = calls.lock().unwrap().len();
    let results = check().await;
    assert_eq!(results[0]["unchanged"], true);
    assert!(pictured().await);
    assert_eq!(calls.lock().unwrap().len(), before);
    // A picture whose origin the page still shows keeps its stored copy: an upload would bring
    // a new expiry.
    let copy = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT image->>'expires_at' FROM catalog_entries WHERE topic_id = 'eink'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let kept = copy().await;
    sqlx::query(
        "UPDATE catalog_entries SET checked_at = NOW() - INTERVAL '8 days' WHERE topic_id = 'eink'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(check().await[0]["unchanged"], true);
    assert_eq!(copy().await, kept);
    // A new product whose sources failed three times waits a week, then is written.
    sqlx::query(
        "UPDATE catalog_entries SET guide = NULL, guide_at = NULL, source_hash = NULL, attempts = 3,
             checked_at = NOW() - INTERVAL '1 day' WHERE topic_id = 'eink'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(check().await.is_empty());
    sqlx::query(
        "UPDATE catalog_entries SET checked_at = NOW() - INTERVAL '8 days' WHERE topic_id = 'eink'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(check().await[0]["written"], true);
    let digest: Value = get("/digest?topic=eink".into()).await.json().await.unwrap();
    let stories = digest["topics"][0]["items"].as_array().unwrap();
    let review = stories
        .iter()
        .find(|s| s["url"].as_str().unwrap().ends_with("/review/colorsoft"))
        .unwrap();
    assert_eq!(review["catalog"]["role"], "review");
    assert_eq!(review["catalog"]["path"], format!("/gadgets/eink/{slug}"));
    let rumor = stories
        .iter()
        .find(|s| s["url"].as_str().unwrap().ends_with("/rumor/scribe"))
        .unwrap();
    assert!(rumor.get("catalog").is_none());
    // Reviews and shares stay with their product after the news retention passes.
    crate::runner::cleanup_items(&state, state.digest.today() + chrono::Duration::days(1))
        .await
        .unwrap();
    let kept: Vec<String> =
        sqlx::query_scalar("SELECT title FROM items WHERE topic_id='eink' ORDER BY title")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kept, ["Kindle Colorsoft 一个月实测", "彩色 Kindle 到手"]);
    assert_eq!(
        prune(&state).await.unwrap(),
        0,
        "a product with reviews stays"
    );
    // Once due, what the catalogue keeps is renewed whatever its day: the product's pictures
    // and an old review's photograph.
    sqlx::query(
        "UPDATE catalog_entries SET image = jsonb_set(image, '{expires_at}', to_jsonb(NOW() + INTERVAL '1 hour')),
             images = jsonb_build_array(jsonb_set(images->0, '{expires_at}', to_jsonb(NOW() + INTERVAL '1 hour')))
         WHERE topic_id = 'eink'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE items SET day = day - 30, image = jsonb_set(image, '{expires_at}', to_jsonb(NOW() + INTERVAL '1 hour'))
         WHERE url LIKE '%/review/colorsoft'",
    )
    .execute(&pool)
    .await
    .unwrap();
    crate::media::startup_sync(&state).await.unwrap();
    let renewed: Vec<bool> = sqlx::query_scalar(
        "SELECT (picture->>'expires_at')::timestamptz > NOW() + INTERVAL '2 days' FROM (
             SELECT image AS picture FROM catalog_entries WHERE topic_id = 'eink'
             UNION ALL SELECT images->0 FROM catalog_entries WHERE topic_id = 'eink'
             UNION ALL SELECT image FROM items WHERE url LIKE '%/review/colorsoft') pictures",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(renewed, [true, true, true]);
    // A product without official pages gets a search for them, the most talked-about first;
    // only the brand's or the brief's sites are kept, and one with none found waits a week.
    for name in ["one", "two", "three"] {
        sqlx::query(
            "INSERT INTO catalog_entries (id, topic_id, key, slug, name, name_en, creator, kind, source_urls)
             VALUES (gen_random_uuid(), 'eink', 'amazon:talked' || $1, 'talked-' || $1,
                 'Talked ' || $1, 'Talked ' || $1, 'Amazon', 'reader', '[]')",
        )
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
        add(name, 0, "review", "1 day", false, false).await;
    }
    let looked = guides(
        &state,
        &eink,
        Section::Gadgets,
        &mut Usage::default(),
        Instant::now() + Duration::from_secs(300),
    )
    .await
    .unwrap();
    assert_eq!(
        looked
            .iter()
            .filter(|outcome| outcome.get("official").is_some())
            .count(),
        3
    );
    let sources = |slug: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Value, bool)>(
                "SELECT source_urls, checked_at IS NOT NULL FROM catalog_entries WHERE slug = $1",
            )
            .bind(slug)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        sources("talked-one").await.0,
        json!([format!("{base}/products/talked-one")])
    );
    assert_eq!(sources("talked-two").await, (json!([]), true));
    let before = calls.lock().unwrap().len();
    guides(
        &state,
        &eink,
        Section::Gadgets,
        &mut Usage::default(),
        Instant::now() + Duration::from_secs(300),
    )
    .await
    .unwrap();
    assert!(
        calls.lock().unwrap()[before..]
            .iter()
            .all(|request| request["text"]["format"]["name"] != "official_pages")
    );
    // A story the run did not publish again still lends its product the official pages it cites,
    // and is not linked twice.
    let links = || async {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM catalog_items")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    let linked = links().await;
    attach(
        &state,
        &eink,
        Section::Gadgets,
        Uuid::new_v4(),
        &json!({"items": [{"url": "https://reviews.example/talked-three-again", "role": "review",
            "work": {"name": "Talked three", "name_en": "Talked three", "creator": "Amazon", "kind": "reader",
                "source_urls": [format!("{base}/products/talked-three"), "https://reseller.example/talked-three"]}}]}),
    )
    .await
    .unwrap();
    assert_eq!(
        sources("talked-three").await.0,
        json!([format!("{base}/products/talked-three")])
    );
    assert_eq!(links().await, linked);
    // The same page cited with `www.` or a trailing slash is not a new source, and gives the
    // guide no tries back; a page it lacks joins at the address cited and does.
    sqlx::query("UPDATE catalog_entries SET attempts = 2 WHERE slug = 'talked-three'")
        .execute(&pool)
        .await
        .unwrap();
    let lend = |urls: Vec<String>| {
        let (state, eink) = (state.clone(), eink.clone());
        async move {
            attach(&state, &eink, Section::Gadgets, Uuid::new_v4(),
                &json!({"items": [{"url": "https://reviews.example/talked-three-again", "role": "review",
                    "work": {"name": "Talked three", "name_en": "Talked three", "creator": "Amazon", "kind": "reader", "source_urls": urls}}]}))
                .await
                .unwrap();
            sqlx::query_as::<_, (Value, i16)>(
                "SELECT source_urls, attempts FROM catalog_entries WHERE slug = 'talked-three'",
            )
            .fetch_one(&state.pool)
            .await
            .unwrap()
        }
    };
    let www = base.replace("https://", "https://www.");
    assert_eq!(
        lend(vec![format!("{www}/products/talked-three/")]).await,
        (json!([format!("{base}/products/talked-three")]), 2)
    );
    assert_eq!(
        lend(vec![format!("{base}/products/talked-three/specs/")]).await,
        (
            json!([
                format!("{base}/products/talked-three"),
                format!("{base}/products/talked-three/specs/")
            ]),
            0
        )
    );
    // A new story about a product still without official pages has it looked for again, but not
    // twice in a day.
    let story = |n: usize| {
        let (state, eink) = (state.clone(), eink.clone());
        async move {
            let run = Uuid::new_v4();
            let url = format!("https://reviews.example/talked-two-again-{n}");
            sqlx::query("INSERT INTO runs (id, topic_id, triggered_by, status, model) VALUES ($1, 'eink', 'manual', 'ok', 'fixture')")
                .bind(run).execute(&state.pool).await.unwrap();
            sqlx::query(
                "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url, published_at, first_run_id)
                 VALUES (gen_random_uuid(), 'eink', CURRENT_DATE, sha256(convert_to($1, 'UTF8')), $1, $1, $1, NOW(), $2)",
            )
            .bind(&url).bind(run).execute(&state.pool).await.unwrap();
            attach(&state, &eink, Section::Gadgets, run, &json!({"items": [{"url": url, "role": "review",
                "work": {"name": "Talked two", "name_en": "Talked two", "creator": "Amazon", "kind": "reader", "source_urls": []}}]}))
                .await.unwrap();
            sources("talked-two").await.1
        }
    };
    assert!(story(0).await, "looked for today, so not again");
    sqlx::query("UPDATE catalog_entries SET checked_at = NOW() - INTERVAL '2 days' WHERE slug = 'talked-two'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !story(1).await,
        "a day later, a new story has it looked for again"
    );
    // The briefing collects nothing from 𝑮𝒂𝒅𝒈𝒆𝒕𝒔 either.
    let before = calls.lock().unwrap().len();
    sqlx::query("UPDATE topics SET enabled=(id=ANY($1))")
        .bind(Section::Gadgets.topics())
        .execute(&pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
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
    // A story collected within the week stays a repeat however many newer ones push it off the
    // list shown to the model, and even when its page is dated anew; an older one may return.
    sqlx::query("UPDATE topics SET enabled = true WHERE id = 'movie'")
        .execute(&pool)
        .await
        .unwrap();
    state.digest.reload_topics(&state).await.unwrap();
    let movie: Topic = sqlx::query_as("SELECT * FROM topics WHERE id = 'movie'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let runs = |run: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (i32, i32)>("SELECT new_count, dup_count FROM runs WHERE id = $1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let run = crate::runner::start(&state, &movie, "manual")
        .await
        .unwrap();
    finished(&state, run).await;
    assert_eq!(runs(run).await, (3, 1));
    let today = state.digest.today();
    sqlx::query("UPDATE items SET day = $1 WHERE topic_id = 'movie'")
        .bind(today - chrono::Duration::days(5))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET last_run_at = NULL WHERE id = 'movie'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO items (id, topic_id, day, fingerprint, title, summary, url)
         SELECT gen_random_uuid(), 'movie', $1, sha256(n::text::bytea), 'Filler ' || n, '', 'https://filler.example/' || n
         FROM generate_series(1, 80) n",
    )
    .bind(today)
    .execute(&pool)
    .await
    .unwrap();
    let run = crate::runner::start(&state, &movie, "manual")
        .await
        .unwrap();
    finished(&state, run).await;
    assert_eq!(runs(run).await, (0, 4));
    sqlx::query(
        "UPDATE items SET day = $1 WHERE topic_id = 'movie' AND url NOT LIKE 'https://filler.example/%'",
    )
    .bind(today - chrono::Duration::days(8))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE topics SET last_run_at = NULL WHERE id = 'movie'")
        .execute(&pool)
        .await
        .unwrap();
    let run = crate::runner::start(&state, &movie, "manual")
        .await
        .unwrap();
    finished(&state, run).await;
    assert_eq!(runs(run).await, (3, 1));
    server.abort();
    ai_server.abort();
    origin_server.abort();
}
