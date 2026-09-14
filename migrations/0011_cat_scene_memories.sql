-- Scene snapshots are durable memories even when they are not tied to a news topic.
ALTER TABLE cat_memories DROP CONSTRAINT cat_memories_kind_check;
ALTER TABLE cat_memories
    ADD CONSTRAINT cat_memories_kind_check
    CHECK (kind IN ('item', 'topic', 'tag', 'place', 'scene'));
