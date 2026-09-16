-- Along keeps every established person's UUID, identity and original portraits.
ALTER TABLE travel_girls RENAME TO travel_companions;
DO $$ DECLARE r record; renamed text; BEGIN
  FOR r IN SELECT table_name FROM information_schema.columns
    WHERE table_schema='public' AND table_name LIKE 'travel_%' AND column_name='girl_id'
  LOOP EXECUTE format('ALTER TABLE %I RENAME COLUMN girl_id TO companion_id', r.table_name); END LOOP;
  FOR r IN SELECT c.conname,t.relname FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid
    JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname='public' AND t.relname LIKE 'travel_%' AND c.conname LIKE '%girl%'
  LOOP renamed:=replace(replace(r.conname,'girls','companions'),'girl','companion');
    EXECUTE format('ALTER TABLE %I RENAME CONSTRAINT %I TO %I',r.relname,r.conname,renamed); END LOOP;
  FOR r IN SELECT indexname FROM pg_indexes WHERE schemaname='public' AND tablename LIKE 'travel_%' AND indexname LIKE '%girl%'
  LOOP renamed:=replace(replace(r.indexname,'girls','companions'),'girl','companion');
    EXECUTE format('ALTER INDEX %I RENAME TO %I',r.indexname,renamed); END LOOP;
  FOR r IN SELECT idx.relname FROM pg_index i JOIN pg_class t ON t.oid=i.indrelid JOIN pg_class idx ON idx.oid=i.indexrelid
    JOIN pg_namespace n ON n.oid=t.relnamespace JOIN pg_attribute a ON a.attrelid=idx.oid
    WHERE n.nspname='public' AND t.relname LIKE 'travel_%' AND a.attname='girl_id' AND a.attnum>0
  LOOP EXECUTE format('ALTER TABLE %I RENAME COLUMN girl_id TO companion_id',r.relname); END LOOP;
END $$;
ALTER TRIGGER travel_girls_identity_immutable ON travel_companions RENAME TO travel_companions_identity_immutable;
ALTER FUNCTION protect_travel_girl_identity() RENAME TO protect_travel_companion_identity;
ALTER TABLE travel_companions ADD COLUMN identity_archive_at timestamptz;
ALTER TABLE travel_companions ADD COLUMN gender text NOT NULL DEFAULT 'female' CHECK(gender IN ('male','female'));
CREATE OR REPLACE FUNCTION protect_travel_companion_identity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.owner_user_id IS DISTINCT FROM OLD.owner_user_id
 OR NEW.identity_version IS DISTINCT FROM OLD.identity_version OR NEW.identity_seed IS DISTINCT FROM OLD.identity_seed
 OR NEW.identity_fingerprint IS DISTINCT FROM OLD.identity_fingerprint OR NEW.identity IS DISTINCT FROM OLD.identity
 OR NEW.initial_accessories IS DISTINCT FROM OLD.initial_accessories OR NEW.adopted_at IS DISTINCT FROM OLD.adopted_at
 OR NEW.gender IS DISTINCT FROM OLD.gender THEN RAISE EXCEPTION 'Along identity is immutable'; END IF;
 RETURN NEW;
END $$;
DROP INDEX travel_companions_appearance_unique;
CREATE UNIQUE INDEX travel_companions_appearance_unique ON travel_companions((identity->>'appearance_key'))
 WHERE identity ? 'appearance_key' AND COALESCE(identity->>'source','ai')='ai';

-- Previously unconfirmed drafts contain text only, no permanent person or image.
DELETE FROM travel_adoption_drafts;
ALTER TABLE travel_adoption_drafts
 ADD COLUMN creation_spec jsonb NOT NULL DEFAULT '{}'::jsonb,
 ADD COLUMN request_fingerprint bytea,
 ADD COLUMN stage text NOT NULL DEFAULT 'identity' CHECK(stage IN ('identity','portrait','ready','failed')),
 ADD COLUMN portrait_asset_id uuid;

CREATE TABLE travel_identity_assets (
 id uuid PRIMARY KEY,
 companion_id uuid REFERENCES travel_companions(id) ON DELETE RESTRICT,
 draft_id uuid REFERENCES travel_adoption_drafts(id) ON DELETE CASCADE,
 role text NOT NULL CHECK(role IN ('source','portrait')),
 ordinal smallint NOT NULL CHECK(ordinal>=0 AND (role<>'source' OR ordinal<7)),
 content_type text NOT NULL CHECK(content_type IN ('image/jpeg','image/png','image/webp')),
 image_content_type text NOT NULL CHECK(image_content_type IN ('image/jpeg','image/png','image/webp')),
 original_bytes bytea NOT NULL CHECK(octet_length(original_bytes) BETWEEN 1 AND 16777216),
 image_bytes bytea NOT NULL CHECK(octet_length(image_bytes) BETWEEN 1 AND 16777216),
 sha256 bytea NOT NULL CHECK(octet_length(sha256)=32 AND sha256=sha256(original_bytes)),
 provenance jsonb NOT NULL DEFAULT '{}'::jsonb,
 created_at timestamptz NOT NULL DEFAULT NOW(),
 CHECK((companion_id IS NULL) <> (draft_id IS NULL)),
 UNIQUE(companion_id,role,ordinal), UNIQUE(draft_id,role,ordinal)
);
INSERT INTO travel_identity_assets(id,companion_id,role,ordinal,content_type,image_content_type,original_bytes,image_bytes,sha256,provenance,created_at)
 SELECT gen_random_uuid(),companion_id,'portrait',(row_number() OVER(PARTITION BY companion_id ORDER BY created_at,visual_version)-1)::smallint,
 content_type,content_type,image_bytes,image_bytes,sha256(image_bytes),jsonb_build_object('model',model,'visual_version',visual_version,'origin','legacy_original'),created_at
 FROM travel_portrait_anchors;
DROP TABLE travel_portrait_anchors;
DROP FUNCTION protect_travel_portrait_anchor();
ALTER TABLE travel_adoption_drafts ADD CONSTRAINT travel_draft_portrait_fk FOREIGN KEY(portrait_asset_id)
 REFERENCES travel_identity_assets(id) ON DELETE SET NULL;
CREATE FUNCTION protect_travel_identity_asset() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF TG_OP='DELETE' THEN
   IF OLD.companion_id IS NOT NULL THEN RAISE EXCEPTION 'A permanent Along identity asset cannot be deleted by routine operations'; END IF;
   RETURN OLD;
 END IF;
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.role IS DISTINCT FROM OLD.role OR NEW.ordinal IS DISTINCT FROM OLD.ordinal
 OR NEW.content_type IS DISTINCT FROM OLD.content_type OR NEW.image_content_type IS DISTINCT FROM OLD.image_content_type
 OR NEW.original_bytes IS DISTINCT FROM OLD.original_bytes OR NEW.image_bytes IS DISTINCT FROM OLD.image_bytes
 OR NEW.sha256 IS DISTINCT FROM OLD.sha256 OR NEW.provenance IS DISTINCT FROM OLD.provenance OR NEW.created_at IS DISTINCT FROM OLD.created_at
 OR (OLD.companion_id IS NOT NULL AND (NEW.companion_id IS DISTINCT FROM OLD.companion_id OR NEW.draft_id IS DISTINCT FROM OLD.draft_id))
 OR (OLD.companion_id IS NULL AND NOT(NEW.companion_id IS NOT NULL AND NEW.draft_id IS NULL))
 THEN RAISE EXCEPTION 'Along original assets are immutable'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER travel_identity_asset_immutable BEFORE UPDATE OR DELETE ON travel_identity_assets
 FOR EACH ROW EXECUTE FUNCTION protect_travel_identity_asset();
-- TRUNCATE bypasses row triggers, so permanent originals explicitly reject it.
CREATE FUNCTION forbid_along_identity_truncate() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Permanent Along originals cannot be truncated'; END $$;
CREATE TRIGGER travel_identity_assets_no_truncate BEFORE TRUNCATE ON travel_identity_assets
 FOR EACH STATEMENT EXECUTE FUNCTION forbid_along_identity_truncate();

CREATE TABLE travel_media_assets (
 id uuid PRIMARY KEY,
 companion_id uuid REFERENCES travel_companions(id) ON DELETE RESTRICT,
 shared_event_id uuid UNIQUE,
 image_bytes bytea NOT NULL CHECK(octet_length(image_bytes) BETWEEN 1 AND 16777216),
 content_type text NOT NULL CHECK(content_type IN ('image/jpeg','image/png','image/webp')),
 checksum bytea NOT NULL CHECK(octet_length(checksum)=32 AND checksum=sha256(image_bytes)),
 public_url text, expires_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT NOW(),
 CHECK((companion_id IS NULL) <> (shared_event_id IS NULL))
);
ALTER TABLE travel_postcards ADD COLUMN media_asset_id uuid REFERENCES travel_media_assets(id) ON DELETE RESTRICT;
CREATE FUNCTION protect_along_media_original() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.id IS DISTINCT FROM OLD.id OR NEW.companion_id IS DISTINCT FROM OLD.companion_id
 OR NEW.shared_event_id IS DISTINCT FROM OLD.shared_event_id OR NEW.image_bytes IS DISTINCT FROM OLD.image_bytes
 OR NEW.content_type IS DISTINCT FROM OLD.content_type OR NEW.checksum IS DISTINCT FROM OLD.checksum
 THEN RAISE EXCEPTION 'An Along keepsake cannot be repainted'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER travel_media_original_immutable BEFORE UPDATE ON travel_media_assets
 FOR EACH ROW EXECUTE FUNCTION protect_along_media_original();

ALTER TABLE travel_adoption_drafts ADD CONSTRAINT travel_ready_draft_has_portrait
 CHECK(stage<>'ready' OR (identity IS NOT NULL AND portrait_asset_id IS NOT NULL));
