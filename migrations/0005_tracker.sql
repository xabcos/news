-- 追踪: XABC follows a hot event in an article of its own and brings it up to date as the event
-- develops. `event` is its key (`xabc::key` of the slug the searches give it), so one event has
-- one article; an article explains a work, profiles a person or follows an event, one at most.
ALTER TABLE articles ADD COLUMN event text,
    ADD CONSTRAINT articles_subject_valid CHECK (num_nonnulls(film, person, event) <= 1);
CREATE UNIQUE INDEX articles_event_idx ON articles (event) WHERE event IS NOT NULL;
