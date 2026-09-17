-- One explicit creation action owns one request ID. Replaying it observes the
-- saved draft; another action may create a new person with identical preferences.
ALTER TABLE travel_adoption_drafts ADD COLUMN request_id uuid;
