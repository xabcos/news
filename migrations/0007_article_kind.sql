-- An article is of one kind (articles::Kind): 分享's `share`, or of a line that gives it a
-- subject, kept in `subject` where six columns each held one line's: `film`, a 解说, the work's
-- key in 影视; `person`, a 时代 profile, `xabc::key` of the English name, after `company:` for
-- a company; `event`, a 追踪 article, `xabc::key` of the event's slug; `product`, a 评测, the
-- product's `catalog_entries.key`; `question`, a 调研, the question as asked; `chapter`, a
-- chapter of the Codex & Claude handbook, its key. A new line adds a kind, not a column.
ALTER TABLE articles ADD COLUMN kind text NOT NULL DEFAULT 'share', ADD COLUMN subject text;
UPDATE articles SET
    kind = CASE WHEN film IS NOT NULL THEN 'film' WHEN person IS NOT NULL THEN 'person'
                WHEN event IS NOT NULL THEN 'event' WHEN product IS NOT NULL THEN 'product'
                WHEN question IS NOT NULL THEN 'question' WHEN chapter IS NOT NULL THEN 'chapter'
                ELSE 'share' END,
    subject = COALESCE(film, person, event, product, question, chapter);
ALTER TABLE articles
    DROP CONSTRAINT articles_subject_valid,
    DROP COLUMN film, DROP COLUMN person, DROP COLUMN event, DROP COLUMN product,
    DROP COLUMN question, DROP COLUMN chapter,
    ADD CONSTRAINT articles_kind_valid
        CHECK (kind IN ('share', 'film', 'person', 'event', 'product', 'question', 'chapter')),
    ADD CONSTRAINT articles_subject_valid CHECK ((kind = 'share') = (subject IS NULL));
-- One subject has one article, but a question may be asked twice.
CREATE UNIQUE INDEX articles_subject_idx ON articles (kind, subject)
    WHERE kind NOT IN ('share', 'question');
