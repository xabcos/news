-- First photos should be queued as soon as an identity is confirmed.
ALTER TABLE travel_autonomy_state ALTER COLUMN next_scene_at SET DEFAULT NOW();

UPDATE travel_autonomy_state a
SET next_scene_at = NOW()
WHERE a.next_scene_at > NOW()
  AND NOT EXISTS (
      SELECT 1 FROM travel_postcards p
      WHERE p.source_key LIKE 'travel:' || a.girl_id::text || ':%'
  );
