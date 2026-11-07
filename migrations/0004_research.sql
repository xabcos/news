-- 调研: readers ask XABC to look something up, and the article it writes goes out under their
-- pen name. A research article keeps the question it answers, as a 解说 keeps its work, though
-- one question may be asked twice.
ALTER TABLE articles ADD COLUMN question text,
    DROP CONSTRAINT articles_subject_valid,
    ADD CONSTRAINT articles_subject_valid CHECK (num_nonnulls(film, person, event, product, question) <= 1);
-- The research the author asked for on `day`.
ALTER TABLE authors ADD COLUMN research smallint NOT NULL DEFAULT 0;
-- The questions waiting: the one being written first, then Pro before Free, each in the order
-- asked. A question leaves when its article, which takes its id, is written, or when its reader
-- takes it back; one that could not be written stays, failed, until its reader clears it.
CREATE TABLE research (
    id uuid PRIMARY KEY,
    author_id bigint NOT NULL REFERENCES authors(user_id) ON DELETE CASCADE,
    question text NOT NULL CONSTRAINT research_question_valid CHECK (char_length(question) BETWEEN 1 AND 1000),
    -- The line it waits in: Pro, or an administrator, as it was asked.
    pro boolean NOT NULL,
    -- Whether its article goes public; only Pro may keep one to themselves.
    public boolean NOT NULL,
    -- The site's date its asker's allowance counted it on, which taking it back or failing gives
    -- back; none for an administrator's.
    day date,
    asked_at timestamptz NOT NULL DEFAULT now(),
    started_at timestamptz,
    failed boolean NOT NULL DEFAULT false
);
