-- An article's body may run to 320,000 characters (`articles::BODY_CHARS`).
ALTER TABLE articles DROP CONSTRAINT articles_body_valid,
    ADD CONSTRAINT articles_body_valid CHECK (char_length(body) <= 320000);
