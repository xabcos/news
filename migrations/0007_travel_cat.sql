-- Travel Cat: durable identity, growth, travel, encounters, and world guide.
--
-- This is the squashed first-release migration for the cat feature. The adopted
-- identity is immutable; growth events are append-only source facts, while
-- personality snapshots and world briefs are rebuildable projections.

CREATE TABLE cat_adoption_drafts (
    user_id        BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    id             UUID NOT NULL UNIQUE,
    identity_version TEXT NOT NULL,
    identity_seed  BYTEA NOT NULL CHECK (OCTET_LENGTH(identity_seed) = 32),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE cats (
    id                   UUID PRIMARY KEY,
    owner_user_id        BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    name                 TEXT NOT NULL,
    identity_version     TEXT NOT NULL,
    identity_seed        BYTEA NOT NULL UNIQUE CHECK (OCTET_LENGTH(identity_seed) = 32),
    identity_fingerprint BYTEA NOT NULL UNIQUE CHECK (OCTET_LENGTH(identity_fingerprint) = 32),
    identity             JSONB NOT NULL,
    initial_accessories  JSONB NOT NULL,
    adopted_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    name_updated_at      TIMESTAMPTZ,
    last_seen_at         TIMESTAMPTZ,
    CONSTRAINT cats_name_valid CHECK (
        name = BTRIM(name) AND CHAR_LENGTH(name) BETWEEN 1 AND 24
    )
);

CREATE TABLE cat_growth_events (
    id         UUID PRIMARY KEY,
    cat_id     UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL CHECK (event_type ~ '^[a-z_]{1,32}$'),
    source_key TEXT NOT NULL CHECK (CHAR_LENGTH(source_key) BETWEEN 1 AND 128),
    payload    JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (cat_id, event_type, source_key)
);

CREATE INDEX cat_growth_events_cat_created_idx
    ON cat_growth_events (cat_id, created_at DESC);

-- Identity, ownership and the adoption snapshot are immutable. Name and
-- last_seen_at remain mutable through their dedicated APIs.
CREATE FUNCTION protect_cat_identity() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.id                   IS DISTINCT FROM OLD.id
    OR NEW.owner_user_id        IS DISTINCT FROM OLD.owner_user_id
    OR NEW.identity_version     IS DISTINCT FROM OLD.identity_version
    OR NEW.identity_seed        IS DISTINCT FROM OLD.identity_seed
    OR NEW.identity_fingerprint IS DISTINCT FROM OLD.identity_fingerprint
    OR NEW.identity             IS DISTINCT FROM OLD.identity
    OR NEW.initial_accessories  IS DISTINCT FROM OLD.initial_accessories
    OR NEW.adopted_at           IS DISTINCT FROM OLD.adopted_at THEN
        RAISE EXCEPTION 'cat identity is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER cats_identity_immutable
BEFORE UPDATE ON cats
FOR EACH ROW
EXECUTE FUNCTION protect_cat_identity();

-- Direct growth-event deletes remain forbidden while account/cat cascades are
-- allowed. PostgreSQL reports cascaded FK actions at trigger depth 2; direct
-- deletes are depth 1.
CREATE FUNCTION forbid_growth_event_rewrite() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' AND pg_trigger_depth() <= 1 THEN
        RAISE EXCEPTION 'cat growth events are append-only';
    END IF;
    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'cat growth events are append-only';
    END IF;
    RETURN COALESCE(NEW, OLD);
END;
$$;

CREATE TRIGGER cat_growth_events_append_only
BEFORE UPDATE OR DELETE ON cat_growth_events
FOR EACH ROW
EXECUTE FUNCTION forbid_growth_event_rewrite();

CREATE TABLE cat_postcards (
    id                   UUID PRIMARY KEY,
    source_key           TEXT NOT NULL UNIQUE,
    news_item_id         UUID REFERENCES items(id) ON DELETE SET NULL,
    tone                 TEXT NOT NULL DEFAULT 'curious',
    provider             TEXT NOT NULL,
    model                TEXT NOT NULL,
    url                  TEXT NOT NULL,
    origin_bytes_sha256  BYTEA,
    expires_at            TIMESTAMPTZ,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE cat_trips (
    id            UUID PRIMARY KEY,
    cat_id        UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    news_item_id  UUID REFERENCES items(id) ON DELETE SET NULL,
    day           DATE NOT NULL,
    status        TEXT NOT NULL DEFAULT 'away',
    pack          TEXT[] NOT NULL DEFAULT '{}',
    departed_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    returns_at    TIMESTAMPTZ NOT NULL,
    settled_at    TIMESTAMPTZ,
    settling_at   TIMESTAMPTZ,
    read_at       TIMESTAMPTZ,
    postcard_id   UUID REFERENCES cat_postcards(id) ON DELETE SET NULL,
    diary         JSONB,
    souvenir      JSONB,
    error         TEXT,
    cost_ticks    BIGINT NOT NULL DEFAULT 0,
    tone          TEXT NOT NULL DEFAULT 'curious',
    location      JSONB,
    CONSTRAINT cat_trips_status_valid CHECK (
        status IN ('away', 'settling', 'returned_unread', 'home', 'vigil')
    )
);

CREATE INDEX cat_trips_due_idx
    ON cat_trips (returns_at)
    WHERE status = 'away';
CREATE INDEX cat_trips_cat_idx
    ON cat_trips (cat_id, departed_at DESC);
CREATE INDEX cat_trips_settling_idx
    ON cat_trips (settling_at)
    WHERE status = 'settling';
CREATE UNIQUE INDEX cat_trips_active_cat_idx
    ON cat_trips (cat_id)
    WHERE status IN ('away', 'settling', 'returned_unread');

-- A world encounter is a set of cats, never a pair.
CREATE TABLE cat_encounters (
    id           UUID PRIMARY KEY,
    world_key    TEXT NOT NULL,
    day          DATE NOT NULL,
    news_item_id UUID REFERENCES items(id) ON DELETE SET NULL,
    postcard_id  UUID REFERENCES cat_postcards(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (world_key, day)
);

CREATE TABLE cat_encounter_members (
    encounter_id UUID NOT NULL REFERENCES cat_encounters(id) ON DELETE CASCADE,
    cat_id       UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    phrase_id    TEXT,
    joined_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (encounter_id, cat_id)
);

CREATE INDEX cat_encounter_members_cat_idx
    ON cat_encounter_members (cat_id, joined_at DESC);

-- Safe news metadata used by travel, reading and the daily world guide.
ALTER TABLE items
    ADD COLUMN tone TEXT NOT NULL DEFAULT 'curious',
    ADD COLUMN location JSONB;

ALTER TABLE items
    ADD CONSTRAINT items_tone_valid
    CHECK (tone IN ('joyful', 'curious', 'serious', 'somber'));

CREATE TABLE cat_accessory_unlocks (
    cat_id       UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    accessory_id TEXT NOT NULL,
    source_event UUID NOT NULL REFERENCES cat_growth_events(id) ON DELETE CASCADE,
    unlocked_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (cat_id, accessory_id)
);

CREATE TABLE cat_memories (
    cat_id       UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('item', 'topic', 'tag', 'place')),
    memory_key   TEXT NOT NULL,
    label        TEXT NOT NULL DEFAULT '',
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    times        INTEGER NOT NULL DEFAULT 1 CHECK (times > 0),
    PRIMARY KEY (cat_id, kind, memory_key)
);

CREATE INDEX cat_memories_cat_seen_idx
    ON cat_memories (cat_id, last_seen_at DESC);

CREATE TABLE cat_reading_daily (
    cat_id     UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    day        DATE NOT NULL,
    topics     JSONB NOT NULL DEFAULT '{}',
    item_ids   UUID[] NOT NULL DEFAULT '{}',
    read_count INTEGER NOT NULL DEFAULT 0 CHECK (read_count >= 0),
    PRIMARY KEY (cat_id, day)
);

-- Versioned personality projections and daily news briefings are rebuildable;
-- neither table changes the adopted identity.
CREATE TABLE cat_personality_snapshots (
    id                    UUID PRIMARY KEY,
    cat_id                UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    revision              BIGINT NOT NULL,
    traits                JSONB NOT NULL DEFAULT '{}'::jsonb,
    interests             JSONB NOT NULL DEFAULT '[]'::jsonb,
    voice                 JSONB NOT NULL DEFAULT '{}'::jsonb,
    reflection            JSONB NOT NULL DEFAULT '{}'::jsonb,
    evidence              JSONB NOT NULL DEFAULT '[]'::jsonb,
    source_event_count    BIGINT NOT NULL DEFAULT 0 CHECK (source_event_count >= 0),
    model                 TEXT NOT NULL,
    prompt_version        TEXT NOT NULL,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (cat_id, revision)
);

CREATE INDEX cat_personality_snapshots_cat_created_idx
    ON cat_personality_snapshots (cat_id, created_at DESC);

CREATE TABLE cat_world_briefs (
    id                    UUID PRIMARY KEY,
    cat_id                UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    day                   DATE NOT NULL,
    status                TEXT NOT NULL DEFAULT 'running',
    item_ids              UUID[] NOT NULL DEFAULT '{}',
    personality_snapshot_id UUID REFERENCES cat_personality_snapshots(id) ON DELETE SET NULL,
    payload               JSONB NOT NULL DEFAULT '{}'::jsonb,
    model                 TEXT NOT NULL,
    prompt_version        TEXT NOT NULL,
    x_search_calls        INTEGER NOT NULL DEFAULT 0 CHECK (x_search_calls >= 0),
    attempts              INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0 AND attempts <= 3),
    started_at            TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at           TIMESTAMPTZ,
    error                 TEXT,
    CONSTRAINT cat_world_briefs_status_valid CHECK (status IN ('running', 'ok', 'failed')),
    UNIQUE (cat_id, day)
);

CREATE INDEX cat_world_briefs_cat_day_idx
    ON cat_world_briefs (cat_id, day DESC);

-- Autonomous life is a durable projection. The scheduler can be restarted or
-- replicated without losing the next planned activity or delivering duplicates.
CREATE TABLE cat_autonomy_state (
    cat_id          UUID PRIMARY KEY REFERENCES cats(id) ON DELETE CASCADE,
    next_guide_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    next_trip_at    TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '6 hours',
    last_guide_at   TIMESTAMPTZ,
    last_trip_at    TIMESTAMPTZ,
    lease_until     TIMESTAMPTZ,
    lease_token     UUID,
    failures        INTEGER NOT NULL DEFAULT 0 CHECK (failures >= 0 AND failures <= 20),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX cat_autonomy_due_idx
    ON cat_autonomy_state (next_guide_at, next_trip_at);

-- Messages are the owner's durable view of autonomous life. Facts remain in
-- trips/briefs/events; this table only records what should appear in the inbox.
CREATE TABLE cat_messages (
    id           UUID PRIMARY KEY,
    cat_id       UUID NOT NULL REFERENCES cats(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('welcome', 'guide', 'travel', 'postcard', 'growth', 'encounter')),
    dedupe_key   TEXT NOT NULL UNIQUE,
    entity_id    UUID,
    payload      JSONB NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    read_at      TIMESTAMPTZ
);

CREATE INDEX cat_messages_cat_created_idx
    ON cat_messages (cat_id, created_at DESC);
CREATE INDEX cat_messages_unread_idx
    ON cat_messages (cat_id, created_at DESC)
    WHERE read_at IS NULL;

ALTER TABLE cats
    ADD COLUMN inbox_seen_at TIMESTAMPTZ;

ALTER TABLE cat_trips
    ADD COLUMN origin TEXT NOT NULL DEFAULT 'manual',
    ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0 AND attempts <= 5),
    ADD COLUMN news_snapshot JSONB,
    ADD COLUMN note JSONB;

ALTER TABLE cat_trips
    ADD CONSTRAINT cat_trips_origin_valid CHECK (origin IN ('manual', 'autonomous'));
