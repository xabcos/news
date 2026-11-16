-- An article an administrator asks XABC to bring up to date waits in 调研's queue like a
-- question, what they ask in `question`; an article waits there once.
ALTER TABLE research ADD COLUMN article uuid UNIQUE REFERENCES articles(id) ON DELETE CASCADE;
