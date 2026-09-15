-- Keep the frequent 24-hour and seven-day insight scans on a narrow index.
CREATE INDEX IF NOT EXISTS items_first_seen_visible_idx
    ON items (first_seen_at DESC, topic_id)
    WHERE NOT hidden;
