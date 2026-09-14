-- Autonomous life is driven by next_scene_at and next_trip_at. Keep the old
-- guide endpoint callable on demand, but remove its unused scheduler columns.
DROP INDEX IF EXISTS cat_autonomy_due_idx;
ALTER TABLE cat_autonomy_state
    DROP COLUMN IF EXISTS next_guide_at,
    DROP COLUMN IF EXISTS last_guide_at;
