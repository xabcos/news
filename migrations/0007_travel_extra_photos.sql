-- A moment's cover is its item's image, which the media refresh renews; the moment keeps only
-- the photographs after it. Guides written before they told of people, sights, food and
-- customs are written again by the next runs.
ALTER TABLE travel_moments RENAME COLUMN photos TO extra_photos;
UPDATE travel_moments SET extra_photos = extra_photos - 0;
ALTER TABLE travel_moments DROP CONSTRAINT travel_moments_photos_check;
ALTER TABLE travel_moments ADD CONSTRAINT travel_moments_extra_photos_check
    CHECK (jsonb_typeof(extra_photos) = 'array' AND jsonb_array_length(extra_photos) <= 3);
UPDATE travel_places SET guide = NULL, guide_at = NULL, guide_attempts = 0;
