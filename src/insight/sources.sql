-- Keep the picture column as a separate candidate pool, even when it illustrates
-- a story also collected by another column. Other cross-column copies collapse.
WITH deduplicated AS (
    SELECT DISTINCT ON (i.fingerprint, t.id = 'p')
           t.id AS topic_id, t.name AS topic, t.name_en AS topic_en,
           i.title, i.title_en, i.summary, i.summary_en, i.url, i.source,
           i.tags, i.importance, i.first_seen_at, i.published_at,
           i.last_seen_at, i.seen_count,
           COALESCE(i.published_at, i.first_seen_at) AS event_at,
           COALESCE(NULLIF(i.image, 'null'::jsonb),
               CASE WHEN r.payload->'cover'->>'source_url' = i.url
                    THEN NULLIF(r.payload->'cover', 'null'::jsonb) END) AS cover
    FROM items i JOIN topics t ON t.id = i.topic_id
    LEFT JOIN runs r ON r.id = i.first_run_id
    -- Photograph and Culture collections stay outside the AI briefing.
    WHERE t.enabled AND t.visible AND t.owner_user_id IS NULL AND t.id NOT IN ('w', 'travel', 'movie', 'music', 'game', 'book', 'eink', 'ai-hardware', 'digital') AND NOT i.hidden
      AND COALESCE(i.published_at, i.first_seen_at) >= $1
      AND COALESCE(i.published_at, i.first_seen_at) < $3
    ORDER BY i.fingerprint, t.id = 'p', i.published_at IS NULL,
             t.id = 'gossip', i.importance DESC, i.first_seen_at, i.id
), ranked AS (
    SELECT *, ROW_NUMBER() OVER (
        PARTITION BY topic_id, date_bin(INTERVAL '24 hours', event_at, $3)
        ORDER BY importance DESC, event_at DESC, url
    ) AS topic_day_rank
    FROM deduplicated
), balanced AS (
    -- Interleave current and historical material so a busy day cannot fill the
    -- entire input budget before any seven-day evidence reaches the model.
    SELECT *, ROW_NUMBER() OVER (
        PARTITION BY event_at >= $2
        ORDER BY topic_day_rank, event_at DESC, importance DESC, url
    ) AS window_rank
    FROM ranked
    WHERE topic_day_rank <= CASE WHEN event_at >= $2 THEN 12 ELSE 3 END
)
SELECT topic_id, topic, topic_en, title, title_en, summary, summary_en, url,
       source, tags, importance, first_seen_at, published_at, last_seen_at,
       seen_count, cover
FROM balanced
ORDER BY window_rank, (event_at >= $2) DESC
LIMIT $4
