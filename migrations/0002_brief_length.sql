-- A topic's brief may run to 8192 characters, room for a broad direction with its sources.
ALTER TABLE topics DROP CONSTRAINT topics_brief_valid,
    ADD CONSTRAINT topics_brief_valid CHECK (char_length(brief) BETWEEN 1 AND 8192);
