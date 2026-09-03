CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    status BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT users_email_valid CHECK (
        email = LOWER(email)
        AND email = BTRIM(email)
        AND CHAR_LENGTH(email) BETWEEN 3 AND 254
    )
);

CREATE TABLE email_login_tokens (
    email TEXT NOT NULL,
    token_hash BYTEA PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT email_login_tokens_email_valid CHECK (
        email = LOWER(email)
        AND email = BTRIM(email)
        AND CHAR_LENGTH(email) BETWEEN 3 AND 254
    ),
    CONSTRAINT email_login_tokens_hash_valid CHECK (OCTET_LENGTH(token_hash) = 32)
);

CREATE INDEX email_login_tokens_email_created_idx
    ON email_login_tokens (email, created_at DESC);
-- The global send-rate window scans by time alone; without this it degrades to a seq scan.
CREATE INDEX email_login_tokens_created_idx
    ON email_login_tokens (created_at DESC);

CREATE TABLE sessions (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash BYTEA PRIMARY KEY,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT sessions_hash_valid CHECK (OCTET_LENGTH(token_hash) = 32)
);

CREATE INDEX sessions_user_idx ON sessions (user_id);
CREATE INDEX sessions_expires_idx ON sessions (expires_at);

-- A topic is one editorial direction the model researches on a schedule. Hours and
-- weekdays are interpreted in the service timezone (NEWS_TIMEZONE); weekdays use ISO
-- numbering, 1 = Monday.
CREATE TABLE topics (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    name_en TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    brief TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    interval_minutes INTEGER NOT NULL DEFAULT 60,
    window_start INTEGER NOT NULL DEFAULT 6,
    window_end INTEGER NOT NULL DEFAULT 24,
    weekdays SMALLINT[] NOT NULL DEFAULT '{1,2,3,4,5}',
    model TEXT NOT NULL DEFAULT 'grok-4.6',
    reasoning_effort TEXT NOT NULL DEFAULT 'low',
    max_items INTEGER NOT NULL DEFAULT 8,
    lookback_hours INTEGER NOT NULL DEFAULT 24,
    daily_run_cap INTEGER NOT NULL DEFAULT 24,
    sort_weight INTEGER NOT NULL DEFAULT 0,
    next_run_at TIMESTAMPTZ,
    last_run_at TIMESTAMPTZ,
    last_status TEXT,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT topics_id_valid CHECK (id ~ '^[a-z0-9][a-z0-9-]{0,31}$'),
    CONSTRAINT topics_name_valid CHECK (CHAR_LENGTH(name) BETWEEN 1 AND 60),
    CONSTRAINT topics_name_en_valid CHECK (CHAR_LENGTH(name_en) <= 60),
    CONSTRAINT topics_description_valid CHECK (CHAR_LENGTH(description) <= 200),
    CONSTRAINT topics_brief_valid CHECK (CHAR_LENGTH(brief) BETWEEN 1 AND 2000),
    CONSTRAINT topics_interval_valid CHECK (interval_minutes BETWEEN 15 AND 1440),
    CONSTRAINT topics_window_valid CHECK (
        window_start BETWEEN 0 AND 23 AND window_end BETWEEN 1 AND 24 AND window_start < window_end
    ),
    CONSTRAINT topics_weekdays_valid CHECK (
        CARDINALITY(weekdays) BETWEEN 1 AND 7 AND weekdays <@ ARRAY[1, 2, 3, 4, 5, 6, 7]::SMALLINT[]
    ),
    CONSTRAINT topics_model_valid CHECK (CHAR_LENGTH(model) BETWEEN 1 AND 64),
    CONSTRAINT topics_effort_valid CHECK (reasoning_effort IN ('low', 'medium', 'high')),
    CONSTRAINT topics_max_items_valid CHECK (max_items BETWEEN 1 AND 30),
    CONSTRAINT topics_lookback_valid CHECK (lookback_hours BETWEEN 1 AND 72),
    CONSTRAINT topics_cap_valid CHECK (daily_run_cap BETWEEN 1 AND 96)
);

-- One row per fetch attempt, with the model's usage so cost stays visible per run.
CREATE TABLE runs (
    id UUID PRIMARY KEY,
    topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    triggered_by TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    model TEXT NOT NULL,
    response_id TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    web_search_calls INTEGER NOT NULL DEFAULT 0,
    x_search_calls INTEGER NOT NULL DEFAULT 0,
    cost_ticks BIGINT NOT NULL DEFAULT 0,
    returned_count INTEGER NOT NULL DEFAULT 0,
    new_count INTEGER NOT NULL DEFAULT 0,
    dup_count INTEGER NOT NULL DEFAULT 0,
    dropped_count INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    payload JSONB,
    CONSTRAINT runs_trigger_valid CHECK (triggered_by IN ('schedule', 'manual')),
    CONSTRAINT runs_status_valid CHECK (status IN ('running', 'ok', 'failed', 'interrupted'))
);

CREATE INDEX runs_topic_started_idx ON runs (topic_id, started_at DESC);
CREATE INDEX runs_started_idx ON runs (started_at DESC);

-- Items are keyed by the local day they were first collected on, not by publication
-- date: the digest answers "what did we find today", and the same story may be reported
-- again the next day only if the model considers it a new development.
CREATE TABLE items (
    id UUID PRIMARY KEY,
    topic_id TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
    day DATE NOT NULL,
    fingerprint BYTEA NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    title_en TEXT NOT NULL DEFAULT '',
    summary_en TEXT NOT NULL DEFAULT '',
    url TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT '',
    published_at TIMESTAMPTZ,
    tags TEXT[] NOT NULL DEFAULT '{}',
    importance SMALLINT NOT NULL DEFAULT 3,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    seen_count INTEGER NOT NULL DEFAULT 1,
    first_run_id UUID REFERENCES runs(id) ON DELETE SET NULL,
    hidden BOOLEAN NOT NULL DEFAULT FALSE,
    pinned BOOLEAN NOT NULL DEFAULT FALSE,
    CONSTRAINT items_fingerprint_valid CHECK (OCTET_LENGTH(fingerprint) = 32),
    CONSTRAINT items_importance_valid CHECK (importance BETWEEN 1 AND 5),
    CONSTRAINT items_unique UNIQUE (topic_id, day, fingerprint)
);

-- Serves the public snapshot load, the admin list and the "already collected" prompt
-- context, all of which read one topic over one or two recent days.
CREATE INDEX items_topic_day_idx
    ON items (topic_id, day DESC, hidden, published_at DESC, first_seen_at DESC);
-- Archive navigation, retention and the cross-topic day list scan by day alone.
CREATE INDEX items_day_idx ON items (day DESC);
CREATE INDEX items_first_run_idx ON items (first_run_id);

INSERT INTO topics (id, name, name_en, description, brief, max_items, sort_weight)
VALUES
    (
        'ai',
        'AI 行业动态',
        'AI Industry',
        '大模型、芯片、融资与政策的每日要闻',
        '人工智能行业的最新动态：大模型与产品发布、重要论文与开源项目、芯片与算力、融资与并购、监管与政策、头部公司的重大事件。面向中文读者，侧重全球视角，同时关注中国市场。',
        8,
        10
    ),
    (
        'openai',
        'OpenAI',
        'OpenAI',
        'OpenAI 官方动态与最新模型',
        'OpenAI 的官方动态：重点关注 X 平台上 @OpenAI、@OpenAIDevs、@sama 等官方与高管账号的发布，以及 openai.com 的公告、博客与文档。覆盖新模型与能力更新、API 与产品发布、定价与配额变化、重要研究、合作与商业动向。优先一手来源（官方账号、官方博客、官方文档），其次权威媒体的报道；忽略未经证实的传闻与营销转发。面向中文读者。',
        5,
        20
    ),
    (
        'claude',
        'Claude',
        'Claude',
        'Anthropic 与 Claude 的官方动态与最新模型',
        'Anthropic 与 Claude 的官方动态：重点关注 X 平台上 @AnthropicAI、@claudeai 等官方账号及核心成员的发布，以及 anthropic.com 的公告、博客与研究。覆盖 Claude 新模型与能力更新、API、Claude Code、Agent SDK 与开发者工具发布、定价与配额变化、安全与对齐研究、合作与商业动向。优先一手来源（官方账号、官方博客、官方文档），其次权威媒体的报道；忽略未经证实的传闻与营销转发。面向中文读者。',
        5,
        30
    )
ON CONFLICT (id) DO NOTHING;
