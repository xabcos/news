-- Codex & Claude 手册: XABC keeps each chapter of a handbook on Claude Code and Codex in one
-- article, `chapter` its key (articles::handbook::CHAPTERS). `sources` holds the fingerprint of
-- each official page the chapter was last checked against, and `checked_at` when: a check that
-- finds nothing to change moves them without touching the article.
ALTER TABLE articles ADD COLUMN chapter text,
    ADD COLUMN sources jsonb,
    ADD COLUMN checked_at timestamptz,
    DROP CONSTRAINT articles_subject_valid,
    ADD CONSTRAINT articles_subject_valid CHECK (num_nonnulls(film, person, event, product, question, chapter) <= 1);
CREATE UNIQUE INDEX articles_chapter_idx ON articles (chapter) WHERE chapter IS NOT NULL;
