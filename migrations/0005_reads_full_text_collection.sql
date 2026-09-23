-- Full-text availability is represented by full_text itself.
ALTER TABLE read_articles
    DROP COLUMN mode,
    DROP COLUMN license_name,
    DROP COLUMN license_url,
    ADD CONSTRAINT read_articles_document_check CHECK (full_text IS NULL OR
        (jsonb_typeof(full_text) = 'object' AND octet_length(full_text::text) <= 2097152));

UPDATE topics
SET brief = REPLACE(brief, '正文与转载许可', '正文')
WHERE id = 'reads' AND owner_user_id IS NULL;
