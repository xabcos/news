-- 𝑻𝒓𝒂𝒗𝒆𝒍 gathers travelers' photographs and words from X by destination. It follows 𝑾 in
-- the navigation. A moment is a news item plus its photographs and place; a destination keeps
-- its coordinates and one guide to its history and culture, written from Wikipedia and
-- Wikivoyage. Destinations outlive their moments, so a returning place keeps its guide.
INSERT INTO topics (
    id, name, name_en, description, brief, enabled, visible, interval_minutes, window_start,
    window_start_minute, window_end, weekdays, model, reasoning_effort, max_items, lookback_hours,
    daily_run_cap, sort_weight
) VALUES (
    'travel', '𝑻𝒓𝒂𝒗𝒆𝒍', '𝑻𝒓𝒂𝒗𝒆𝒍', '世界各地旅行者在 X 上分享的旅途照片与亲身感受，以及目的地的历史文化导读',
$brief$中文：从 X 上挑选世界各地的旅途分享：在当地拍摄的照片和发帖人自己的文字，覆盖自然风光、城市街巷、建筑古迹、美食市集、节庆与当地生活，兼顾各大洲；发帖人可以是旅行者、摄影爱好者或当地居民。照片以地方为主角，清晰好看、让人想去。排除转发他人照片的聚合账号、广告、AI 生成图、以儿童为主体或私密的照片，以及正文没有写明地点的帖子。
English: Pick travel posts from across the world on X: photographs taken on the spot with the poster's own words, covering landscapes, streets, architecture and heritage, food and markets, festivals and local life on every continent, by travelers, photographers or locals. The place is the subject; photographs are clear, beautiful and make people want to go. Exclude aggregators reposting others' photographs, ads, AI-generated images, photographs centered on children or private moments, and posts whose text does not name the place.$brief$,
    true, true, 120, 0, 0, 24, '{1,2,3,4,5,6,7}', 'grok-4.7', 'medium', 30, 48, 24, 16
)
ON CONFLICT (id) DO NOTHING;

-- Destinations are Wikidata items with an English Wikivoyage guide; `voyage`, `wiki_zh` and
-- `wiki_en` name the pages the guide is written from.
CREATE TABLE travel_places (
    id text PRIMARY KEY CHECK (id ~ '^Q[1-9][0-9]{0,11}$'),
    slug text NOT NULL UNIQUE CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$' AND char_length(slug) <= 80),
    name text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 120),
    name_en text NOT NULL CHECK (char_length(name_en) BETWEEN 1 AND 120),
    country_id text NOT NULL CHECK (country_id ~ '^Q[1-9][0-9]{0,11}$'),
    country text NOT NULL,
    country_en text NOT NULL,
    latitude double precision NOT NULL CHECK (latitude BETWEEN -90 AND 90),
    longitude double precision NOT NULL CHECK (longitude BETWEEN -180 AND 180),
    voyage text NOT NULL,
    wiki_zh text,
    wiki_en text,
    guide jsonb,
    guide_at timestamptz,
    guide_attempts smallint NOT NULL DEFAULT 0,
    CONSTRAINT travel_places_guide_written CHECK ((guide IS NULL) = (guide_at IS NULL))
);
CREATE INDEX travel_places_pending_idx ON travel_places (guide_attempts) WHERE guide IS NULL;

-- `spot_id` is the place the traveler named, which may lie inside the destination. The cover is
-- the item's image, which the media refresh renews; `extra_photos` keeps the post's other
-- photographs, in order.
CREATE TABLE travel_moments (
    item_id uuid PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    place_id text NOT NULL REFERENCES travel_places(id),
    spot_id text NOT NULL CHECK (spot_id ~ '^Q[1-9][0-9]{0,11}$'),
    spot text NOT NULL,
    spot_en text NOT NULL,
    extra_photos jsonb NOT NULL
        CHECK (jsonb_typeof(extra_photos) = 'array' AND jsonb_array_length(extra_photos) <= 3),
    text text NOT NULL,
    lang text NOT NULL
);
CREATE INDEX travel_moments_place_idx ON travel_moments (place_id);
CREATE INDEX travel_moments_spot_idx ON travel_moments (spot_id);
