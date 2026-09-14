-- The simplified cat page no longer has an inbox or an unread-return state.
-- Backfill rows created by the previous flow before that state becomes unreachable.
UPDATE cat_trips
SET status = 'home', read_at = COALESCE(read_at, NOW())
WHERE status = 'returned_unread';

-- The visible identity is now fixed for every adopted cat.
UPDATE cats
SET name = '旅行猫咪', name_updated_at = NULL
WHERE name IS DISTINCT FROM '旅行猫咪' OR name_updated_at IS NOT NULL;

DROP INDEX IF EXISTS cat_messages_unread_idx;
ALTER TABLE cats DROP COLUMN IF EXISTS inbox_seen_at;
