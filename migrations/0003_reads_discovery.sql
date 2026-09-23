UPDATE topics SET max_items = 4,
    brief = '精选欧美、日本、德国、法国等地区的优质原创博文、工程实践、科技文摘与公开长帖，暂不采集中文互联网媒体。优先近72小时新作，以近30天仍有价值的文章补充；重视独立观点、可复用经验和实质信息，兼顾国家、语言与来源多样性。排除泛泛而谈、重复常识、快讯、广告、转载、付费墙与登录后内容。品质优先，每轮最多发布4篇，每天最多24篇，不设最低数量，没有合格内容时允许不收录。正文与转载许可独立核验，不为凑数降低质量。'
WHERE id = 'reads' AND owner_user_id IS NULL;

-- Discovery metadata only; unlicensed source bodies are never kept here.
CREATE TABLE read_candidates (
    fingerprint bytea PRIMARY KEY CHECK (octet_length(fingerprint) = 32),
    url text NOT NULL,
    title text NOT NULL,
    source text NOT NULL,
    language text NOT NULL,
    published_at timestamptz NOT NULL,
    attempts smallint NOT NULL DEFAULT 0 CHECK (attempts BETWEEN 0 AND 3),
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    finished boolean NOT NULL DEFAULT false
);
CREATE INDEX read_candidates_pending_idx ON read_candidates (published_at DESC)
    WHERE NOT finished;

-- Refreshed in one transaction, so a failed Stats request leaves the last good list intact.
ALTER TABLE read_articles ADD COLUMN views bigint CHECK (views >= 0);
ALTER TABLE read_articles ADD COLUMN hot_rank smallint CHECK (hot_rank BETWEEN 1 AND 24);
CREATE INDEX read_articles_hot_idx ON read_articles (hot_rank) WHERE hot_rank IS NOT NULL;
