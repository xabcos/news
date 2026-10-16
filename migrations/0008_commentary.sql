-- 文娱 · 解说: XABC explains a film or series in an article of its own and revises it as more
-- is known. `film` is the work's key in 影视 (`catalog::work_key`), so either finds the other,
-- and one work has one article; 分享's articles have none.
ALTER TABLE articles ADD COLUMN film text;
CREATE UNIQUE INDEX articles_film_idx ON articles (film) WHERE film IS NOT NULL;
