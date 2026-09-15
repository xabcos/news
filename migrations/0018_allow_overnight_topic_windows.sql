ALTER TABLE topics
    DROP CONSTRAINT topics_window_valid;

ALTER TABLE topics
    ADD CONSTRAINT topics_window_valid CHECK (
        window_start BETWEEN 0 AND 23
        AND window_end BETWEEN 1 AND 24
        AND window_start <> window_end
    );
