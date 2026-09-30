use crate::{
    error::{AppError, Result},
    state::AppState,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};
use tokio::time::MissedTickBehavior;
use uuid::Uuid;

pub(crate) const TRACK_URL: &str =
    "https://stats.xairouter.com/track/pk-c994c1ce749548c8b45c55d21fdfc4cf";

#[derive(sqlx::FromRow)]
struct Reading {
    id: Uuid,
    url: String,
    first_seen_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct PageViews {
    url: String,
    visits: i64,
}

#[derive(Deserialize)]
struct PageBatch {
    pages: Vec<PageViews>,
}

pub(crate) async fn refresh_loop(state: AppState) {
    let endpoint = format!("{}/pages", TRACK_URL.replace("/track/", "/stats/"));
    let mut interval = tokio::time::interval(Duration::from_secs(64 * 60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            _ = interval.tick() => {}
        }
        tokio::select! {
            _ = state.shutdown.cancelled() => break,
            result = refresh(&state, &endpoint) => {
                if let Err(error) = result {
                    tracing::warn!(%error, "Reads statistics refresh failed; retaining cached counts and ranks");
                }
            }
        }
    }
}

pub(super) async fn refresh(state: &AppState, endpoint: &str) -> Result<()> {
    let rows: Vec<Reading> = sqlx::query_as(
        "SELECT i.id, i.url, i.first_seen_at FROM items i
         JOIN topics t ON t.id = i.topic_id JOIN read_articles a ON a.item_id = i.id
         WHERE i.topic_id = 'reads' AND t.enabled AND t.owner_user_id IS NULL
           AND NOT i.hidden AND i.first_seen_at >= NOW() - INTERVAL '30 days'
         ORDER BY i.first_seen_at DESC, i.id DESC LIMIT 1500",
    )
    .fetch_all(&state.pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let base = state.config.public_base_url.as_str().trim_end_matches('/');
    let mut counts = HashMap::new();
    for chunk in rows.chunks(100) {
        let urls: Vec<_> = chunk
            .iter()
            .map(|row| format!("{base}/reads/{}", row.id))
            .collect();
        let batch: PageBatch = state
            .http
            .post(endpoint)
            .timeout(Duration::from_secs(10))
            .json(&serde_json::json!({"urls": urls}))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let pages: HashMap<_, _> = batch
            .pages
            .into_iter()
            .map(|page| (page.url, page.visits))
            .collect();
        for (row, url) in chunk.iter().zip(&urls) {
            let visits = pages
                .get(url)
                .copied()
                .filter(|visits| *visits >= 0)
                .ok_or_else(|| AppError::Internal("Stats 返回的页面统计不完整".into()))?;
            counts.insert(row.id, visits);
        }
        // Stay below the public endpoint's ten requests/second limit.
        if rows.len() > 100 {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    let ranks = rank(&rows, &counts, Utc::now());
    let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
    let views: Vec<_> = ids.iter().map(|id| counts[id]).collect();
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE read_articles a SET views = v.views FROM unnest($1::uuid[], $2::bigint[]) AS v(id, views) WHERE a.item_id = v.id")
        .bind(ids).bind(views).execute(&mut *tx).await?;
    sqlx::query("UPDATE read_articles SET hot_rank = NULL WHERE hot_rank IS NOT NULL")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE read_articles a SET hot_rank = r.rank FROM unnest($1::uuid[]) WITH ORDINALITY AS r(id, rank) WHERE a.item_id = r.id")
        .bind(ranks).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

// Cumulative PV with a seven-day freshness half-life; this is not a weekly PV chart.
fn rank(rows: &[Reading], counts: &HashMap<Uuid, i64>, now: DateTime<Utc>) -> Vec<Uuid> {
    let mut candidates: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            let views = *counts.get(&row.id)?;
            if views == 0 {
                return None;
            }
            let age = (now - row.first_seen_at).num_seconds().max(0) as f64 / 86400.0;
            let score = (views as f64).ln_1p() * 2_f64.powf(-age / 7.0);
            Some((row, score))
        })
        .collect();
    candidates.sort_by(|(a, x), (b, y)| {
        y.total_cmp(x)
            .then(b.first_seen_at.cmp(&a.first_seen_at))
            .then(b.id.cmp(&a.id))
    });
    let mut sources = HashMap::<String, usize>::new();
    candidates
        .into_iter()
        .filter_map(|(row, _)| {
            let url = url::Url::parse(&row.url).ok()?;
            let source = url.host_str()?.trim_start_matches("www.").to_owned();
            let count = sources.entry(source).or_default();
            if *count == 2 {
                return None;
            }
            *count += 1;
            Some(row.id)
        })
        .take(24)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popularity_balances_freshness_diversity_and_real_views() {
        let now = Utc::now();
        let rows: Vec<_> = (0..60)
            .map(|i| Reading {
                id: Uuid::from_u128(i + 1),
                url: format!("https://source{}.example/essay/{i}", i / 3),
                first_seen_at: now - chrono::Duration::days(if i == 0 { 29 } else { 1 }),
            })
            .collect();
        let mut counts: HashMap<_, _> = rows.iter().map(|row| (row.id, 10)).collect();
        counts.insert(rows[0].id, 1000);
        counts.insert(rows[59].id, 0);
        let ranked = rank(&rows, &counts, now);
        assert_eq!(ranked.len(), 24);
        assert!(!ranked.contains(&rows[0].id));
        assert!(!ranked.contains(&rows[59].id));
        for source in rows.chunks(3) {
            assert!(source.iter().filter(|row| ranked.contains(&row.id)).count() <= 2);
        }
        assert!(rank(&rows, &HashMap::new(), now).is_empty());
    }
}
