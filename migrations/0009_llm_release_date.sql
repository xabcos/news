-- Official pages almost never print a release date beside a model, so the field stayed empty.
ALTER TABLE llm_models DROP COLUMN released_on;
