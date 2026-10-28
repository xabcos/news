-- 时代: XABC profiles a person people talk about in an article of its own and brings it up to
-- date when they are talked about again. `person` is their key (`xabc::people::key`), so one
-- person has one article.
ALTER TABLE articles ADD COLUMN person text;
CREATE UNIQUE INDEX articles_person_idx ON articles (person) WHERE person IS NOT NULL;
