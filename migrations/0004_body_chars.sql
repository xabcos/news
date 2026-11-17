-- An article's body may be up to `articles::BODY_CHARS` characters, three times what it was.
ALTER TABLE articles DROP CONSTRAINT articles_body_valid,
    ADD CONSTRAINT articles_body_valid CHECK (char_length(body) <= 960000);
