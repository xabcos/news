-- Filter the daily statistics columns before deduplication, across the same seven-day window.
-- Other columns remain editorial sources but cannot displace or inflate this sample.
WITH news AS (
    SELECT DISTINCT ON (i.fingerprint)
           i.topic_id, t.name AS topic, i.tags, i.published_at AS event_at
    FROM items i JOIN topics t ON t.id = i.topic_id
    WHERE t.enabled AND t.owner_user_id IS NULL AND NOT i.hidden
      AND i.topic_id = ANY($4)
      AND i.published_at >= $1 AND i.published_at < $3
    ORDER BY i.fingerprint, i.importance DESC, i.first_seen_at, i.id
), topic_counts AS (
    SELECT topic_id, topic,
           COUNT(*) FILTER (WHERE event_at >= $2) AS current_items,
           COUNT(*) FILTER (WHERE event_at < $2) AS baseline_items
    FROM news GROUP BY topic_id, topic
), tag_counts AS (
    SELECT tag, COUNT(*) FILTER (WHERE event_at >= $2) AS current_items,
           COUNT(*) FILTER (WHERE event_at < $2) AS baseline_items
    FROM news CROSS JOIN LATERAL (
        SELECT DISTINCT CASE
            WHEN lower(btrim(tag)) IN ('agent', 'agents', 'ai agent', 'ai agents', '智能体', 'ai代理')
                THEN 'Agent'
            ELSE btrim(tag)
        END AS tag
        FROM unnest(news.tags) AS tags(tag)
        WHERE btrim(tag) <> ''
    ) tags
    GROUP BY tag
), daily AS (
    SELECT day AS window_start, COUNT(news.event_at) AS items
    FROM generate_series($1::timestamptz, $2::timestamptz, INTERVAL '24 hours') day
    LEFT JOIN news ON news.event_at >= day AND news.event_at < day + INTERVAL '24 hours'
    GROUP BY day ORDER BY day
)
SELECT jsonb_build_object(
    'topic_ids', $4::text[],
    'window_start', $2::timestamptz, 'window_end', $3::timestamptz,
    'trend_window_start', $1::timestamptz, 'baseline_days', 6,
    'current_items', (SELECT COUNT(*) FROM news WHERE event_at >= $2),
    'seven_day_items', (SELECT COUNT(*) FROM news),
    'baseline_daily_average', (SELECT ROUND(COUNT(*) / 6.0, 2) FROM news WHERE event_at < $2),
    'observed_days', (SELECT COUNT(*) FROM daily WHERE items > 0),
    'daily', (SELECT jsonb_agg(to_jsonb(daily)) FROM daily),
    'topics', COALESCE((SELECT jsonb_agg(jsonb_build_object(
        'topic_id', topic_id, 'topic', topic, 'current_items', current_items,
        'baseline_daily_average', ROUND(baseline_items / 6.0, 2),
        'change_percent', ROUND((current_items * 6.0 / NULLIF(baseline_items, 0) - 1) * 100, 1)
    ) ORDER BY current_items DESC, topic_id) FROM topic_counts), '[]'::jsonb),
    'tags', COALESCE((SELECT jsonb_agg(to_jsonb(ranked)) FROM (
        SELECT tag, current_items, ROUND(baseline_items / 6.0, 2) AS baseline_daily_average
        FROM tag_counts ORDER BY current_items DESC, baseline_items DESC, tag LIMIT 12
    ) ranked), '[]'::jsonb)
)
