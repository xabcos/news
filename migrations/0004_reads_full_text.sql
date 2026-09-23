ALTER TABLE read_articles
    ADD COLUMN full_text jsonb CHECK (full_text IS NULL OR
        (mode = 'full' AND jsonb_typeof(full_text) = 'object' AND octet_length(full_text::text) <= 2097152)),
    ADD COLUMN images_renew_at timestamptz;

CREATE INDEX read_articles_images_renew_idx ON read_articles (images_renew_at)
    WHERE images_renew_at IS NOT NULL;
