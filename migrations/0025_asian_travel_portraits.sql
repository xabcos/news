-- The visual direction is now explicitly adult East Asian. Preserve immutable
-- identities and history; generate a new portrait without referencing earlier art.
UPDATE travel_autonomy_state a
SET next_scene_at = NOW(), updated_at = NOW()
WHERE NOT EXISTS (
    SELECT 1 FROM travel_postcards p
    WHERE p.source_key LIKE 'travel:' || a.girl_id::text || ':%-asian-v1'
      AND p.provider = 'grok_image'
);
