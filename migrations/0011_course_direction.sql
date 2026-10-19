-- An administrator turns a course with a sentence (`direction`, given at `adjusted_at`); XABC
-- rewrites its brief to it, and lessons written before it are revised to the new brief.
ALTER TABLE courses
    ADD COLUMN direction text NOT NULL DEFAULT '',
    ADD COLUMN adjusted_at timestamptz;
