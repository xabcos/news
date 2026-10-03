use super::{PREFIX, Reader};
use crate::state::AppState;
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[test]
fn preview_links_keep_the_home_feed_and_language() {
    assert_eq!(
        Reader::Admin.localized("/?topic=w", true),
        "/?topic=w&lang=en"
    );
    assert_eq!(
        Reader::Admin.localized("/reads?page=2", true),
        format!("{PREFIX}/reads?page=2&lang=en")
    );
    assert_eq!(Reader::Public.path("/culture/book"), "/culture/book");
}

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
    // The original bookmarked page resolves to the authenticated preview, without caching the redirect.
    let response = http
        .get(format!("{base}{}", paths[0]))
        .header("cookie", &admin)
        .header("accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 307);
    assert_eq!(
        response.headers()["location"],
        format!("{PREFIX}{}", paths[0])
    );
    assert_eq!(response.headers()["cache-control"], "private, no-store");
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
    sqlx::query("INSERT INTO topics(id,name,brief,owner_user_id) SELECT 'preview-personal','Personal','Test',id FROM users WHERE role='user' ORDER BY id LIMIT 1")
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
        sqlx::query("INSERT INTO items(id,topic_id,day,fingerprint,title,summary,url,hidden) VALUES($1,$2,$3,$4,$5,'Test',$6,$7)")
            .bind(Uuid::new_v4()).bind(topic).bind(day).bind(Sha256::digest([n]).to_vec())
            .bind(format!("Preview {n}")).bind(format!("https://example.test/preview/{n}")).bind(hidden)
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

    let days = preview("/days?topic=w")
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(days["days"].as_array().unwrap().len(), 2);
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
                assert_ne!(item["title"], "Preview 28");
            }
            after = group["next"].as_str().map(str::to_owned);
            if after.is_none() {
                break;
            }
        }
    }
    assert_eq!(ids.len(), 27);
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
    sqlx::query("DELETE FROM sessions")
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(preview("/digest?topic=w").await.unwrap().status(), 401);
    server.abort();
}
