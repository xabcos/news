-- Keep the original successful portrait independent of expiring public media.
CREATE TABLE travel_portrait_anchors (
    girl_id uuid NOT NULL REFERENCES travel_girls(id) ON DELETE CASCADE,
    visual_version text NOT NULL,
    model text NOT NULL,
    content_type text NOT NULL CHECK (content_type IN ('image/png', 'image/jpeg', 'image/webp')),
    image_bytes bytea NOT NULL CHECK (octet_length(image_bytes) BETWEEN 1 AND 16777216),
    created_at timestamptz NOT NULL DEFAULT NOW(),
    PRIMARY KEY (girl_id, visual_version)
);

CREATE FUNCTION protect_travel_portrait_anchor() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'The original travel portrait cannot be replaced';
END;
$$;
CREATE TRIGGER travel_portrait_anchor_immutable
BEFORE UPDATE ON travel_portrait_anchors
FOR EACH ROW EXECUTE FUNCTION protect_travel_portrait_anchor();

-- A bounded lease serializes a girl's first portrait and subsequent edits even
-- across workers, without holding a database connection during image generation.
ALTER TABLE travel_girls
    ADD COLUMN image_lease_token uuid,
    ADD COLUMN image_lease_until timestamptz,
    ADD CONSTRAINT travel_image_lease_pair CHECK (
        (image_lease_token IS NULL) = (image_lease_until IS NULL)
    );

-- Reject identical appearance recipes, independently of random account seeds.
CREATE UNIQUE INDEX travel_girls_appearance_unique
ON travel_girls ((identity->>'appearance_key'))
WHERE identity ? 'appearance_key';

-- Drafts are cheap and cannot carry an older derivation recipe into confirmation.
DELETE FROM travel_adoption_drafts;

ALTER TABLE travel_adoption_drafts
    ADD COLUMN generation_error text,
    ADD COLUMN identity jsonb CHECK (identity IS NULL OR jsonb_typeof(identity) = 'object'),
    ADD COLUMN initial_accessories jsonb NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE travel_postcards
    ADD COLUMN creative_plan jsonb NOT NULL DEFAULT '{}'::jsonb;

CREATE INDEX travel_postcards_girl_created_idx
ON travel_postcards ((split_part(source_key, ':', 2)), created_at DESC)
WHERE source_key LIKE 'travel:%';
