use super::PREFIX;
use crate::state::AppState;
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) async fn session(state: &AppState, role: &str) -> String {
    let token = Uuid::new_v4().to_string();
    let user: i64 = sqlx::query_scalar("INSERT INTO users(email,role) VALUES($1,$2) RETURNING id")
        .bind(format!("{token}@preview.example.test"))
        .bind(role)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at) VALUES($1,$2,NOW()+INTERVAL '1 day')",
    )
    .bind(Sha256::digest(token.as_bytes()).to_vec())
    .bind(user)
    .execute(&state.pool)
    .await
    .unwrap();
    format!(
        "{}={token}; news_role={role}",
        state.config.session_cookie_name
    )
}

// Called from each section's database fixture while its real list/detail data is present.
pub(crate) async fn pages(
    state: &AppState,
    base: &str,
    topic: &str,
    paths: &[String],
) -> Vec<String> {
    let http = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let admin = session(state, "admin").await;
    let user = session(state, "user").await;
    let previous: (bool, bool) = sqlx::query_as("SELECT enabled,visible FROM topics WHERE id=$1")
        .bind(topic)
        .fetch_one(&state.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE topics SET enabled=true,visible=false WHERE id=$1")
        .bind(topic)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(state).await.unwrap();
    let mut bodies = Vec::new();
    for path in paths {
        assert_eq!(
            http.get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            404,
            "{path}"
        );
        for (cookie, status) in [("news_role=admin", 401), (user.as_str(), 403)] {
            let response = http
                .get(format!("{base}{PREFIX}{path}"))
                .header("cookie", cookie)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{path}");
            assert_eq!(response.headers()["cache-control"], "private, no-store");
        }
        let response = http
            .get(format!("{base}{PREFIX}{path}"))
            .header("cookie", &admin)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
        assert!(
            !response
                .headers()
                .contains_key("access-control-allow-origin")
        );
        bodies.push(response.text().await.unwrap());
    }
    // A page readers can't see sends an administrator on to its preview, without caching the redirect.
    let page = || {
        http.get(format!("{base}{}", paths[0]))
            .header("cookie", &admin)
            .header("accept", "text/html")
            .send()
    };
    let response = page().await.unwrap();
    assert_eq!(response.status(), 307);
    assert_eq!(
        response.headers()["location"],
        format!("{PREFIX}{}", paths[0])
    );
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    // A script is not sent on: it reads the preview itself (`PublicPage.readPath`).
    let response = http
        .get(format!("{base}{}", paths[0]))
        .header("cookie", &admin)
        .header("accept", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    // Once readers can see it, the administrator reads it at its own address, as they do.
    sqlx::query("UPDATE topics SET visible=true WHERE id=$1")
        .bind(topic)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(state).await.unwrap();
    let response = page().await.unwrap();
    assert_eq!(response.status(), 200, "{}", paths[0]);
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .starts_with("public,")
    );
    sqlx::query("UPDATE topics SET enabled=false WHERE id=$1")
        .bind(topic)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(state).await.unwrap();
    for path in paths {
        assert_eq!(
            http.get(format!("{base}{PREFIX}{path}"))
                .header("cookie", &admin)
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    sqlx::query("UPDATE topics SET enabled=$2,visible=$3 WHERE id=$1")
        .bind(topic)
        .bind(previous.0)
        .bind(previous.1)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(state).await.unwrap();
    bodies
}

// Runs at the end of the gallery fixture, using the same isolated database.
pub(crate) async fn feed(state: &AppState) {
    let admin = session(state, "admin").await;
    let user = session(state, "user").await;
    sqlx::query(
        "INSERT INTO topics(id,name,brief,visible) VALUES('preview-shared','Shared','Test',true)",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    // The reader just signed in owns it.
    sqlx::query("INSERT INTO topics(id,name,brief,owner_user_id) SELECT 'preview-personal','Personal','Test',id FROM users WHERE role='user' ORDER BY id DESC LIMIT 1")
        .execute(&state.pool).await.unwrap();
    sqlx::query("UPDATE topics SET visible=false WHERE id='w'")
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(state).await.unwrap();
    sqlx::query("DELETE FROM items WHERE topic_id='w'")
        .execute(&state.pool)
        .await
        .unwrap();
    let today = state.digest.today();
    // Each photograph arrives after the one before but was posted earlier, so pages only line
    // up when they follow 𝑾's order.
    for n in 0..29 {
        let (topic, day, hidden) = if n == 0 {
            ("preview-shared", today, false)
        } else {
            (
                "w",
                if n <= 25 {
                    today
                } else {
                    today.pred_opt().unwrap()
                },
                n == 28,
            )
        };
        sqlx::query("INSERT INTO items(id,topic_id,day,fingerprint,title,summary,url,hidden,published_at) VALUES($1,$2,$3,$4,$5,'Test',$6,$7,NOW()-make_interval(hours => $8))")
            .bind(Uuid::new_v4()).bind(topic).bind(day).bind(Sha256::digest([n]).to_vec())
            .bind(format!("Preview {n}")).bind(format!("https://example.test/preview/{n}")).bind(hidden)
            .bind(i32::from(n))
            .execute(&state.pool).await.unwrap();
    }
    state.digest.reload_topics(state).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = crate::web::router(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = Client::builder().no_proxy().build().unwrap();
    let preview = |path: &str| {
        http.get(format!("{base}{PREFIX}{path}"))
            .header("cookie", &admin)
            .send()
    };
    for path in ["/digest?topic=w", "/topics", "/days?topic=w"] {
        for (cookie, status) in [("", 401), ("news_role=admin", 401), (user.as_str(), 403)] {
            assert_eq!(
                http.get(format!("{base}{PREFIX}{path}"))
                    .header("cookie", cookie)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                status
            );
        }
    }
    // A private response fills the very same snapshots that the public handler then reads.
    let response = preview("/digest?topic=preview-shared").await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );
    let warmed = response.json::<Value>().await.unwrap();
    // A change made behind the cache's back shows that the public read reuses the snapshot.
    sqlx::query(
        "UPDATE items SET title='Changed behind the cache' WHERE topic_id='preview-shared'",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let response = http
        .get(format!("{base}/digest?topic=preview-shared"))
        .send()
        .await
        .unwrap();
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .starts_with("public,")
    );
    assert_eq!(response.json::<Value>().await.unwrap(), warmed);

    // The preview counts and shows 𝑾's photograph that waits for a ❤️, marked as waiting.
    let days = preview("/days?topic=w")
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(days["days"].as_array().unwrap().len(), 2);
    assert_eq!(days["days"][1]["count"], 3);
    let mut ids = std::collections::HashSet::new();
    for day in days["days"].as_array().unwrap() {
        let mut after = None;
        loop {
            let mut url = format!(
                "/digest?topic=w&day={}&limit=12",
                day["day"].as_str().unwrap()
            );
            if let Some(cursor) = after {
                url.push_str(&format!("&after={cursor}"));
            }
            let data = preview(&url).await.unwrap().json::<Value>().await.unwrap();
            let group = &data["topics"][0];
            for item in group["items"].as_array().unwrap() {
                assert!(ids.insert(item["id"].as_str().unwrap().to_owned()));
                assert_eq!(item["hidden"] == true, item["title"] == "Preview 28");
            }
            after = group["next"].as_str().map(str::to_owned);
            if after.is_none() {
                break;
            }
        }
    }
    assert_eq!(ids.len(), 28);
    sqlx::query("UPDATE items SET title='Updated private photo' WHERE topic_id='w' AND day=$1")
        .bind(today)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.refresh(state, "w", today).await.unwrap();
    let response = preview("/digest?topic=w").await.unwrap();
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("Updated private photo")
    );
    // Neither warming nor updating a hidden topic grants public or personal-feed access.
    for path in ["/digest?topic=w", "/feed.xml?topic=w"] {
        assert_eq!(
            http.get(format!("{base}{path}"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
    assert_eq!(
        http.get(format!("{base}/api/me/feed?topic=w"))
            .header("cookie", &user)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    let public_days = http
        .get(format!("{base}/days?topic=w"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(public_days["days"].as_array().unwrap().is_empty());
    let public = http
        .get(format!("{base}/topics"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(
        !public["topics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "w")
    );
    let private = preview("/topics")
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(
        private["topics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "w")
    );
    assert!(
        !private["topics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "preview-personal")
    );
    assert_eq!(
        preview("/digest?topic=preview-personal")
            .await
            .unwrap()
            .status(),
        404
    );
    // Everyone gets the same news page, with every page's bar in the address's language: a
    // hidden topic has no tab, and an administrator opens it by its address.
    let home = |path: &str, cookie: &str| {
        http.get(format!("{base}{path}"))
            .header("cookie", cookie)
            .send()
    };
    let response = home("/?topic=w", "").await.unwrap();
    assert!(
        response.headers()["cache-control"]
            .to_str()
            .unwrap()
            .starts_with("public,")
    );
    let page = response.text().await.unwrap();
    for cookie in [admin.as_str(), user.as_str()] {
        assert_eq!(
            home("/?topic=w", cookie)
                .await
                .unwrap()
                .text()
                .await
                .unwrap(),
            page
        );
    }
    for part in [
        "<html lang=\"zh-CN\">",
        "<nav class=\"tabs\" aria-label=\"主题\"><a class=tab href=\"/\">𝐀𝐋𝐋</a>",
        "<a class=tab href=\"/?topic=preview-shared\">Shared</a>",
        "href=\"/?topic=w&amp;lang=en\" hreflang=\"en\">EN</a>",
        "aria-label=\"回到顶部\"",
    ] {
        assert!(page.contains(part), "{part}");
    }
    for absent in [
        "topic=w\"",
        "preview-personal",
        "aria-current",
        "rel=\"preload\"",
    ] {
        assert!(!page.contains(absent), "{absent}");
    }
    let english = home("/?topic=preview-shared&lang=en", "")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for part in [
        "<html lang=\"en\">",
        "<nav class=\"tabs\" aria-label=\"Topics\">",
        "<a class=tab href=\"/?topic=preview-shared&amp;lang=en\" aria-current=\"true\">Shared</a>",
        ">Workspace</a><a class=\"space-link button button-quiet button-small\" href=\"/?topic=preview-shared\" hreflang=\"zh-CN\">中文</a>",
        "aria-label=\"Back to top\"",
        "<link rel=\"preload\" href=\"/digest?topic=preview-shared&amp;limit=12\" as=\"fetch\" crossorigin fetchpriority=\"high\">",
    ] {
        assert!(english.contains(part), "{part}");
    }
    // The switch keeps the rest of the address; a dated one preloads nothing; the share card is
    // the configured site's.
    let dated = home("/?day=2026-10-20&lang=en", "")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(dated.contains("href=\"/?day=2026-10-20\" hreflang=\"zh-CN\">中文</a>"));
    assert!(!dated.contains("rel=\"preload\""));
    let site = state.config.public_base_url.as_str().trim_end_matches('/');
    for card in [
        format!("<meta property=\"og:url\" content=\"{site}/\">"),
        format!("<meta property=\"og:image\" content=\"{site}/og.png\">"),
    ] {
        assert!(page.contains(&card), "{card}");
    }
    // The page's validator holds until the catalogue changes (checked after 𝑾 is shown below).
    let etag = home("/", "").await.unwrap().headers()["etag"].clone();
    let revalidate = || {
        http.get(format!("{base}/"))
            .header("if-none-match", &etag)
            .send()
    };
    assert_eq!(revalidate().await.unwrap().status(), 304);
    // An administrator's preview of a page draws the public tabs as well.
    let previewed = preview("/articles").await.unwrap().text().await.unwrap();
    assert!(previewed.contains("<a class=tab href=\"/articles\" aria-current=\"true\">"));
    assert!(!previewed.contains("topic=w\""));
    // A signed-in reader's own topic comes marked as theirs, for the tab the page adds.
    let mine = http
        .get(format!("{base}/api/me/feed?limit=1"))
        .header("cookie", &user)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let own = mine["topics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|topic| topic["own"] == true)
        .map(|topic| topic["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(own, ["preview-personal"]);
    // A visibility change takes effect with the same invalidation used by the workspace.
    sqlx::query("UPDATE topics SET visible=true WHERE id='w'")
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(state).await.unwrap();
    assert_eq!(
        http.get(format!("{base}/digest?topic=w"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let shown = revalidate().await.unwrap();
    assert_eq!(shown.status(), 200);
    assert!(
        shown
            .text()
            .await
            .unwrap()
            .contains("<a class=tab href=\"/?topic=w\">")
    );
    // A logged-in reader's pages follow 𝑾's order too.
    let (mut seen, mut after) = (std::collections::HashSet::new(), String::new());
    loop {
        let data = http
            .get(format!(
                "{base}/api/me/feed?topic=w&day={today}&limit=12{after}"
            ))
            .header("cookie", &user)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        let group = &data["digest"]["topics"][0];
        for item in group["items"].as_array().unwrap() {
            assert!(seen.insert(item["id"].as_str().unwrap().to_owned()));
        }
        let Some(next) = group["next"].as_str() else {
            break;
        };
        after = format!("&after={next}");
    }
    assert_eq!(seen.len(), 25);
    // Readers don't see the waiting photograph until an administrator gives it a ❤️.
    let yesterday = today.pred_opt().unwrap();
    let public = |path: String| http.get(format!("{base}{path}")).send();
    let photos = public(format!("/digest?topic=w&day={yesterday}"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(photos.contains("Preview 27") && !photos.contains("Preview 28"));
    let counts = public("/days?topic=w".into())
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(counts["days"][1]["count"], 2);
    let waiting: Uuid = sqlx::query_scalar("SELECT id FROM items WHERE title='Preview 28'")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    let mark = |hidden: bool| {
        http.patch(format!("{base}/api/admin/items/{waiting}"))
            .header("cookie", &admin)
            .header("sec-fetch-site", "same-origin")
            .json(&serde_json::json!({ "hidden": hidden }))
            .send()
    };
    // A ❤️ publishes it in the feed and the Atom feed; a second click withdraws it again.
    for hidden in [false, true] {
        assert_eq!(mark(hidden).await.unwrap().status(), 200);
        for path in [
            format!("/digest?topic=w&day={yesterday}"),
            "/feed.xml?topic=w".into(),
        ] {
            let body = public(path).await.unwrap().text().await.unwrap();
            assert_eq!(body.contains("Preview 28"), !hidden);
        }
    }
    // With today's photographs all hidden, readers fall back to yesterday while the
    // administrator stays on today.
    sqlx::query("UPDATE items SET hidden=true WHERE topic_id='w' AND day=$1")
        .bind(today)
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.refresh(state, "w", today).await.unwrap();
    let w = |topics: Value| {
        topics["topics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|topic| topic["id"] == "w")
            .unwrap()
            .clone()
    };
    let readers = w(public("/topics".into())
        .await
        .unwrap()
        .json()
        .await
        .unwrap());
    let admins = w(preview("/topics").await.unwrap().json().await.unwrap());
    assert_eq!(
        (&readers["today_count"], &readers["latest_day"]),
        (&0.into(), &yesterday.to_string().into())
    );
    assert_eq!(
        (&admins["today_count"], &admins["latest_day"]),
        (&25.into(), &today.to_string().into())
    );
    // Older days come from PostgreSQL, answered apart for readers and the administrator.
    sqlx::query("DELETE FROM items WHERE topic_id='w' AND day >= $1")
        .bind(yesterday)
        .execute(&state.pool)
        .await
        .unwrap();
    for (n, ago, hidden) in [(30_u8, 3, false), (31, 2, true)] {
        sqlx::query("INSERT INTO items(id,topic_id,day,fingerprint,title,summary,url,hidden) VALUES($1,'w',$2,$3,$4,'Test',$5,$6)")
            .bind(Uuid::new_v4()).bind(today - chrono::Duration::days(ago)).bind(Sha256::digest([n]).to_vec())
            .bind(format!("Preview {n}")).bind(format!("https://example.test/preview/{n}")).bind(hidden)
            .execute(&state.pool).await.unwrap();
    }
    for day in [today, yesterday] {
        state.digest.refresh(state, "w", day).await.unwrap();
    }
    let readers = public("/digest?topic=w".into())
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let admins = preview("/digest?topic=w")
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        readers["day"],
        (today - chrono::Duration::days(3)).to_string()
    );
    assert_eq!(
        admins["day"],
        (today - chrono::Duration::days(2)).to_string()
    );
    sqlx::query("UPDATE topics SET visible=false WHERE id='w'")
        .execute(&state.pool)
        .await
        .unwrap();
    state.digest.reload_topics(state).await.unwrap();
    assert_eq!(
        http.get(format!("{base}/digest?topic=w"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(preview("/digest?topic=w").await.unwrap().status(), 200);
    // The workspace's two switches, in all four combinations, decide what every reader sees.
    for (enabled, visible) in [(true, false), (false, true), (false, false), (true, true)] {
        let response = http
            .patch(format!("{base}/api/admin/topics/preview-shared"))
            .header("cookie", &admin)
            .header("sec-fetch-site", "same-origin")
            .json(&serde_json::json!({ "enabled": enabled, "visible": visible }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.json::<Value>().await.unwrap()["visible"], visible);
        let shown = enabled && visible;
        for cookie in ["", admin.as_str(), user.as_str()] {
            let get = |path: &str| {
                http.get(format!("{base}{path}"))
                    .header("cookie", cookie)
                    .send()
            };
            for path in ["/topics", "/digest?limit=128"] {
                let listed = get(path).await.unwrap().json::<Value>().await.unwrap();
                let listed = listed["topics"].as_array().unwrap();
                assert_eq!(
                    listed.iter().any(|topic| topic["id"] == "preview-shared"),
                    shown,
                    "{path}"
                );
            }
            for path in [
                "/digest?topic=preview-shared",
                "/feed.xml?topic=preview-shared",
            ] {
                let status = get(path).await.unwrap().status();
                assert_eq!(status.is_success(), shown, "{path}");
            }
        }
        let days = public("/days?topic=preview-shared".into())
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(!days["days"].as_array().unwrap().is_empty(), shown);
        let private = http
            .get(format!("{base}/api/me/feed?topic=preview-shared"))
            .header("cookie", &user)
            .send()
            .await
            .unwrap();
        assert_eq!(private.status().is_success(), shown);
    }
    sqlx::query("DELETE FROM sessions")
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(preview("/digest?topic=w").await.unwrap().status(), 401);
    server.abort();
}
