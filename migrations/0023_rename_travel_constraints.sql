-- Finish removing legacy names from indexes and constraints left behind by
-- PostgreSQL when the tables and columns were renamed in migration 0022.
DO $$
DECLARE
    item RECORD;
    renamed TEXT;
    travel_tables CONSTANT TEXT[] := ARRAY[
        'travel_girls', 'travel_adoption_drafts', 'travel_growth_events',
        'travel_postcards', 'travel_trips', 'travel_encounters',
        'travel_encounter_members', 'travel_accessory_unlocks',
        'travel_memories', 'travel_reading_daily',
        'travel_personality_snapshots', 'travel_world_briefs',
        'travel_autonomy_state', 'travel_messages'
    ];
BEGIN
    -- Rename constraints first: PostgreSQL also renames their backing indexes.
    FOR item IN
        SELECT n.nspname AS schema_name, cls.relname AS table_name, con.conname AS constraint_name
        FROM pg_constraint con
        JOIN pg_class cls ON cls.oid = con.conrelid
        JOIN pg_namespace n ON n.oid = cls.relnamespace
        WHERE n.nspname = 'public'
          AND cls.relname = ANY (travel_tables)
          AND con.conname ~ '^cats?_'
    LOOP
        renamed := replace(item.constraint_name, '_cat_id', '_girl_id');
        renamed := regexp_replace(renamed, '^cats_', 'travel_girls_');
        renamed := regexp_replace(renamed, '^cat_', 'travel_');
        IF renamed <> item.constraint_name THEN
            EXECUTE format(
                'ALTER TABLE %I.%I RENAME CONSTRAINT %I TO %I',
                item.schema_name,
                item.table_name,
                item.constraint_name,
                renamed
            );
        END IF;
    END LOOP;

    -- Any remaining standalone indexes are renamed without touching other modules.
    FOR item IN
        SELECT schemaname AS schema_name, indexname AS index_name
        FROM pg_indexes
        WHERE schemaname = 'public'
          AND tablename = ANY (travel_tables)
          AND indexname ~ '^cats?_'
    LOOP
        renamed := replace(item.index_name, '_cat_id', '_girl_id');
        renamed := regexp_replace(renamed, '^cats_', 'travel_girls_');
        renamed := regexp_replace(renamed, '^cat_', 'travel_');
        EXECUTE format('ALTER INDEX %I.%I RENAME TO %I', item.schema_name, item.index_name, renamed);
    END LOOP;

    -- Table-column renames do not rename the index tuple's own attributes.
    -- ALTER TABLE supports renaming those attributes without rebuilding indexes.
    FOR item IN
        SELECT ns.nspname AS schema_name, idx.relname AS index_name, a.attname
        FROM pg_index i
        JOIN pg_class tbl ON tbl.oid = i.indrelid
        JOIN pg_class idx ON idx.oid = i.indexrelid
        JOIN pg_namespace ns ON ns.oid = tbl.relnamespace
        JOIN pg_attribute a ON a.attrelid = idx.oid
        WHERE ns.nspname = 'public'
          AND tbl.relname = ANY (travel_tables)
          AND a.attname = 'cat_id'
          AND a.attnum > 0 AND NOT a.attisdropped
    LOOP
        EXECUTE format(
            'ALTER TABLE %I.%I RENAME COLUMN %I TO girl_id',
            item.schema_name, item.index_name, item.attname
        );
    END LOOP;
END;
$$;

-- Replace the function bodies as well as their names. The protection rules and
-- trigger identities remain unchanged for already-confirmed travel companions.
CREATE OR REPLACE FUNCTION protect_travel_girl_identity() RETURNS trigger
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
        RAISE EXCEPTION 'travel girl identity is immutable';
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION forbid_travel_growth_event_rewrite() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'DELETE' AND pg_trigger_depth() <= 1 THEN
        RAISE EXCEPTION 'travel growth events are append-only';
    END IF;
    IF TG_OP = 'UPDATE' THEN
        RAISE EXCEPTION 'travel growth events are append-only';
    END IF;
    RETURN COALESCE(NEW, OLD);
END;
$$;
