-- Autonomous visual life updates run independently from the daily news guide.
ALTER TABLE cat_autonomy_state
    ADD COLUMN next_scene_at TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '10 minutes';

CREATE INDEX cat_autonomy_scene_due_idx
    ON cat_autonomy_state (next_scene_at);
