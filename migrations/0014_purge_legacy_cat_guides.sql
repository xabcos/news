-- The autonomous loop now creates life scenes from visual news locations.
-- Remove old global-news guide messages and their short-term projections so the
-- owner's journal starts with the simplified product behavior.
DELETE FROM cat_messages WHERE kind = 'guide';
DELETE FROM cat_memories WHERE kind IN ('item', 'topic', 'tag');
