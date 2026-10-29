-- A topic's visible days, read index-only by `/days` and `latest_day`. `items_unique`
-- (topic_id, day, fingerprint) serves a topic's day, and the retention sweep skips through
-- this index by day, so the wider topic-day index and the day index go.
DROP INDEX items_topic_day_idx;
DROP INDEX items_day_idx;
CREATE INDEX items_topic_hidden_day_idx ON items (topic_id, hidden, day);
-- Sessions are deleted by token and by expiry, never by user.
DROP INDEX sessions_user_idx;
-- A repeat sighting is counted (`seen_count`); when it happened was never read.
ALTER TABLE items DROP COLUMN last_seen_at;
-- Stored briefings drop the two fields their sources no longer carry.
UPDATE ai_insights SET payload = jsonb_set(payload, '{sources}', COALESCE(
    (SELECT jsonb_agg(source - 'last_seen_at' - 'seen_count' ORDER BY position)
     FROM jsonb_array_elements(payload->'sources') WITH ORDINALITY AS s(source, position)),
    '[]'))
WHERE payload ? 'sources';
-- Nothing reads a run's or a briefing's response id, a briefing's token counts and cost, or
-- when a session or an article's picture was created.
ALTER TABLE runs DROP COLUMN response_id;
UPDATE runs SET payload = payload - 'selection_response_id' WHERE payload ? 'selection_response_id';
ALTER TABLE ai_insights DROP COLUMN response_id, DROP COLUMN input_tokens, DROP COLUMN cached_tokens,
    DROP COLUMN output_tokens, DROP COLUMN reasoning_tokens, DROP COLUMN web_search_calls,
    DROP COLUMN x_search_calls, DROP COLUMN cost_ticks;
ALTER TABLE sessions DROP COLUMN created_at;
ALTER TABLE article_images DROP COLUMN created_at;
-- 趣图 and AI 创作 now list their newest run first (`digest::BY_ARRIVAL`); open pages reload.
UPDATE topics SET feed_revision = feed_revision + 1 WHERE id IN ('fun', 'creations');
