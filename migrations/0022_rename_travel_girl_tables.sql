-- Remove the legacy cat schema names now that the product is a travel girl.
-- The previous cleanup left these tables empty; this migration keeps any future
-- rows while removing the old table and foreign-key column names.

ALTER TABLE IF EXISTS cats RENAME TO travel_girls;
ALTER TABLE IF EXISTS cat_adoption_drafts RENAME TO travel_adoption_drafts;
ALTER TABLE IF EXISTS cat_growth_events RENAME TO travel_growth_events;
ALTER TABLE IF EXISTS cat_postcards RENAME TO travel_postcards;
ALTER TABLE IF EXISTS cat_trips RENAME TO travel_trips;
ALTER TABLE IF EXISTS cat_encounters RENAME TO travel_encounters;
ALTER TABLE IF EXISTS cat_encounter_members RENAME TO travel_encounter_members;
ALTER TABLE IF EXISTS cat_accessory_unlocks RENAME TO travel_accessory_unlocks;
ALTER TABLE IF EXISTS cat_memories RENAME TO travel_memories;
ALTER TABLE IF EXISTS cat_reading_daily RENAME TO travel_reading_daily;
ALTER TABLE IF EXISTS cat_personality_snapshots RENAME TO travel_personality_snapshots;
ALTER TABLE IF EXISTS cat_world_briefs RENAME TO travel_world_briefs;
ALTER TABLE IF EXISTS cat_autonomy_state RENAME TO travel_autonomy_state;
ALTER TABLE IF EXISTS cat_messages RENAME TO travel_messages;

ALTER TABLE travel_growth_events RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_trips RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_encounter_members RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_accessory_unlocks RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_memories RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_reading_daily RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_personality_snapshots RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_world_briefs RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_autonomy_state RENAME COLUMN cat_id TO girl_id;
ALTER TABLE travel_messages RENAME COLUMN cat_id TO girl_id;

ALTER FUNCTION protect_cat_identity() RENAME TO protect_travel_girl_identity;
ALTER FUNCTION forbid_growth_event_rewrite() RENAME TO forbid_travel_growth_event_rewrite;
ALTER TRIGGER cats_identity_immutable ON travel_girls RENAME TO travel_girls_identity_immutable;
ALTER TRIGGER cat_growth_events_append_only ON travel_growth_events RENAME TO travel_growth_events_append_only;

ALTER INDEX IF EXISTS cat_growth_events_cat_created_idx RENAME TO travel_growth_events_girl_created_idx;
ALTER INDEX IF EXISTS cat_trips_due_idx RENAME TO travel_trips_due_idx;
ALTER INDEX IF EXISTS cat_trips_cat_idx RENAME TO travel_trips_girl_idx;
ALTER INDEX IF EXISTS cat_trips_settling_idx RENAME TO travel_trips_settling_idx;
ALTER INDEX IF EXISTS cat_trips_active_cat_idx RENAME TO travel_trips_active_girl_idx;
ALTER INDEX IF EXISTS cat_encounter_members_cat_idx RENAME TO travel_encounter_members_girl_idx;
ALTER INDEX IF EXISTS cat_memories_cat_seen_idx RENAME TO travel_memories_girl_seen_idx;
ALTER INDEX IF EXISTS cat_personality_snapshots_cat_created_idx RENAME TO travel_personality_snapshots_girl_created_idx;
ALTER INDEX IF EXISTS cat_world_briefs_cat_day_idx RENAME TO travel_world_briefs_girl_day_idx;
ALTER INDEX IF EXISTS cat_autonomy_due_idx RENAME TO travel_autonomy_due_idx;
ALTER INDEX IF EXISTS cat_autonomy_scene_due_idx RENAME TO travel_autonomy_scene_due_idx;
ALTER INDEX IF EXISTS cat_messages_cat_created_idx RENAME TO travel_messages_girl_created_idx;
ALTER INDEX IF EXISTS cat_messages_unread_idx RENAME TO travel_messages_unread_idx;

ALTER TABLE travel_girls RENAME CONSTRAINT cats_name_valid TO travel_girls_name_valid;
ALTER TABLE travel_trips RENAME CONSTRAINT cat_trips_status_valid TO travel_trips_status_valid;
ALTER TABLE travel_trips RENAME CONSTRAINT cat_trips_origin_valid TO travel_trips_origin_valid;
ALTER TABLE travel_world_briefs RENAME CONSTRAINT cat_world_briefs_status_valid TO travel_world_briefs_status_valid;
