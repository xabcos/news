-- Four independent collections, presented under one Culture navigation entry.
INSERT INTO topics (id, name, name_en, description, brief, enabled, visible,
    interval_minutes, window_start, window_end, weekdays, model, reasoning_effort,
    max_items, lookback_hours, daily_run_cap, sort_weight)
SELECT id, name, name, description, brief, true, true, 720, 0, 24,
    '{1,2,3,4,5,6,7}'::smallint[], 'grok-4.7', 'medium', 8, 48, 4, weight
FROM (VALUES
    ('movie', '𝑴𝒐𝒗𝒊𝒆', '全球电影、剧集与动画的新作、预告、上映和创作故事',
     '关注全球电影、剧集和动画：新预告、定档、上映、电影节与奖项、主创访谈、创作幕后和有实质内容的评论。兼顾各地区与独立作品，以作品为中心，排除明星私生活、营销水军和无来源的传闻。', 17),
    ('music', '𝑴𝒖𝒔𝒊𝒄', '全球音乐的新专辑、单曲、演出和音乐人故事',
     '关注全球新专辑、单曲、音乐现场、巡演、音乐奖项和音乐人的创作访谈。兼顾不同语言、风格与独立音乐，以作品和音乐本身为中心，排除粉丝争吵和明星私生活。', 18),
    ('game', '𝑮𝒂𝒎𝒆', '全球游戏的新作、发售、更新与开发故事',
     '关注全球 PC、主机和移动游戏的新作、预告、发售、重要更新、DLC、独立游戏和开发者故事。突出真实玩法和平台信息，区分正式发布与传闻，避免重复促销和无实质内容的热议。', 19),
    ('book', '𝑩𝒐𝒐𝒌', '全球值得关注的新书、文学奖、再版与阅读讨论',
     '全品类精选全球值得关注的书：文学、人文、历史、社科、科普、艺术、科技和商业。发现新书、获奖、再版、新译本、改编与作者访谈，也关注因新事件重获关注的旧书。每本书须有具体的近期关注理由，不以销量或技术话题限制范围，不收录空泛励志书单与营销推荐。', 21)
) AS seed(id, name, description, brief, weight)
ON CONFLICT (id) DO NOTHING;

-- Draft references stay private until the source-based guide has been written.
CREATE TABLE culture_works (
    id uuid PRIMARY KEY,
    topic_id text NOT NULL REFERENCES topics(id),
    slug text NOT NULL,
    name text NOT NULL,
    name_en text NOT NULL,
    creator text NOT NULL,
    kind text NOT NULL,
    source_url text NOT NULL,
    source_urls jsonb NOT NULL,
    image jsonb,
    guide jsonb,
    source_hash bytea,
    guide_at timestamptz,
    checked_at timestamptz,
    attempts smallint NOT NULL DEFAULT 0,
    UNIQUE (topic_id, slug),
    UNIQUE (topic_id, source_url),
    CHECK (topic_id IN ('movie', 'music', 'game', 'book'))
);
CREATE TABLE culture_items (
    item_id uuid PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    work_id uuid NOT NULL REFERENCES culture_works(id)
);
CREATE INDEX culture_items_work_idx ON culture_items(work_id);

CREATE INDEX items_culture_order_idx ON items (topic_id, first_seen_at DESC, id DESC)
    WHERE NOT hidden AND topic_id IN ('movie', 'music', 'game', 'book');
