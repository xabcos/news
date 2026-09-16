-- Participation is opt-in. Existing companions keep their private life.
CREATE TABLE travel_world_settings (
    companion_id uuid PRIMARY KEY REFERENCES travel_companions(id) ON DELETE CASCADE,
    social_mode text NOT NULL DEFAULT 'private' CHECK (social_mode IN ('private', 'open')),
    next_match_at timestamptz NOT NULL DEFAULT NOW(),
    busy_until timestamptz,
    lease_token uuid,
    updated_at timestamptz NOT NULL DEFAULT NOW(),
    CHECK ((busy_until IS NULL) = (lease_token IS NULL))
);
CREATE INDEX travel_world_available_idx ON travel_world_settings (next_match_at)
    WHERE social_mode = 'open';

-- AI proposals are not facts. Only a validated, accepted proposal is committed
-- as a scene. Its original facts cannot later be rewritten by a photo retry.
CREATE TABLE travel_shared_events (
    id uuid PRIMARY KEY,
    first_companion_id uuid NOT NULL,
    second_companion_id uuid NOT NULL,
    first_trip_id uuid REFERENCES travel_trips(id) ON DELETE SET NULL,
    second_trip_id uuid REFERENCES travel_trips(id) ON DELETE SET NULL,
    location jsonb NOT NULL CHECK (jsonb_typeof(location) = 'object'),
    status text NOT NULL DEFAULT 'proposing'
        CHECK (status IN ('proposing', 'declined', 'confirmed', 'photographing', 'ready', 'photo_failed')),
    proposal jsonb,
    scene jsonb,
    photo jsonb,
    happened_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT NOW(),
    updated_at timestamptz NOT NULL DEFAULT NOW(),
    lease_token uuid,
    lease_until timestamptz,
    next_attempt_at timestamptz NOT NULL DEFAULT NOW(),
    photo_attempts integer NOT NULL DEFAULT 0 CHECK (photo_attempts BETWEEN 0 AND 3),
    error text,
    CHECK (first_companion_id < second_companion_id),
    CHECK ((lease_token IS NULL) = (lease_until IS NULL)),
    CHECK (status IN ('proposing', 'declined') OR COALESCE(
        (happened_at IS NOT NULL AND scene IS NOT NULL
         AND jsonb_typeof(scene->'participants') = 'array'
         AND jsonb_array_length(scene->'participants') = 2), FALSE)),
    CHECK (status <> 'ready' OR photo IS NOT NULL),
    UNIQUE (first_trip_id, second_trip_id)
);
CREATE INDEX travel_shared_events_pending_idx ON travel_shared_events (next_attempt_at)
    WHERE status IN ('confirmed', 'photographing', 'photo_failed');
CREATE INDEX travel_shared_events_lease_idx ON travel_shared_events (lease_until)
    WHERE status IN ('proposing', 'photographing');
CREATE INDEX travel_shared_events_pair_idx
    ON travel_shared_events (first_companion_id, second_companion_id, happened_at DESC);

CREATE FUNCTION protect_travel_shared_facts() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.status NOT IN ('proposing', 'declined') AND (
        NEW.first_companion_id IS DISTINCT FROM OLD.first_companion_id OR
        NEW.second_companion_id IS DISTINCT FROM OLD.second_companion_id OR
        NEW.location IS DISTINCT FROM OLD.location OR
        NEW.scene IS DISTINCT FROM OLD.scene OR
        NEW.happened_at IS DISTINCT FROM OLD.happened_at OR
        NEW.status IN ('proposing', 'declined')
    ) THEN
        RAISE EXCEPTION 'A shared travel memory cannot be rewritten';
    END IF;
    IF OLD.photo IS NOT NULL AND NEW.photo->>'asset_id' IS DISTINCT FROM OLD.photo->>'asset_id' THEN
        RAISE EXCEPTION 'A shared keepsake cannot be replaced by a different photograph';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER travel_shared_facts_immutable BEFORE UPDATE ON travel_shared_events
    FOR EACH ROW EXECUTE FUNCTION protect_travel_shared_facts();

CREATE TABLE travel_shared_event_members (
    event_id uuid NOT NULL REFERENCES travel_shared_events(id) ON DELETE CASCADE,
    companion_id uuid NOT NULL REFERENCES travel_companions(id) ON DELETE CASCADE,
    position smallint NOT NULL CHECK (position IN (1, 2)),
    personal_memory text NOT NULL CHECK (char_length(personal_memory) BETWEEN 1 AND 300),
    PRIMARY KEY (event_id, companion_id),
    UNIQUE (event_id, position)
);
CREATE INDEX travel_shared_event_members_companion_idx
    ON travel_shared_event_members (companion_id, event_id);

CREATE TABLE travel_relationships (
    companion_id uuid NOT NULL REFERENCES travel_companions(id) ON DELETE CASCADE,
    peer_id uuid NOT NULL REFERENCES travel_companions(id) ON DELETE CASCADE,
    meetings integer NOT NULL CHECK (meetings > 0),
    memory text NOT NULL CHECK (char_length(memory) BETWEEN 1 AND 300),
    first_met_at timestamptz NOT NULL,
    last_met_at timestamptz NOT NULL,
    last_event_id uuid REFERENCES travel_shared_events(id) ON DELETE SET NULL,
    PRIMARY KEY (companion_id, peer_id),
    CHECK (companion_id <> peer_id)
);

CREATE TABLE travel_world_memories (
    companion_id uuid NOT NULL REFERENCES travel_companions(id) ON DELETE CASCADE,
    event_id uuid NOT NULL REFERENCES travel_shared_events(id) ON DELETE CASCADE,
    peer_id uuid REFERENCES travel_companions(id) ON DELETE SET NULL,
    summary text NOT NULL CHECK (char_length(summary) BETWEEN 1 AND 300),
    created_at timestamptz NOT NULL DEFAULT NOW(),
    PRIMARY KEY (companion_id, event_id)
);
CREATE INDEX travel_world_memories_recent_idx ON travel_world_memories (companion_id, created_at DESC);

-- The media table is created with identity storage in 0027; its event owner
-- becomes available here. A keepsake cannot silently outlive its shared facts.
ALTER TABLE travel_media_assets ADD CONSTRAINT travel_media_shared_event_fk
    FOREIGN KEY (shared_event_id) REFERENCES travel_shared_events(id) ON DELETE RESTRICT;
