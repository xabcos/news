-- 数码 · 评测: XABC reviews a product 数码 collects in an article of its own and brings it up to
-- date as new reviews come. `product` is the product's key in 数码 (`catalog_entries.key`, brand
-- and model), so one product has one review wherever 数码 files it.
ALTER TABLE articles ADD COLUMN product text,
    DROP CONSTRAINT articles_subject_valid,
    ADD CONSTRAINT articles_subject_valid CHECK (num_nonnulls(film, person, event, product) <= 1);
CREATE UNIQUE INDEX articles_product_idx ON articles (product) WHERE product IS NOT NULL;
