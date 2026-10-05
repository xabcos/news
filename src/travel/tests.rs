use super::*;
use chrono::{Duration, Utc};
use place::{fold, is_item, matched, slug};
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

fn window() -> NewsWindow {
    NewsWindow {
        start: Utc::now() - Duration::hours(24),
        end: Utc::now(),
    }
}

fn proposal(url: &str) -> NewItem {
    NewItem {
        fingerprint: vec![0; 32],
        title: "竹林道口的清晨".into(),
        summary: "来拍岚山小火车，到得太早，还去野宫神社参拜了。".into(),
        title_en: "Morning at the Bamboo Crossing".into(),
        summary_en: "Came to shoot the Arashiyama train and visited Nonomiya Shrine.".into(),
        url: url.into(),
        source: "Model".into(),
        published_at: None,
        tags: vec!["古迹".into()],
        tags_en: vec!["Heritage".into()],
        importance: 4,
        image: None,
    }
}

fn photo(key: &str, width: u32, height: u32) -> Value {
    json!({ "type": "photo", "media_url_https": format!("https://pbs.twimg.com/media/{key}.jpg"),
        "original_info": { "width": width, "height": height } })
}

#[test]
fn names_match_the_post_in_its_own_script() {
    let text = fold(
        "嵐山トロッコ列車撮影ですが嵯峨地区の竹林の踏切来てる。\n早く着きすぎて野宮神社参拝出来た。\n#嵯峨 #嵐山",
    );
    assert_eq!(matched(&text, "野宮神社"), Some(8));
    assert_eq!(matched(&text, "嵐山"), Some(4));
    assert_eq!(matched(&text, "佐賀県"), None);
    assert_eq!(matched(&text, "嵐"), None);

    // Latin names match whole words, accents aside; long ones may begin a hashtag.
    let text = fold("le canada pour notre voyage de noces 🍁\n\nMONTRÉAL <3 #KyotoTrip #HighLine");
    assert_eq!(matched(&text, "Montréal"), Some(8));
    assert_eq!(matched(&text, "Montreal"), Some(8));
    assert_eq!(matched(&text, "Kyoto"), Some(5));
    assert_eq!(matched(&text, "High Line"), Some(8));
    assert_eq!(matched(&text, "Canada"), Some(6));
    assert_eq!(matched(&text, "Can"), None);
    assert_eq!(matched(&text, "OU"), None);
    let text = fold("Il y a un an je partais en Road trip dans l’Ouest américain");
    assert_eq!(matched(&text, "Ouest américain"), Some(14));
    assert_eq!(matched(&text, "OU"), None);
    assert_eq!(matched(&text, "Road"), Some(4));
    // Kana keep their voicing marks.
    assert_eq!(matched(&fold("バンコクに来た"), "ハンコク"), None);
}

#[test]
fn destinations_are_addressed_by_their_guide() {
    assert_eq!(slug("Kyoto").as_deref(), Some("kyoto"));
    assert_eq!(slug("São Paulo").as_deref(), Some("sao-paulo"));
    assert_eq!(slug("Tokyo/Shibuya").as_deref(), Some("tokyo-shibuya"));
    assert_eq!(slug("Santiago (Chile)").as_deref(), Some("santiago-chile"));
    assert_eq!(slug("Łódź").as_deref(), Some("lodz"));
    assert_eq!(slug("北京"), None);
    assert_eq!(slug("…"), None);
    assert!(is_item("Q34600") && !is_item("Q034600") && !is_item("P17") && !is_item("Q"));
}

#[test]
fn regions_take_turns() {
    let at = |hours: i64| NewsWindow {
        start: Utc::now(),
        end: chrono::DateTime::from_timestamp(hours * 3600, 0).unwrap(),
    };
    let seen = (0..12)
        .step_by(2)
        .map(|hours| region(at(hours)))
        .collect::<HashSet<_>>();
    assert_eq!(seen.len(), REGIONS.len());
    assert_eq!(region(at(0)), region(at(1)));
    assert_eq!(region(at(0)), region(at(12)));
}

#[test]
fn reads_the_named_places_by_post() {
    let payload = json!({ "items": [
        { "url": "https://twitter.com/famcom103/status/2105447381550694617?s=20",
          "place": " 野宮神社 ", "place_en": "Nonomiya Shrine" },
        { "url": "https://x.com/x/status/2105447381550694618", "place": "", "place_en": "Kyoto" },
        { "url": "https://example.com/post", "place": "Paris", "place_en": "Paris" },
    ] });
    let named = named_places(&payload);
    assert_eq!(named.len(), 2);
    assert_eq!(
        named["2105447381550694617"],
        ["野宮神社", "Nonomiya Shrine"]
    );
    assert_eq!(named["2105447381550694618"], ["Kyoto"]);
}

#[test]
fn confirms_posts_with_their_photographs_in_order() {
    let created = Utc::now() - Duration::hours(2);
    let record = || {
        json!({ "created_at": created, "user": { "name": " ハッシー ", "screen_name": "famcom103" },
            "text": "早く着きすぎて野宮神社参拝出来た。 https://t.co/NbOYXwl3EA", "lang": "ja",
            "mediaDetails": [photo("One", 4000, 2256), photo("Small", 800, 600), photo("Two", 2256, 4000),
                photo("Three", 3000, 2000), photo("Four", 2000, 3000), photo("Five", 3000, 3000)] })
    };
    let post = |value: Value| serde_json::from_value::<Post>(value).unwrap();
    let mut item = proposal("https://twitter.com/FAMCOM103/status/2105447381550694617?s=20");
    let photos = confirm(&mut item, "2105447381550694617", &post(record()), window()).unwrap();
    assert_eq!(
        item.url,
        "https://x.com/famcom103/status/2105447381550694617"
    );
    assert_eq!(item.source, "ハッシー");
    assert_eq!(item.published_at, Some(created));
    assert_eq!(
        photos
            .iter()
            .map(|photo| photo.url.as_str())
            .collect::<Vec<_>>(),
        [
            "https://pbs.twimg.com/media/One?format=jpg&name=medium",
            "https://pbs.twimg.com/media/Two?format=jpg&name=medium",
            "https://pbs.twimg.com/media/Three?format=jpg&name=medium",
            "https://pbs.twimg.com/media/Four?format=jpg&name=medium",
        ]
    );
    let first = &photos[0];
    assert_eq!((first.width, first.height), (Some(4000), Some(2256)));
    assert_eq!(first.credit, "ハッシー @famcom103");
    assert_eq!(first.caption, "竹林道口的清晨");
    assert_eq!(first.source_url, item.url);
    assert_eq!(post(record()).words(), "早く着きすぎて野宮神社参拝出来た。");

    let refused = |change: fn(&mut Value)| {
        let mut value = record();
        change(&mut value);
        let mut item = proposal("https://x.com/famcom103/status/2105447381550694617");
        confirm(&mut item, "2105447381550694617", &post(value), window()).unwrap_err()
    };
    assert_eq!(
        refused(|value| value["possibly_sensitive"] = json!(true)),
        "X 标记为敏感内容"
    );
    assert_eq!(
        refused(|value| value["created_at"] = json!(Utc::now() - Duration::days(3))),
        "发布时间不在采集窗口内"
    );
    assert_eq!(
        refused(|value| value["mediaDetails"] = json!([photo("Small", 800, 600)])),
        "照片尺寸过小"
    );
    assert_eq!(
        refused(|value| value["mediaDetails"] = json!([])),
        "帖子没有照片"
    );
    assert_eq!(
        refused(|value| value["user"]["screen_name"] = json!("famcom/../i")),
        "X 帖子作者无效"
    );
}

fn destination(id: &str, country: &str) -> Destination {
    Destination {
        id: id.into(),
        slug: id.to_ascii_lowercase(),
        name: id.into(),
        name_en: id.into(),
        country_id: country.into(),
        country: country.into(),
        country_en: country.into(),
        latitude: 0.0,
        longitude: 0.0,
        voyage: id.into(),
        wiki_zh: None,
        wiki_en: None,
    }
}

#[test]
fn limits_count_destinations_by_day_and_countries_by_run() {
    let kyoto = destination("Q34600", "Q17");
    let mut per_destination = HashMap::from([("Q34600".to_owned(), DESTINATION_DAILY_LIMIT - 1)]);
    let mut per_country = HashMap::new();
    assert_eq!(
        admit(&mut per_destination, &mut per_country, &kyoto),
        Ok(())
    );
    assert_eq!(
        admit(&mut per_destination, &mut per_country, &kyoto),
        Err("同一目的地今日已满")
    );
    // A full destination does not count against its country.
    assert_eq!(per_country["Q17"], 1);
    per_country.insert("Q17".into(), COUNTRY_RUN_LIMIT - 1);
    let nara = destination("Q169134", "Q17");
    assert_eq!(admit(&mut per_destination, &mut per_country, &nara), Ok(()));
    assert_eq!(
        admit(&mut per_destination, &mut per_country, &nara),
        Err("同一国家本轮已满")
    );
    assert_eq!(
        admit(
            &mut per_destination,
            &mut per_country,
            &destination("Q90", "Q142")
        ),
        Ok(())
    );
}

#[test]
fn guides_keep_the_people_sights_and_dishes_the_sources_name() {
    let entry = |name: &str, name_en: &str| guide::Entry {
        name: name.into(),
        name_en: name_en.into(),
        text: format!(" {name}的故事 "),
        text_en: format!("The story of {name_en}."),
    };
    let draft = guide::Guide {
        tagline: " 千年古都 ".into(),
        tagline_en: "An old capital".into(),
        origin: "意为都城。".into(),
        origin_en: String::new(),
        history: "794 年建都。".into(),
        history_en: "Founded in 794.".into(),
        people: vec![
            entry("紫式部", "Murasaki Shikibu"),
            entry("无名氏", "Nobody"),
        ],
        highlights: (1..=5)
            .map(|n| entry(&format!("寺{n}"), &format!("Temple {n}")))
            .collect(),
        flavors: vec![guide::Entry {
            text_en: String::new(),
            ..entry("汤豆腐", "Yudofu")
        }],
        culture: String::new(),
        culture_en: String::new(),
        season: String::new(),
        season_en: String::new(),
        customs: String::new(),
        customs_en: String::new(),
        fact: String::new(),
        fact_en: String::new(),
        sources: Vec::new(),
    };
    let corpus = "MURASAKI SHIKIBU wrote here. 寺1 寺2 寺3 寺4 寺5 湯豆腐 汤豆腐";
    let tidy = guide::tidy(draft, corpus);
    assert_eq!(tidy.tagline, "千年古都");
    // A section written in one language only is left out.
    assert_eq!((tidy.origin.as_str(), tidy.origin_en.as_str()), ("", ""));
    assert_eq!(
        tidy.people
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["紫式部"]
    );
    assert_eq!(tidy.people[0].text, "紫式部的故事");
    assert_eq!(tidy.highlights.len(), 3);
    assert!(tidy.flavors.is_empty());
}

#[test]
fn guides_read_history_and_culture_sections() {
    let page = "Kyoto was the capital of Japan for over a millennium.\n\n== Districts ==\nList of districts.\n\n== Understand ==\nIntro to the city.\n\n=== History ===\nFounded in 794 as Heian-kyō.\n\n=== Climate ===\nHot summers.\n\n== Get in ==\nBy train.\n\n=== By plane ===\nKansai airport.\n\n== Eat ==\nKyoto cuisine.\n";
    let kept = guide::sections(page, &["understand", "eat"]);
    assert_eq!(
        kept,
        "Kyoto was the capital of Japan for over a millennium.\nUnderstand\nIntro to the city.\nHistory\nFounded in 794 as Heian-kyō.\nClimate\nHot summers.\nEat\nKyoto cuisine."
    );
    assert!(!kept.contains("Kansai") && !kept.contains("districts"));
}

#[test]
fn people_stories_have_no_length_limit_but_their_numbers_are_checked() {
    let story = |text: &str, text_en: &str| guide::Guide {
        tagline: "千年古都".into(),
        tagline_en: "An old capital".into(),
        history: "794 年建都。".into(),
        history_en: "Founded in 794.".into(),
        people: vec![guide::Entry {
            name: "紫式部".into(),
            name_en: "Murasaki Shikibu".into(),
            text: text.into(),
            text_en: text_en.into(),
        }],
        ..Default::default()
    };
    let corpus = "Founded in 794. Murasaki Shikibu served at the court around 1008.";
    let long = story(
        &"紫式部在宫中侍奉，约 1008 年写成《源氏物语》。".repeat(12),
        &"Murasaki Shikibu served at the court and finished her tale around 1008. ".repeat(12),
    );
    assert!(guide::verify(&long, corpus).is_ok());
    let invented = story(
        "紫式部 1001 年入宫。",
        "Murasaki Shikibu joined the court in 1001.",
    );
    assert!(matches!(
        guide::verify(&invented, corpus),
        Err(guide::Problem::Numbers(numbers)) if numbers == ["1001"]
    ));
}

#[test]
fn guide_numbers_must_come_from_the_sources() {
    let corpus = "Founded in 794 as Heian-kyō. Population 1,467,856 in 2020; 3.5 km of canals. 東京は１８６８年から首都。";
    assert_eq!(
        guide::numbers("794 年迁都，1,467,856 人，3.5 公里，19th century"),
        ["794", "1467856", "3.5", "19"]
    );
    let corpus = corpus.replace(',', "").nfkc().collect::<String>();
    for number in ["794", "1467856", "2020", "3.5", "1868"] {
        assert!(guide::has_number(&corpus, number), "{number}");
    }
    for number in ["79", "146", "3", "5", "186"] {
        assert!(!guide::has_number(&corpus, number), "{number}");
    }
    // Months and small counts are often spelled out, so they pass; larger numbers do not.
    let words = "the nineteen temples were rebuilt in june".to_owned();
    for (number, cited) in [
        ("6", true),
        ("12", true),
        ("19", true),
        ("18", false),
        ("794", true),
        ("795", false),
    ] {
        assert_eq!(guide::cited(&corpus, &words, number), cited, "{number}");
    }
}

/// The world a run reaches: X and its photographs, Wikidata, the wikis and the file server.
/// A CONNECT proxy answers for each host, so the clients use the real names on port 443.
async fn world(
    posts: HashMap<String, Value>,
    entities: HashMap<String, Value>,
    hits: HashMap<String, Vec<String>>,
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

    let hosts = [
        "cdn.syndication.twimg.com",
        "pbs.twimg.com",
        "www.wikidata.org",
        "en.wikivoyage.org",
        "zh.wikipedia.org",
        "en.wikipedia.org",
        "media.example.test",
    ];
    let certified = rcgen::generate_simple_self_signed(hosts.map(str::to_owned).to_vec()).unwrap();
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
            let host = connect
                .strip_prefix("CONNECT ")
                .and_then(|rest| rest.split(':').next())
                .unwrap()
                .to_owned();
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
            let query = target
                .split_once('?')
                .map(|(_, query)| {
                    url::form_urlencoded::parse(query.as_bytes())
                        .into_owned()
                        .collect::<HashMap<_, _>>()
                })
                .unwrap_or_default();
            let json =
                |value: Value| ("200 OK", "application/json", value.to_string().into_bytes());
            let (status, kind, reply) = match host.as_str() {
                "cdn.syndication.twimg.com" => {
                    assert_eq!(query["token"], x::embed_token(&query["id"]));
                    match posts.get(&query["id"]) {
                        Some(post) => json(post.clone()),
                        None => json(json!({ "__typename": "TweetTombstone", "tombstone": {} })),
                    }
                }
                "pbs.twimg.com" => {
                    assert!(target.ends_with("?format=jpg&name=medium"), "{target}");
                    ("200 OK", "image/jpeg", photo.clone())
                }
                "media.example.test" => {
                    assert_eq!(target, "/upload");
                    json(json!({
                        "direct_link": format!("https://media.example.test/files/{upload}.jpg"),
                        "expires_at": Utc::now() + Duration::hours(36),
                    }))
                }
                "www.wikidata.org" => match query["action"].as_str() {
                    "wbsearchentities" => json(json!({ "search": hits
                        .get(&query["search"])
                        .into_iter()
                        .flatten()
                        .map(|id| json!({ "id": id }))
                        .collect::<Vec<_>>() })),
                    _ => json(json!({ "entities": query["ids"]
                        .split('|')
                        .map(|id| (id.to_owned(), entities.get(id).cloned()
                            .unwrap_or_else(|| json!({ "id": id, "missing": "" }))))
                        .collect::<serde_json::Map<_, _>>() })),
                },
                _ if query["titles"] == "Unreachable" => (
                    "503 Service Unavailable",
                    "application/json",
                    b"{}".to_vec(),
                ),
                _ => {
                    // Every wiki page tells the same short story.
                    assert_eq!(query["prop"], "extracts");
                    assert_eq!(query.contains_key("variant"), host == "zh.wikipedia.org");
                    json(
                        json!({ "query": { "pages": [{ "title": query["titles"], "extract":
                        "A city of temples, gardens and old streets beside the hills, admired by travelers for centuries.\n\n\
                         == Understand ==\nThe old capital rewards slow walks: lanes of wooden houses, moss gardens and quiet shrines.\n\n\
                         == See ==\nThe Golden Pavilion stands over its pond, and the temples on the hills look over the city.\n\n\
                         == History ==\nFounded in 794 as a capital, it kept the court until 1868 and its craftsmen kept working through every age. The poet Ono no Komachi lived here.\n\n\
                         == 历史 ==\n这座城市于794年建都，直到1868年一直是朝廷所在地，工匠的手艺代代相传。\n\n\
                         == Getting in ==\nTrains run every 10 minutes.\n\n\
                         == Culture ==\nTea houses serving matcha, festivals in summer and autumn leaves draw visitors from the whole country, and the old crafts of weaving and pottery live on." }] } }),
                    )
                }
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

/// A Wikidata item with the statements a lookup reads.
fn item(
    id: &str,
    labels: &[(&str, &str)],
    voyage: Option<&str>,
    coordinates: Option<(f64, f64)>,
    located_in: Option<&str>,
    country: Option<&str>,
) -> (String, Value) {
    let statement = |value: Value| json!([{ "mainsnak": { "datavalue": { "value": value } }, "rank": "normal" }]);
    let mut claims = serde_json::Map::new();
    if let Some((latitude, longitude)) = coordinates {
        claims.insert(
            "P625".into(),
            statement(json!({ "latitude": latitude, "longitude": longitude,
            "globe": "http://www.wikidata.org/entity/Q2" })),
        );
    }
    if let Some(parent) = located_in {
        claims.insert("P131".into(), statement(json!({ "id": parent })));
    }
    if let Some(country) = country {
        claims.insert("P17".into(), statement(json!({ "id": country })));
    }
    let mut sitelinks = serde_json::Map::new();
    if let Some(title) = voyage {
        sitelinks.insert("enwikivoyage".into(), json!({ "title": title }));
        sitelinks.insert("enwiki".into(), json!({ "title": title }));
        sitelinks.insert("zhwiki".into(), json!({ "title": labels[0].1 }));
    }
    let entity = json!({
        "id": id,
        "labels": labels.iter().map(|(language, value)| (language.to_string(), json!({ "value": value })))
            .collect::<serde_json::Map<_, _>>(),
        "sitelinks": sitelinks,
        "claims": claims,
    });
    (id.to_owned(), entity)
}

#[tokio::test]
#[ignore = "requires NEWS_TRAVEL_TEST_DATABASE_URL and local test configuration"]
async fn postgres_travel_keeps_moments_the_post_and_wikidata_confirm() {
    use axum::{Json, Router, routing::post};
    use std::sync::Mutex;
    let database_url =
        std::env::var("NEWS_TRAVEL_TEST_DATABASE_URL").expect("disposable database URL");
    let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(database.starts_with("news_travel_test"));
    // A database left by an earlier run starts over; a fresh one has no tables yet.
    for statement in [
        "DELETE FROM runs WHERE topic_id = 'travel'",
        "DELETE FROM items WHERE topic_id = 'travel'",
        "DELETE FROM travel_places",
        "UPDATE topics SET enabled = true, last_run_at = NULL, last_status = NULL WHERE owner_user_id IS NULL",
    ] {
        sqlx::query(statement).execute(&pool).await.ok();
    }
    let now = Utc::now();
    let id = |n: u64| (2_105_000_000_000_000_000_u64 + n).to_string();
    let record = |account: &str, text: &str, lang: &str, photos: Value| {
        json!({ "__typename": "Tweet", "created_at": now - Duration::hours(2), "text": text, "lang": lang,
            "user": { "name": account.to_uppercase(), "screen_name": account }, "mediaDetails": photos })
    };
    // (post, account, words, language, place, place in English)
    let proposals = [
        (
            1,
            "alice",
            "早く着きすぎて野宮神社参拝出来た。#嵐山 https://t.co/a",
            "ja",
            "野宮神社",
            "Nonomiya Shrine, Kyoto",
        ),
        (2, "bob", "京都の紅葉が始まった", "ja", "京都", "Kyoto"),
        (3, "carol", "京都駅から歩いて", "ja", "京都", "Kyoto"),
        (4, "dave", "京都で抹茶", "ja", "京都", "Kyoto"),
        (5, "erin", "京都の夜", "ja", "京都", "Kyoto"),
        (6, "frank", "北海道の朝", "ja", "北海道", "Hokkaido"),
        (7, "frank", "北海道の湖", "ja", "北海道", "Hokkaido"),
        (8, "gina", "北海道のラーメン", "ja", "北海道", "Hokkaido"),
        (9, "hana", "北海道の牧場", "ja", "北海道", "Hokkaido"),
        (10, "ivy", "奈良の鹿", "ja", "奈良", "Nara"),
        (
            11,
            "jack",
            "Sunset walk on the High Line",
            "en",
            "High Line",
            "High Line, New York",
        ),
        (12, "kate", "Paris at night", "en", "Paris", "Paris"),
        (13, "liam", "Lovely morning walk", "en", "Kyoto", "Kyoto"),
        (
            14,
            "mali",
            "ชอบวิวที่นี้ กรุงเทพท้องฟ้าสดใส",
            "th",
            "กรุงเทพ",
            "Bangkok",
        ),
        (15, "nora", "Tokyo tower", "en", "東京", "Tokyo"),
    ];
    let mut posts = HashMap::new();
    for (n, account, text, lang, ..) in proposals {
        if n == 15 {
            continue;
        }
        let photos = if n == 1 {
            json!([
                photo("AliceOne", 4000, 2256),
                photo("Small", 640, 480),
                photo("AliceTwo", 2256, 4000)
            ])
        } else {
            json!([photo(&format!("Photo{n}"), 3000, 2000)])
        };
        let mut post = record(account, text, lang, photos);
        if n == 12 {
            post["possibly_sensitive"] = json!(true);
        }
        // Hokkaido's posts are the most liked, then Bangkok's, then Kyoto's; X gives the High
        // Line's no count.
        let likes = match n {
            6..=9 => Some(900 + n),
            14 => Some(500),
            11 => None,
            _ => Some(100 + n),
        };
        if let Some(likes) = likes {
            post["favorite_count"] = json!(likes);
        }
        posts.insert(id(n), post);
    }
    let entities = HashMap::from([
        item(
            "Q501266",
            &[
                ("zh-cn", "野宫神社"),
                ("ja", "野宮神社"),
                ("en", "Nonomiya Shrine"),
            ],
            None,
            Some((35.0177, 135.6743)),
            Some("Q580186"),
            Some("Q17"),
        ),
        item(
            "Q580186",
            &[("zh-cn", "右京区"), ("en", "Ukyō-ku")],
            None,
            Some((35.0, 135.7)),
            Some("Q34600"),
            Some("Q17"),
        ),
        item(
            "Q34600",
            &[("zh-cn", "京都市"), ("ja", "京都"), ("en", "Kyoto")],
            Some("Kyoto"),
            Some((35.0116, 135.7681)),
            None,
            Some("Q17"),
        ),
        item(
            "Q35581",
            &[("zh-cn", "北海道"), ("en", "Hokkaido")],
            Some("Hokkaido"),
            Some((43.0, 142.0)),
            None,
            Some("Q17"),
        ),
        item(
            "Q169134",
            &[("zh-cn", "奈良市"), ("ja", "奈良"), ("en", "Nara")],
            Some("Nara"),
            Some((34.68, 135.8)),
            None,
            Some("Q17"),
        ),
        item(
            "Q17",
            &[("zh-cn", "日本"), ("en", "Japan")],
            Some("Japan"),
            None,
            None,
            None,
        ),
        item(
            "Q843869",
            &[("zh-cn", "高线公园"), ("en", "High Line")],
            None,
            Some((40.748, -74.0048)),
            Some("Q11299"),
            Some("Q30"),
        ),
        item(
            "Q11299",
            &[("zh-cn", "曼哈顿"), ("en", "Manhattan")],
            Some("Manhattan"),
            Some((40.78, -73.97)),
            None,
            Some("Q30"),
        ),
        item(
            "Q30",
            &[("zh-cn", "美国"), ("en", "United States")],
            Some("United States"),
            None,
            None,
            None,
        ),
        item(
            "Q1861",
            &[("zh-cn", "曼谷"), ("th", "กรุงเทพมหานคร"), ("en", "Bangkok")],
            Some("Bangkok"),
            Some((13.75, 100.5)),
            None,
            Some("Q869"),
        ),
        item(
            "Q869",
            &[("zh-cn", "泰国"), ("en", "Thailand")],
            Some("Thailand"),
            None,
            None,
            None,
        ),
        item(
            "Q90",
            &[("zh-cn", "巴黎"), ("en", "Paris")],
            Some("Paris"),
            Some((48.85, 2.35)),
            None,
            Some("Q142"),
        ),
    ]);
    let hits = [
        ("野宮神社", "Q501266"),
        ("京都", "Q34600"),
        ("Kyoto", "Q34600"),
        ("北海道", "Q35581"),
        ("Hokkaido", "Q35581"),
        ("奈良", "Q169134"),
        ("High Line", "Q843869"),
        ("Paris", "Q90"),
        ("กรุงเทพ", "Q1861"),
        ("Bangkok", "Q1861"),
    ]
    .into_iter()
    .map(|(name, id)| (name.to_owned(), vec![id.to_owned()]))
    .collect::<HashMap<_, _>>();
    let model = proposals
        .iter()
        .map(|(n, account, _, _, place, place_en)| {
            json!({ "title": format!("旅途{n}"), "title_en": format!("Journey {n}"),
                "summary": "旅途中的风景。", "summary_en": "A view from the road.",
                "url": format!("https://x.com/{account}/status/{}", id(*n)), "source": "Model",
                "published_at": now - Duration::hours(1), "tags": ["风景"], "tags_en": ["Scenery"],
                "importance": 4, "place": place, "place_en": place_en })
        })
        .collect::<Vec<_>>();
    // The guide names one person and one sight the sources never mention; both are dropped.
    let entry = |name: &str, name_en: &str, text: &str, text_en: &str| json!({ "name": name, "name_en": name_en, "text": text, "text_en": text_en });
    let guide = move |history: &str| {
        json!({ "tagline": "千年古都，庭园与老街", "tagline_en": "An old capital of gardens and lanes",
            "origin": "名字的意思是都城。", "origin_en": "The name means capital.",
            "history": history, "history_en": "Founded in 794, it kept the court until 1868.",
            "people": [entry("小野小町", "Ono no Komachi", "诗人小野小町曾住在这里。", "The poet Ono no Komachi lived here."),
                entry("无名氏", "Nobody Real", "资料里没有这个人。", "The sources never mention this person.")],
            "highlights": [entry("金阁", "Golden Pavilion", "池边的金色楼阁。", "A golden hall over a pond."),
                entry("想象之塔", "Imaginary Tower", "并不存在。", "It does not exist.")],
            "flavors": [entry("抹茶", "matcha", "茶屋里的抹茶。", "Matcha in the tea houses.")],
            "culture": "茶屋、夏秋的节庆与红叶吸引着各地旅人。", "culture_en": "Tea houses, festivals and autumn leaves.",
            "season": "", "season_en": "",
            "customs": "进茶屋前先脱鞋。", "customs_en": "Take off your shoes before entering a tea house.",
            "fact": "这里的工匠代代相传。", "fact_en": "Its crafts have passed down through every age." })
    };
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let seen = Arc::clone(&requests);
    let ai = Router::new().route(
        "/v1/responses",
        post(move |Json(request): Json<Value>| {
            let (model, seen) = (model.clone(), Arc::clone(&seen));
            async move {
                seen.lock().unwrap().push(request.clone());
                let input = request["input"].as_str().unwrap();
                let payload = if request["text"]["format"]["name"] == "travel_guide" {
                    assert!(request.get("tools").is_none());
                    assert_eq!(request["reasoning"]["effort"], "medium");
                    assert!(input.contains("Founded in 794"));
                    if input.contains("目的地：曼哈顿") {
                        guide("建于 3000 年前。")
                    } else if input.contains("目的地：北海道") && !input.contains("2099") {
                        guide("2099 年开拓。")
                    } else {
                        guide("794 年建都。\n1868 年以前一直是都城。")
                    }
                } else {
                    assert_eq!(request["tools"], json!([{ "type": "x_search", "enable_image_understanding": true }]));
                    assert!(request["instructions"].as_str().unwrap().contains("「𝑻𝒓𝒂𝒗𝒆𝒍」"));
                    assert!(input.contains("本轮也留意："));
                    assert!(request["text"]["format"]["schema"].to_string().contains("place_en"));
                    json!({ "items": model, "cover": null })
                };
                Json(json!({ "status": "completed", "id": "travel-fixture",
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
    let (certificate, proxy, origin_server) = world(posts, entities, hits).await;
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

    let topic: Topic = sqlx::query_as("SELECT * FROM topics WHERE id = 'travel'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert!(is_travel(&topic) && topic.enabled && topic.visible);
    assert_eq!(topic.name, "𝑻𝒓𝒂𝒗𝒆𝒍");
    assert_eq!(
        (
            topic.interval_minutes,
            topic.max_items,
            topic.lookback_hours,
            topic.sort_weight
        ),
        (120, 30, 48, 16)
    );
    let catalogue = state
        .digest
        .topics()
        .iter()
        .map(|topic| topic.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(&catalogue[..4], ["x", "p", "w", "travel"]);

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
    let payload = payload.unwrap();
    assert_eq!((new, dropped), (10, 5), "{payload}");
    let reasons = payload["skipped"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skip| {
            (
                skip["url"]
                    .as_str()
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_owned(),
                skip["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        [
            (id(9), "同一国家本轮已满".to_owned()),
            (id(10), "同一国家本轮已满".to_owned()),
            (id(12), "X 标记为敏感内容".to_owned()),
            (id(13), "正文没有写出可核实的地点".to_owned()),
            (id(15), "X 帖子无法读取".to_owned()),
        ]
    );
    let guides = payload["guides"].as_array().unwrap();
    assert_eq!(guides.len(), 4, "{guides:?}");
    for place in ["Q34600", "Q35581", "Q1861"] {
        assert!(
            guides
                .iter()
                .any(|guide| guide["place"] == place && guide["written"] == true),
            "{place}: {guides:?}"
        );
    }
    assert!(guides.iter().any(
        |guide| guide["place"] == "Q11299" && guide["error"] == "导读中的数字与资料不符：3000"
    ));
    // Hokkaido's first draft cited a year the sources lack and was written again.
    let corrected = requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| {
            request["input"]
                .as_str()
                .unwrap()
                .contains("目的地：北海道")
        })
        .map(|request| request["input"].as_str().unwrap().contains("找不到：2099"))
        .collect::<Vec<_>>();
    assert_eq!(corrected, [false, true]);

    let places = sqlx::query_as::<_, (String, String, String, String, String, bool, i16)>(
        "SELECT id, slug, name, country, country_id, guide IS NOT NULL, guide_attempts FROM travel_places ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap();
    let place = |id: &str, slug: &str, name: &str, country: &str, country_id: &str, guide: bool| {
        (
            id.to_owned(),
            slug.to_owned(),
            name.to_owned(),
            country.to_owned(),
            country_id.to_owned(),
            guide,
            1_i16,
        )
    };
    assert_eq!(
        places,
        [
            place("Q11299", "manhattan", "曼哈顿", "美国", "Q30", false),
            place("Q1861", "bangkok", "曼谷", "泰国", "Q869", true),
            place("Q34600", "kyoto", "京都市", "日本", "Q17", true),
            place("Q35581", "hokkaido", "北海道", "日本", "Q17", true),
        ]
    );
    let (spot_id, spot, destination, photos, text, lang): (
        String,
        String,
        String,
        Value,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT m.spot_id, m.spot, m.place_id, m.extra_photos, m.text, m.lang FROM travel_moments m
             JOIN items i ON i.id = m.item_id WHERE i.url LIKE '%/alice/%'",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        (spot_id.as_str(), spot.as_str(), destination.as_str()),
        ("Q501266", "野宫神社", "Q34600")
    );
    assert_eq!(
        (text.as_str(), lang.as_str()),
        ("早く着きすぎて野宮神社参拝出来た。#嵐山", "ja")
    );
    // The cover is the item's image; the moment keeps the photograph after it.
    let photos = photos.as_array().unwrap();
    assert_eq!(photos.len(), 1);
    assert!(
        photos[0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://media.example.test/files/")
    );
    assert_eq!(
        photos[0]["origin_url"],
        "https://pbs.twimg.com/media/AliceTwo?format=jpg&name=medium"
    );
    let guide: Value = sqlx::query_scalar("SELECT guide FROM travel_places WHERE id = 'Q34600'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let names = |field: &str| {
        guide[field]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name_en"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names("people"), ["Ono no Komachi"]);
    assert_eq!(names("highlights"), ["Golden Pavilion"]);
    assert_eq!(names("flavors"), ["matcha"]);
    assert_eq!(guide["origin"], "名字的意思是都城。");
    assert_eq!(guide["sources"].as_array().unwrap().len(), 3);

    let today = state.digest.today();
    let snapshot = state
        .digest
        .snapshot(&state, TOPIC_ID, today)
        .await
        .unwrap();
    assert_eq!(snapshot.items.len(), 10);
    let alice = snapshot
        .items
        .iter()
        .find(|item| item.url.contains("/alice/"))
        .unwrap();
    assert_eq!(alice.url, format!("https://x.com/alice/status/{}", id(1)));
    assert_eq!(alice.source, "ALICE");
    let travel = alice.travel.as_ref().unwrap();
    assert_eq!(travel["photos"].as_array().unwrap().len(), 2);
    assert!(
        travel["photos"][0].get("origin_url").is_none()
            && travel["photos"][0].get("expires_at").is_none()
    );
    assert_eq!(travel["photos"][0]["width"], 4000);
    assert_eq!(travel["spot"], "野宫神社");
    assert_eq!(travel["place"]["slug"], "kyoto");
    assert_eq!(travel["place"]["country"], "日本");
    assert_eq!(
        (&travel["place"]["latitude"], &travel["place"]["longitude"]),
        (&json!(35.01), &json!(135.77))
    );
    assert_eq!(travel["place"]["tagline"], "千年古都，庭园与老街");
    assert_eq!(travel["place"]["fact"], "这里的工匠代代相传。");
    assert_eq!(
        alice.image.as_ref().unwrap()["url"],
        travel["photos"][0]["url"]
    );
    let bangkok = snapshot
        .items
        .iter()
        .find(|item| item.url.contains("/mali/"))
        .unwrap();
    assert_eq!(bangkok.travel.as_ref().unwrap()["place"]["name"], "曼谷");
    // A moment keeps the likes its post had when X confirmed it.
    let likes: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT i.source, m.likes FROM travel_moments m JOIN items i ON i.id = m.item_id
         WHERE i.source IN ('ALICE', 'JACK') ORDER BY i.source",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert_eq!(likes, [("ALICE".into(), Some(101)), ("JACK".into(), None)]);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = crate::web::router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let get = |path: &str| http.get(format!("{base}{path}")).send();
    let page = get("/travel/kyoto").await.unwrap();
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    for text in [
        "千年古都，庭园与老街",
        "<p class=destination-fact>这里的工匠代代相传。</p>",
        "地名由来",
        "<dd><p>794 年建都。</p><p>1868 年以前一直是都城。</p></dd>",
        "<b>小野小町</b>",
        "<b>金阁</b>",
        "<b>抹茶</b>",
        "入乡随俗",
        "旅人瞬间 <span>5</span>",
        "data-source=\"/travel/kyoto/moments\"",
        "TouristDestination",
        "<section class=destination-moments id=photos>",
        "id=moments>",
        "35.01°N 135.77°E",
        "维基导游 · Kyoto",
        "rel=\"license noopener noreferrer\">CC BY-SA 4.0</a>",
        "<section class=destination-nearby><h2>附近</h2><div class=nearby>",
        "<a class=nearby-card href=\"/travel/hokkaido\"><img src=\"https://media.example.test/",
        "<span class=nearby-name>北海道</span><span class=nearby-meta>日本 · ",
    ] {
        assert!(html.contains(text), "missing {text}");
    }
    // The nearest destination comes first; Japan has no other destination to list.
    let at = |slug: &str| html.find(&format!("href=\"/travel/{slug}\"")).unwrap();
    assert!(at("hokkaido") < at("bangkok") && at("bangkok") < at("manhattan"));
    for text in [
        "无名氏",
        "想象之塔",
        "data-next",
        "时节与节庆",
        "你知道吗",
        "依据资料整理",
        "destination-more",
        "href=\"/travel/kyoto\"",
        // Without portraits there is nothing to filter.
        "moment-kinds",
    ] {
        assert!(!html.contains(text), "unexpected {text}");
    }
    // Older moments load a page at a time after the newest one.
    let (time, newest): (chrono::DateTime<Utc>, uuid::Uuid) = sqlx::query_as(
        "SELECT i.first_seen_at, i.id FROM items i JOIN travel_moments m ON m.item_id = i.id
         WHERE m.place_id = 'Q34600' ORDER BY i.first_seen_at DESC, i.id DESC LIMIT 1",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap();
    let page: Value = get(&format!(
        "/travel/kyoto/moments?before={}_{newest}",
        time.timestamp_micros()
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let previews = crate::reading::tests::pages(
        &state,
        &base,
        "travel",
        &[
            "/travel/kyoto".into(),
            "/travel/map.json".into(),
            "/travel/kyoto/guide.json".into(),
            format!(
                "/travel/kyoto/moments?before={}_{newest}",
                time.timestamp_micros()
            ),
        ],
    )
    .await;
    assert!(previews[0].contains("data-source=\"/api/admin/preview/travel/kyoto/moments\""));
    assert_eq!(page["moments"].as_array().unwrap().len(), 4);
    assert!(page["next"].is_null());
    assert!(page["moments"][0]["travel"]["photos"].is_array());
    assert_eq!(
        get("/travel/kyoto/moments?before=soon")
            .await
            .unwrap()
            .status(),
        400
    );
    // Two of Kyoto's five moments are mainly of people: the page offers them on their own, and
    // the filter carries through the next pages. All of Bangkok's are, so it offers no choice.
    sqlx::query(
        "UPDATE items SET tags = ARRAY['人像'] || tags WHERE id IN (
             SELECT m.item_id FROM travel_moments m JOIN items i ON i.id = m.item_id
             WHERE m.place_id = 'Q34600' ORDER BY i.first_seen_at DESC, i.id DESC LIMIT 2)
         OR id IN (SELECT m.item_id FROM travel_moments m JOIN travel_places p ON p.id = m.place_id
             WHERE p.slug = 'bangkok')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let text = |path: &'static str| async move { get(path).await.unwrap().text().await.unwrap() };
    assert!(text("/travel/kyoto").await.contains(
        "旅人瞬间 <span>5</span></h2><nav class=moment-kinds><a href=\"/travel/kyoto#photos\" aria-current=\"true\">全部</a><a href=\"/travel/kyoto?kind=portraits#photos\">人像 2</a></nav>"
    ));
    let portraits = text("/travel/kyoto?kind=portraits").await;
    assert!(
        portraits.contains(
            "<a href=\"/travel/kyoto?kind=portraits#photos\" aria-current=\"true\">人像 2</a>"
        ) && portraits.contains("data-source=\"/travel/kyoto/moments?kind=portraits\"")
    );
    let data = portraits
        .split_once("id=moments>")
        .and_then(|(_, rest)| rest.split_once("</script>"))
        .unwrap()
        .0;
    let shown: Vec<Value> = serde_json::from_str(data).unwrap();
    assert!(shown.len() == 2 && shown.iter().all(|moment| moment["tags"][0] == "人像"));
    let newest = shown[0]["id"].as_str().unwrap();
    let time = chrono::DateTime::parse_from_rfc3339(shown[0]["first_seen_at"].as_str().unwrap())
        .unwrap()
        .timestamp_micros();
    let page: Value = get(&format!(
        "/travel/kyoto/moments?kind=portraits&before={time}_{newest}"
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    // Unfiltered, the four older moments would follow; filtered, only the other portrait does.
    assert_eq!(page["moments"].as_array().unwrap().len(), 1);
    assert_eq!(page["moments"][0]["id"], shown[1]["id"]);
    assert!(
        text("/travel/kyoto?kind=else")
            .await
            .contains("<a href=\"/travel/kyoto#photos\" aria-current=\"true\">全部</a>")
    );
    let bangkok = text("/travel/bangkok?kind=portraits").await;
    assert!(bangkok.contains("曼谷") && !bangkok.contains("moment-kinds"));
    let english = get("/travel/kyoto?lang=en")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(english.contains("An old capital of gardens and lanes") && english.contains("Japan"));
    assert!(
        english.contains("<h2>Nearby</h2>")
            && english.contains("href=\"/travel/hokkaido?lang=en\"")
            && english.contains(
                "<a href=\"/travel/kyoto?kind=portraits&amp;lang=en#photos\">Portraits 2</a>"
            )
    );
    let manhattan = get("/travel/manhattan")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(manhattan.contains("曼哈顿") && !manhattan.contains("destination-guide"));
    assert_eq!(get("/travel/nowhere").await.unwrap().status(), 404);
    let redirect = get("/travel?lang=en").await.unwrap();
    assert_eq!(redirect.status(), 307);
    assert_eq!(redirect.headers()["location"], "/?topic=travel&lang=en");
    let map: Value = get("/travel/map.json").await.unwrap().json().await.unwrap();
    let mut slugs = map["places"]
        .as_array()
        .unwrap()
        .iter()
        .map(|place| place["slug"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    slugs.sort();
    assert_eq!(slugs, ["bangkok", "hokkaido", "kyoto", "manhattan"]);
    // Beside the map: the most liked moment of each destination, the most liked first, and a
    // moment without a count last.
    let featured = |map: &Value| {
        map["featured"]
            .as_array()
            .unwrap()
            .iter()
            .map(|moment| moment["slug"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        featured(&map),
        ["hokkaido", "bangkok", "kyoto", "manhattan"]
    );
    // Hokkaido has several moments; its most liked one stands for it.
    let hokkaido: Vec<(i64, String)> = sqlx::query_as(
        "SELECT m.likes, i.image->>'url' FROM travel_moments m JOIN items i ON i.id = m.item_id
         JOIN travel_places p ON p.id = m.place_id WHERE p.slug = 'hokkaido' ORDER BY m.likes DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert!(hokkaido.len() > 1 && hokkaido[0].0 > hokkaido[1].0);
    assert_eq!(map["featured"][0]["likes"], hokkaido[0].0);
    assert_eq!(map["featured"][0]["photo"]["url"], hokkaido[0].1);
    assert!(map["featured"][3]["likes"].is_null());
    assert!(
        map["featured"][0]["photo"]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://media.example.test/")
            && map["featured"][0]["photo"].get("origin_url").is_none()
    );
    // The viewer reads a destination's guide without its sources.
    let guide = get("/travel/kyoto/guide.json").await.unwrap();
    assert_eq!(guide.status(), 200);
    let guide: Value = guide.json().await.unwrap();
    assert_eq!(guide["guide"]["tagline"], "千年古都，庭园与老街");
    assert_eq!(guide["guide"]["highlights"][0]["name"], "金阁");
    assert!(guide["guide"].get("sources").is_none());
    for path in ["/travel/manhattan/guide.json", "/travel/nowhere/guide.json"] {
        assert_eq!(get(path).await.unwrap().status(), 404, "{path}");
    }
    assert!(
        map["places"]
            .as_array()
            .unwrap()
            .iter()
            .all(|place| place["recent"] == true)
    );
    // Later runs deepen two destinations that have a guide: one much visited, one with few moments.
    let deepened = |focus: &str| {
        focus
            .split("\n本轮深挖：")
            .nth(1)
            .map_or(0, |places| places.split('；').count())
    };
    let first = focus(&state, window()).await.unwrap();
    assert_eq!(deepened(&first), 2, "{first}");
    let focus = first;
    assert!(
        focus.starts_with("\n本轮也留意：") && focus.contains("\n本轮深挖："),
        "{focus}"
    );
    assert!(
        ["京都", "北海道", "曼谷"]
            .iter()
            .any(|name| focus.contains(name)),
        "{focus}"
    );
    assert!(!focus.contains("曼哈顿"), "{focus}");
    let index = get("/sitemap.xml").await.unwrap().text().await.unwrap();
    assert!(index.contains("/travel/sitemap.xml"));
    let sitemap = get("/travel/sitemap.xml")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(sitemap.contains("/travel/kyoto</loc>") && sitemap.contains("/travel/kyoto?lang=en"));
    assert!(!sitemap.contains("manhattan"));

    // A guide that keeps failing is tried again in later runs, up to three times; one whose
    // pages cannot be read waits for a later run without using up its attempts.
    sqlx::query(
        "INSERT INTO travel_places (id, slug, name, name_en, country_id, country, country_en,
             latitude, longitude, voyage) VALUES ('Q404', 'unreachable', '远方', 'Faraway', 'Q404',
             '远方', 'Faraway', 0, 0, 'Unreachable')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let mut usage = crate::fetch::Usage::default();
    let deadline = Instant::now() + std::time::Duration::from_secs(600);
    for _ in 0..3 {
        let outcomes = guide::write(&state, &topic, &[], &mut usage, deadline)
            .await
            .unwrap();
        assert!(
            outcomes
                .iter()
                .any(|outcome| outcome["place"] == "Q404" && outcome["skipped"] == "资料暂时读不到"),
            "{outcomes:?}"
        );
    }
    let attempts: i16 =
        sqlx::query_scalar("SELECT guide_attempts FROM travel_places WHERE id = 'Q404'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(attempts, 0);
    let attempts: i16 =
        sqlx::query_scalar("SELECT guide_attempts FROM travel_places WHERE id = 'Q11299'")
            .fetch_one(&state.pool)
            .await
            .unwrap();
    assert_eq!(attempts, 3);

    // The map and the onward links name only destinations with a page: a place with only a guide
    // joins both, one whose moments are all hidden leaves them, and old moments fade its pin.
    sqlx::query(
        "INSERT INTO travel_places (id, slug, name, name_en, country_id, country, country_en,
             latitude, longitude, voyage, guide, guide_at)
         SELECT 'Q169134', 'nara', '奈良市', 'Nara', country_id, country, country_en, 34.68, 135.8,
             'Nara', guide, guide_at FROM travel_places WHERE slug = 'kyoto'",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE items SET hidden = true WHERE id IN (SELECT m.item_id FROM travel_moments m
         JOIN travel_places p ON p.id = m.place_id WHERE p.slug = 'manhattan')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE items SET day = day - 10 WHERE id IN (SELECT m.item_id FROM travel_moments m
         JOIN travel_places p ON p.id = m.place_id WHERE p.slug = 'bangkok')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let map: Value = get("/travel/map.json").await.unwrap().json().await.unwrap();
    let pins = map["places"]
        .as_array()
        .unwrap()
        .iter()
        .map(|place| {
            (
                place["slug"].as_str().unwrap(),
                place["recent"].as_bool().unwrap(),
            )
        })
        .collect::<HashMap<_, _>>();
    assert_eq!(
        pins,
        HashMap::from([
            ("kyoto", true),
            ("hokkaido", true),
            ("bangkok", false),
            ("nara", false)
        ])
    );
    // Favorites come from the last seven days' visible moments only.
    assert_eq!(featured(&map), ["hokkaido", "kyoto"]);
    let html = get("/travel/kyoto").await.unwrap().text().await.unwrap();
    assert!(html.contains("href=\"/travel/bangkok\"") && !html.contains("manhattan"));
    assert!(html.contains(
        "<p class=destination-more><span>日本的更多目的地</span><a href=\"/travel/nara\">奈良市</a></p>"
    ));
    assert_eq!(get("/travel/manhattan").await.unwrap().status(), 404);
    // The destination with the fewest moments is always one of the two; Nara has none.
    let focus = super::focus(&state, window()).await.unwrap();
    assert_eq!(deepened(&focus), 2, "{focus}");
    assert!(focus.contains("奈良市（Nara，日本）"), "{focus}");

    // The briefing neither collects nor reads travel moments.
    sqlx::query("UPDATE topics SET enabled = (id = 'travel')")
        .execute(&state.pool)
        .await
        .unwrap();
    crate::runner::collect_briefing(&state, window(), None)
        .await
        .unwrap();
    let runs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE topic_id = 'travel'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(runs, 1);

    server.abort();
    ai_server.abort();
    origin_server.abort();
}
