-- 学习: XABC's courses. A lesson is an article in a track (`course`), at its place in the track's
-- order (`lesson`), under the chapter it names; it keeps the subjects news about it would name
-- (`keywords`) and when it is next reviewed (`review_at`). 分享 and 解说 have none of these.
ALTER TABLE articles
    ADD COLUMN course text,
    ADD COLUMN chapter text,
    ADD COLUMN lesson integer,
    ADD COLUMN keywords text[],
    ADD COLUMN review_at timestamptz,
    ADD CONSTRAINT articles_lesson_valid CHECK (
        (course IS NULL) = (chapter IS NULL) AND (course IS NULL) = (lesson IS NULL)
        AND (course IS NULL) = (keywords IS NULL) AND (course IS NULL) = (review_at IS NULL)
        AND (course IS NULL OR film IS NULL)
        AND lesson > 0 AND char_length(chapter) BETWEEN 1 AND 40
    );
CREATE UNIQUE INDEX articles_lesson_idx ON articles (course, lesson) WHERE course IS NOT NULL;
