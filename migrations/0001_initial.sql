CREATE TABLE users (
    id BIGSERIAL PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    status BOOLEAN NOT NULL DEFAULT TRUE,
    role TEXT NOT NULL DEFAULT 'user',
    pro_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT users_email_valid CHECK (
        email = LOWER(email)
        AND email = BTRIM(email)
        AND CHAR_LENGTH(email) BETWEEN 3 AND 254
    ),
    CONSTRAINT users_role_valid CHECK (role IN ('user', 'admin'))
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
    owner_user_id BIGINT REFERENCES users(id) ON DELETE CASCADE,
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

-- Public topics have a NULL owner; personal topics are scoped to one account.
CREATE INDEX topics_owner_sort_idx
    ON topics (owner_user_id, sort_weight, created_at DESC);

-- The scheduler only needs to inspect enabled topics with a queued run.
CREATE INDEX topics_due_idx
    ON topics (next_run_at)
    WHERE enabled AND next_run_at IS NOT NULL;

-- The catalogue is intentionally tiny. Prices and durations are copied into an
-- order at creation time so a later catalogue edit cannot change an existing order.
CREATE TABLE plans (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    price_fen BIGINT NOT NULL CHECK (price_fen >= 0),
    duration_days INTEGER NOT NULL CHECK (duration_days >= 0 AND duration_days <= 3650),
    description TEXT NOT NULL DEFAULT '',
    active BOOLEAN NOT NULL DEFAULT TRUE,
    sort_weight INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT plans_code_valid CHECK (code ~ '^[a-z][a-z0-9_-]{0,31}$'),
    CONSTRAINT plans_paid_duration_valid CHECK (price_fen = 0 OR duration_days > 0)
);

INSERT INTO plans (code, name, price_fen, duration_days, description, sort_weight)
VALUES
    ('free', 'Free', 0, 0, '最多 1 个主题，最短 240 分钟抓取间隔', 10),
    ('pro', 'Pro', 9900, 31, '最多 8 个主题，最短 60 分钟抓取间隔', 20);

CREATE TABLE orders (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plan_code TEXT NOT NULL REFERENCES plans(code),
    out_trade_no TEXT NOT NULL UNIQUE,
    provider_trade_no TEXT,
    amount_fen BIGINT NOT NULL CHECK (amount_fen > 0),
    duration_days INTEGER NOT NULL CHECK (duration_days > 0),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'paid', 'closed')),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '30 minutes'),
    paid_at TIMESTAMPTZ,
    payment JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- A short lease prevents two browser tabs from creating the same YPay order
    -- while keeping the local transaction independent from the network call.
    payment_started_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX orders_user_created_idx
    ON orders (user_id, created_at DESC, id DESC);
CREATE INDEX orders_user_pending_idx
    ON orders (user_id, plan_code, created_at DESC)
    WHERE status = 'pending';

CREATE TABLE payment_events (
    id BIGSERIAL PRIMARY KEY,
    event_key TEXT NOT NULL UNIQUE,
    out_trade_no TEXT NOT NULL REFERENCES orders(out_trade_no) ON DELETE CASCADE,
    status TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX payment_events_order_idx
    ON payment_events (out_trade_no, created_at DESC);

CREATE TABLE ai_insights (
    id UUID PRIMARY KEY,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    window_start TIMESTAMPTZ NOT NULL,
    model TEXT NOT NULL,
    response_id TEXT,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    cached_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    web_search_calls INTEGER NOT NULL DEFAULT 0,
    x_search_calls INTEGER NOT NULL DEFAULT 0,
    cost_ticks BIGINT NOT NULL DEFAULT 0,
    payload JSONB,
    error TEXT,
    CONSTRAINT ai_insights_status_valid CHECK (status IN ('running', 'ok', 'failed'))
);

CREATE INDEX ai_insights_started_idx ON ai_insights (started_at DESC);
CREATE INDEX ai_insights_finished_idx ON ai_insights (status, finished_at DESC);

INSERT INTO topics (
    id, owner_user_id, name, name_en, description, brief, enabled, interval_minutes,
    window_start, window_end, weekdays, model, reasoning_effort, max_items,
    lookback_hours, daily_run_cap, sort_weight
)
VALUES
(
    'x', NULL, '𝐗', '𝐗', '全球 AI、科技与产业前沿动态',
    '中文：跟踪全球人工智能、AI 硬件、机器人、智能体、支付与跨境支付、开发者工具、科技创业和产业商业动态。重点关注大模型与产品发布、重要论文与开源项目、芯片与算力，以及 DeepSeek、Kimi、豆包、Qwen、MiniMax、百度千帆、腾讯、华为、字节、阿里等公司；关注 Apple、华为、Kindle、手机、阅读器、显示器等硬件。持续关注 xairouter.com、@hackernews、@moonbitlang、@opencode、@cursor_ai、@Cloudflare 等账号和全球主流媒体。兼顾积极的产品机会、就业信号、创作者、娱乐、审美与互联网公司动态及可信的行业趣闻。非政治导向，排除未经证实的传闻、营销转发和重复报道。
    English: Track global developments in AI, AI hardware, robotics, agents, payments, cross-border payments, developer tools, technology companies and startups. Prioritize new models and products, important research and open source projects, chips and compute, and companies such as DeepSeek, Kimi, Doubao, Qwen, MiniMax, Baidu Qianfan, Tencent, Huawei, ByteDance and Alibaba. Cover Apple, Huawei, Kindle, phones, e-readers and displays. Follow xairouter.com, @hackernews, @moonbitlang, @opencode, @cursor_ai, @Cloudflare and authoritative global media. Include constructive product, business and employment signals, creator and entertainment trends, culture and well-sourced internet stories; avoid politics, unverified rumors, promotional reposts and duplicates.',
    TRUE, 60, 0, 24, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 24, 24, 24, 10
),
(
    'openai', NULL, '𝑶𝒑𝒆𝒏𝑨𝑰', '𝑶𝒑𝒆𝒏𝑨𝑰', 'OpenAI 官方动态与产品生态',
    '中文：聚焦 OpenAI 官方动态，重点跟踪 X 平台 @OpenAI、@OpenAIDevs、@sama、@thsottiaux、@CodexReleases 等账号，以及 openai.com 的公告、博客、研究和文档。覆盖新模型与能力更新、ChatGPT、Codex、API、开发者工具、定价与配额、重要研究、合作和商业动向。优先一手来源，其次权威媒体；忽略未经证实的传闻与营销转发。面向中文读者。
English: Follow official OpenAI developments, especially @OpenAI, @OpenAIDevs, @sama, @thsottiaux and @CodexReleases on X, together with announcements, research, blogs and documentation on openai.com. Cover new models and capabilities, ChatGPT, Codex, APIs, developer tools, pricing, quotas, research, partnerships and business moves. Prefer primary sources and then authoritative reporting; ignore unverified rumors and promotional reposts. Written for Chinese readers.',
    TRUE, 60, 0, 24, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 24, 24, 24, 20
),
(
    'anthropic', NULL, '𝑨𝒏𝒕𝒉𝒓𝒐𝒑𝒊𝒄', '𝑨𝒏𝒕𝒉𝒓𝒐𝒑𝒊𝒄', 'Anthropic 与 Claude 官方动态',
    '中文：聚焦 Anthropic 与 Claude 的官方动态，重点跟踪 X 平台 @AnthropicAI、@claudeai、@ClaudeDevs、@ClaudeCodeLog 等账号及核心成员，以及 anthropic.com 的公告、博客、研究和文档。覆盖 Claude 新模型与能力、Claude Code、API、Agent SDK、开发者工具、定价与配额、安全与对齐研究、合作和商业动向。优先官方来源和权威媒体，忽略未经证实的传闻与营销转发。面向中文读者。
English: Follow official Anthropic and Claude developments, especially @AnthropicAI, @claudeai, @ClaudeDevs and @ClaudeCodeLog, core contributors, and announcements, research, blogs and documentation from anthropic.com. Cover Claude models and capabilities, Claude Code, APIs, the Agent SDK, developer tools, pricing, quotas, safety, alignment, partnerships and business moves. Prefer primary sources and authoritative reporting; ignore unverified rumors and promotional reposts. Written for Chinese readers.',
    TRUE, 60, 0, 24, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 24, 24, 24, 30
),
(
    'github', NULL, '𝑮𝒊𝒕𝑯𝒖𝒃', '𝑮𝒊𝒕𝑯𝒖𝒃', 'GitHub 热门 AI 项目与开源生态',
    '中文：关注 GitHub 热门 AI 项目、开源模型、开发框架、Agent 工具、开发者基础设施和社区趋势，以及 xairouter.com 的最新动态。跟踪 AWS Bedrock、OpenAI、Anthropic、智谱、DeepSeek、Kimi、Qwen、MiniMax、Meta 等生态，关注 X 平台 @CodexReleases、@ClaudeDevs、@ClaudeCodeLog、@moonbitlang、@opencode、@zcode_ai、@dotey、@cursor_ai、@Cloudflare 等账号。优先项目仓库、官方文档、发布记录和真实使用反馈，排除刷榜、营销和未经证实的传闻。
English: Track popular GitHub AI projects, open source models, developer frameworks, agent tools, infrastructure and community trends, together with the latest developments from xairouter.com. Follow the ecosystems around AWS Bedrock, OpenAI, Anthropic, Zhipu, DeepSeek, Kimi, Qwen, MiniMax and Meta, and posts from @CodexReleases, @ClaudeDevs, @ClaudeCodeLog, @moonbitlang, @opencode, @zcode_ai, @dotey, @cursor_ai and @Cloudflare. Prefer repositories, official documentation, release notes and real usage evidence; avoid artificial ranking, promotion and unverified claims.',
    TRUE, 60, 0, 24, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 24, 24, 24, 40
),
(
    'cases', NULL, '𝑪𝒂𝒔𝒆𝒔', '𝑪𝒂𝒔𝒆𝒔', '全球 AI 应用与商业实践案例',
    '中文：收集全球各行业真实、可复用的 AI 应用案例，覆盖医疗、教育、科研、量化研究、金融与支付、跨境支付、制造、营销、客服、内容、软件开发、个人效率和业务自动化。说明采用的模型、流程、成本、结果、限制和可迁移经验，关注对创业、收入和就业的实际启发。投资与股市内容只做事实和风险分析，不提供个性化买卖建议。优先企业公告、技术报告、客户案例和权威媒体，忽略夸大宣传与无法核实的案例。
English: Collect real and reusable AI use cases across healthcare, education, research, quantitative research, finance, payments, cross-border payments, manufacturing, marketing, support, content, software development, productivity and business automation. Explain the models, workflow, cost, results, limitations and transferable lessons, with practical implications for startups, income and employment. Treat investment and stock-market material as factual and risk analysis, never individualized trading advice. Prefer company announcements, technical reports, customer stories and authoritative reporting; ignore exaggerated or unverifiable claims.',
    TRUE, 60, 0, 24, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 24, 24, 24, 50
),
(
    'events', NULL, '𝑬𝒗𝒆𝒏𝒕𝒔', '𝑬𝒗𝒆𝒏𝒕𝒔', '全球热点、社会变化与风险事件',
    '中文：关注最近发生的全球互联网、科技与社会热点，覆盖公司与行业动态、虚拟货币、AI 支付、跨境支付、就业与裁员、灾害预警、公共安全、国际局势、军事技术和 AI 军事应用，以及俄乌、中东等重大冲突的可靠进展；必要时补充历史档案与背景资料。重点说明事实、时间线、影响范围、可信来源和可能的后续风险，不追逐情绪化标题。优先政府与机构通报、现场证据和权威媒体，忽略未经证实的传闻、煽动性内容和营销转发。
    English: Track recent global internet, technology and social events, including company and industry developments, crypto, AI payments, cross-border payments, employment and layoffs, disaster warnings, public safety, international affairs, military technology, AI in defense, and verified developments in major conflicts such as Ukraine and the Middle East; add historical archives and background when they clarify the facts. Explain facts, timelines, impact, source quality and possible follow-up risks without sensational headlines. Prefer official statements, direct evidence and authoritative reporting; ignore unverified rumors, inflammatory content and promotional reposts.',
    TRUE, 60, 0, 24, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 24, 24, 24, 60
);
