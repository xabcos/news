-- The travel companion is now a girl. Keep the historical table for storage
-- compatibility, but make any legacy display name consistent with the new product.
UPDATE cats
SET name = '旅行女孩', name_updated_at = NULL
WHERE name = '旅行猫咪';
