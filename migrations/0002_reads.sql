INSERT INTO topics (
    id, name, name_en, description, brief, interval_minutes, window_start, window_end,
    weekdays, reasoning_effort, max_items, lookback_hours, daily_run_cap, sort_weight
) VALUES (
    'reads', '𝑹𝒆𝒂𝒅𝒔', '𝑹𝒆𝒂𝒅𝒔', '值得读完的文章与观点',
    '精选 AI、科技、商业领域有阅读价值的原创博文、经验复盘、方法论和公开长帖，兼收特别优质的其他内容。用 X、Hacker News 等热门社区发现线索，回到作者原文、博客或公开帖子核实。热度必须有可核实的传播或讨论依据，不猜测互动数字。优先有独立观点、具体经验和实质信息的内容；排除快讯、广告、营销、付费墙、登录后内容和重复文章。每次最多推荐两篇，摘要说明为什么值得读。没有合格内容时返回空数组。正文与转载许可由后续流程独立核验。',
    64, 0, 24, '{1,2,3,4,5,6,7}', 'high', 2, 72, 24, 15
);

-- A reading keeps its original URL identity across dates and survives news retention.
CREATE UNIQUE INDEX items_reads_source_idx ON items (fingerprint) WHERE topic_id = 'reads';

-- Serve the list in display order and bound related-article discovery by recency.
CREATE INDEX items_reads_order_idx ON items (pinned DESC, first_seen_at DESC, id DESC)
    WHERE topic_id = 'reads' AND NOT hidden;
CREATE INDEX items_reads_recent_idx ON items (first_seen_at DESC, id DESC)
    WHERE topic_id = 'reads' AND NOT hidden;

CREATE TABLE read_articles (
    item_id uuid PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    mode text NOT NULL CHECK (mode IN ('full', 'brief')),
    author text NOT NULL,
    source_title text NOT NULL,
    source_language text NOT NULL,
    body text NOT NULL CHECK (char_length(body) BETWEEN 200 AND 80000),
    body_en text NOT NULL DEFAULT '' CHECK (char_length(body_en) <= 80000),
    source_hash bytea NOT NULL CHECK (octet_length(source_hash) = 32),
    license_name text,
    license_url text,
    fetched_at timestamptz NOT NULL,
    CHECK ((mode = 'full' AND license_name IS NOT NULL AND license_url IS NOT NULL)
        OR (mode = 'brief' AND license_name IS NULL AND license_url IS NULL))
);
