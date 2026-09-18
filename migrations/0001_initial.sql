-- NEWS core schema for a fresh deployment.
-- This file creates the complete NEWS schema and public topic catalog.

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SET check_function_bodies = false;

-- Name: maintain_user_metrics(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.maintain_user_metrics() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE user_metrics
        SET registered_count = registered_count + 1
        WHERE id = TRUE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'user_metrics singleton row is missing';
        END IF;
    ELSIF TG_OP = 'DELETE' THEN
        UPDATE user_metrics
        SET registered_count = registered_count - 1
        WHERE id = TRUE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'user_metrics singleton row is missing';
        END IF;
    ELSIF TG_OP = 'TRUNCATE' THEN
        UPDATE user_metrics
        SET registered_count = 0
        WHERE id = TRUE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'user_metrics singleton row is missing';
        END IF;
    END IF;
    RETURN NULL;
END;
$$;


--

-- Name: ai_insights; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_insights (
    id uuid NOT NULL,
    status text NOT NULL,
    started_at timestamp with time zone NOT NULL,
    finished_at timestamp with time zone,
    window_start timestamp with time zone NOT NULL,
    model text NOT NULL,
    response_id text,
    input_tokens integer DEFAULT 0 NOT NULL,
    cached_tokens integer DEFAULT 0 NOT NULL,
    output_tokens integer DEFAULT 0 NOT NULL,
    reasoning_tokens integer DEFAULT 0 NOT NULL,
    web_search_calls integer DEFAULT 0 NOT NULL,
    x_search_calls integer DEFAULT 0 NOT NULL,
    cost_ticks bigint DEFAULT 0 NOT NULL,
    payload jsonb,
    error text,
    CONSTRAINT ai_insights_status_valid CHECK ((status = ANY (ARRAY['running'::text, 'ok'::text, 'failed'::text])))
);


--

-- Name: email_login_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.email_login_tokens (
    email text NOT NULL,
    token_hash bytea NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT email_login_tokens_email_valid CHECK (((email = lower(email)) AND (email = btrim(email)) AND ((char_length(email) >= 3) AND (char_length(email) <= 254)))),
    CONSTRAINT email_login_tokens_hash_valid CHECK ((octet_length(token_hash) = 32))
);


--

-- Name: items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.items (
    id uuid NOT NULL,
    topic_id text NOT NULL,
    day date NOT NULL,
    fingerprint bytea NOT NULL,
    title text NOT NULL,
    summary text NOT NULL,
    title_en text DEFAULT ''::text NOT NULL,
    summary_en text DEFAULT ''::text NOT NULL,
    url text NOT NULL,
    source text DEFAULT ''::text NOT NULL,
    published_at timestamp with time zone,
    tags text[] DEFAULT '{}'::text[] NOT NULL,
    importance smallint DEFAULT 3 NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    seen_count integer DEFAULT 1 NOT NULL,
    first_run_id uuid,
    hidden boolean DEFAULT false NOT NULL,
    pinned boolean DEFAULT false NOT NULL,
    tags_en text[] DEFAULT '{}'::text[] NOT NULL,
    image jsonb,
    CONSTRAINT items_fingerprint_valid CHECK ((octet_length(fingerprint) = 32)),
    CONSTRAINT items_importance_valid CHECK (((importance >= 1) AND (importance <= 5)))
);


--

-- Name: maintenance_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.maintenance_state (
    task text NOT NULL,
    last_completed_day date NOT NULL
);


--

-- Name: orders; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.orders (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    plan_code text NOT NULL,
    out_trade_no text NOT NULL,
    provider_trade_no text,
    amount_fen bigint NOT NULL,
    duration_days integer NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    expires_at timestamp with time zone DEFAULT (now() + '01:00:00'::interval) NOT NULL,
    paid_at timestamp with time zone,
    payment jsonb DEFAULT '{}'::jsonb NOT NULL,
    payment_started_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT orders_amount_fen_check CHECK ((amount_fen > 0)),
    CONSTRAINT orders_duration_days_check CHECK ((duration_days > 0)),
    CONSTRAINT orders_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'paid'::text, 'closed'::text])))
);


--

-- Name: orders_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.orders_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--

-- Name: orders_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.orders_id_seq OWNED BY public.orders.id;


--

-- Name: payment_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.payment_events (
    id bigint NOT NULL,
    event_key text NOT NULL,
    out_trade_no text NOT NULL,
    status text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    user_id bigint NOT NULL
);


--

-- Name: payment_events_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.payment_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--

-- Name: payment_events_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.payment_events_id_seq OWNED BY public.payment_events.id;


--

-- Name: plans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plans (
    code text NOT NULL,
    name text NOT NULL,
    price_fen bigint NOT NULL,
    duration_days integer NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    active boolean DEFAULT true NOT NULL,
    sort_weight integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT plans_code_valid CHECK ((code ~ '^[a-z][a-z0-9_-]{0,31}$'::text)),
    CONSTRAINT plans_duration_days_check CHECK (((duration_days >= 0) AND (duration_days <= 3650))),
    CONSTRAINT plans_paid_duration_valid CHECK (((price_fen = 0) OR (duration_days > 0))),
    CONSTRAINT plans_price_fen_check CHECK ((price_fen >= 0))
);


--

-- Name: runs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.runs (
    id uuid NOT NULL,
    topic_id text NOT NULL,
    triggered_by text NOT NULL,
    status text NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    model text NOT NULL,
    response_id text,
    input_tokens integer DEFAULT 0 NOT NULL,
    cached_tokens integer DEFAULT 0 NOT NULL,
    output_tokens integer DEFAULT 0 NOT NULL,
    reasoning_tokens integer DEFAULT 0 NOT NULL,
    web_search_calls integer DEFAULT 0 NOT NULL,
    x_search_calls integer DEFAULT 0 NOT NULL,
    cost_ticks bigint DEFAULT 0 NOT NULL,
    returned_count integer DEFAULT 0 NOT NULL,
    new_count integer DEFAULT 0 NOT NULL,
    dup_count integer DEFAULT 0 NOT NULL,
    dropped_count integer DEFAULT 0 NOT NULL,
    error text,
    payload jsonb,
    CONSTRAINT runs_status_valid CHECK ((status = ANY (ARRAY['running'::text, 'ok'::text, 'failed'::text, 'interrupted'::text]))),
    CONSTRAINT runs_trigger_valid CHECK ((triggered_by = ANY (ARRAY['schedule'::text, 'manual'::text])))
);


--

-- Name: sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sessions (
    user_id bigint NOT NULL,
    token_hash bytea NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT sessions_hash_valid CHECK ((octet_length(token_hash) = 32))
);


--

-- Name: topics; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.topics (
    id text NOT NULL,
    owner_user_id bigint,
    name text NOT NULL,
    name_en text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    brief text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    interval_minutes integer DEFAULT 60 NOT NULL,
    window_start integer DEFAULT 6 NOT NULL,
    window_end integer DEFAULT 24 NOT NULL,
    weekdays smallint[] DEFAULT '{1,2,3,4,5}'::smallint[] NOT NULL,
    model text DEFAULT 'grok-4.6'::text NOT NULL,
    reasoning_effort text DEFAULT 'low'::text NOT NULL,
    max_items integer DEFAULT 8 NOT NULL,
    lookback_hours integer DEFAULT 24 NOT NULL,
    daily_run_cap integer DEFAULT 24 NOT NULL,
    sort_weight integer DEFAULT 0 NOT NULL,
    next_run_at timestamp with time zone,
    last_run_at timestamp with time zone,
    last_status text,
    consecutive_failures integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    feed_revision bigint DEFAULT 0 NOT NULL,
    CONSTRAINT topics_brief_valid CHECK (((char_length(brief) >= 1) AND (char_length(brief) <= 2000))),
    CONSTRAINT topics_cap_valid CHECK (((daily_run_cap >= 1) AND (daily_run_cap <= 96))),
    CONSTRAINT topics_description_valid CHECK ((char_length(description) <= 200)),
    CONSTRAINT topics_effort_valid CHECK ((reasoning_effort = ANY (ARRAY['low'::text, 'medium'::text, 'high'::text]))),
    CONSTRAINT topics_id_valid CHECK ((id ~ '^[a-z0-9][a-z0-9-]{0,31}$'::text)),
    CONSTRAINT topics_interval_valid CHECK (((interval_minutes >= 15) AND (interval_minutes <= 1440))),
    CONSTRAINT topics_lookback_valid CHECK (((lookback_hours >= 1) AND (lookback_hours <= 72))),
    CONSTRAINT topics_max_items_valid CHECK (((max_items >= 1) AND (max_items <= 30))),
    CONSTRAINT topics_model_valid CHECK (((char_length(model) >= 1) AND (char_length(model) <= 64))),
    CONSTRAINT topics_name_en_valid CHECK ((char_length(name_en) <= 60)),
    CONSTRAINT topics_name_valid CHECK (((char_length(name) >= 1) AND (char_length(name) <= 60))),
    CONSTRAINT topics_weekdays_valid CHECK (((cardinality(weekdays) >= 1) AND (cardinality(weekdays) <= 7) AND (weekdays <@ ARRAY[(1)::smallint, (2)::smallint, (3)::smallint, (4)::smallint, (5)::smallint, (6)::smallint, (7)::smallint]))),
    CONSTRAINT topics_window_valid CHECK (((window_start >= 0) AND (window_start <= 23) AND ((window_end >= 1) AND (window_end <= 24)) AND (window_start <> window_end)))
);


--

-- Name: user_metrics; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_metrics (
    id boolean DEFAULT true NOT NULL,
    registered_count bigint NOT NULL,
    CONSTRAINT user_metrics_id_check CHECK ((id = true)),
    CONSTRAINT user_metrics_registered_count_check CHECK ((registered_count >= 0))
);


--

-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id bigint NOT NULL,
    email text NOT NULL,
    status boolean DEFAULT true NOT NULL,
    role text DEFAULT 'user'::text NOT NULL,
    pro_until timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_login_at timestamp with time zone,
    CONSTRAINT users_email_valid CHECK (((email = lower(email)) AND (email = btrim(email)) AND ((char_length(email) >= 3) AND (char_length(email) <= 254)))),
    CONSTRAINT users_role_valid CHECK ((role = ANY (ARRAY['user'::text, 'admin'::text])))
);


--

-- Name: users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--

-- Name: users_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.users_id_seq OWNED BY public.users.id;


--

-- Name: orders id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.orders ALTER COLUMN id SET DEFAULT nextval('public.orders_id_seq'::regclass);


--

-- Name: payment_events id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payment_events ALTER COLUMN id SET DEFAULT nextval('public.payment_events_id_seq'::regclass);


--

-- Name: users id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users ALTER COLUMN id SET DEFAULT nextval('public.users_id_seq'::regclass);


--

-- Name: ai_insights ai_insights_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_insights
    ADD CONSTRAINT ai_insights_pkey PRIMARY KEY (id);


--

-- Name: email_login_tokens email_login_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_login_tokens
    ADD CONSTRAINT email_login_tokens_pkey PRIMARY KEY (token_hash);


--

-- Name: items items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_pkey PRIMARY KEY (id);


--

-- Name: items items_unique; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_unique UNIQUE (topic_id, day, fingerprint);


--

-- Name: maintenance_state maintenance_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.maintenance_state
    ADD CONSTRAINT maintenance_state_pkey PRIMARY KEY (task);


--

-- Name: orders orders_out_trade_no_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.orders
    ADD CONSTRAINT orders_out_trade_no_key UNIQUE (out_trade_no);


--

-- Name: orders orders_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.orders
    ADD CONSTRAINT orders_pkey PRIMARY KEY (id);


--

-- Name: payment_events payment_events_event_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payment_events
    ADD CONSTRAINT payment_events_event_key_key UNIQUE (event_key);


--

-- Name: payment_events payment_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payment_events
    ADD CONSTRAINT payment_events_pkey PRIMARY KEY (id);


--

-- Name: plans plans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plans
    ADD CONSTRAINT plans_pkey PRIMARY KEY (code);


--

-- Name: runs runs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_pkey PRIMARY KEY (id);


--

-- Name: sessions sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_pkey PRIMARY KEY (token_hash);


--

-- Name: topics topics_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topics
    ADD CONSTRAINT topics_pkey PRIMARY KEY (id);


--

-- Name: user_metrics user_metrics_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_metrics
    ADD CONSTRAINT user_metrics_pkey PRIMARY KEY (id);


--

-- Name: users users_email_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_email_key UNIQUE (email);


--

-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);


--

-- Name: ai_insights_finished_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_insights_finished_idx ON public.ai_insights USING btree (status, finished_at DESC);


--

-- Name: ai_insights_started_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_insights_started_idx ON public.ai_insights USING btree (started_at DESC);


--

-- Name: email_login_tokens_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX email_login_tokens_created_idx ON public.email_login_tokens USING btree (created_at DESC);


--

-- Name: email_login_tokens_email_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX email_login_tokens_email_created_idx ON public.email_login_tokens USING btree (email, created_at DESC);


--

-- Name: items_day_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_day_idx ON public.items USING btree (day DESC);


--

-- Name: items_first_run_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_first_run_idx ON public.items USING btree (first_run_id);


--

-- Name: items_first_seen_visible_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_first_seen_visible_idx ON public.items USING btree (first_seen_at DESC, topic_id) WHERE (NOT hidden);


--

-- Name: items_topic_day_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_topic_day_idx ON public.items USING btree (topic_id, day DESC, hidden, published_at DESC, first_seen_at DESC);


--

-- Name: orders_cleanup_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX orders_cleanup_idx ON public.orders USING btree (created_at, id) WHERE ((status = ANY (ARRAY['pending'::text, 'closed'::text])) AND (paid_at IS NULL));


--

-- Name: orders_user_created_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX orders_user_created_idx ON public.orders USING btree (user_id, created_at DESC, id DESC);


--

-- Name: orders_user_pending_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX orders_user_pending_idx ON public.orders USING btree (user_id, plan_code, created_at DESC) WHERE (status = 'pending'::text);


--

-- Name: payment_events_order_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX payment_events_order_idx ON public.payment_events USING btree (out_trade_no, created_at DESC);


--

-- Name: runs_started_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX runs_started_idx ON public.runs USING btree (started_at DESC);


--

-- Name: runs_topic_started_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX runs_topic_started_idx ON public.runs USING btree (topic_id, started_at DESC);


--

-- Name: sessions_expires_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX sessions_expires_idx ON public.sessions USING btree (expires_at);


--

-- Name: sessions_user_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX sessions_user_idx ON public.sessions USING btree (user_id);


--

-- Name: topics_due_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX topics_due_idx ON public.topics USING btree (next_run_at) WHERE (enabled AND (next_run_at IS NOT NULL));


--

-- Name: topics_owner_sort_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX topics_owner_sort_idx ON public.topics USING btree (owner_user_id, sort_weight, created_at DESC);


--

-- Name: users_active_last_login_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_active_last_login_idx ON public.users USING btree (last_login_at DESC) WHERE (status AND (last_login_at IS NOT NULL));


--

-- Name: users users_metrics_count; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER users_metrics_count AFTER INSERT OR DELETE ON public.users FOR EACH ROW EXECUTE FUNCTION public.maintain_user_metrics();


--

-- Name: users users_metrics_truncate; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER users_metrics_truncate AFTER TRUNCATE ON public.users FOR EACH STATEMENT EXECUTE FUNCTION public.maintain_user_metrics();


--

-- Name: items items_first_run_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_first_run_id_fkey FOREIGN KEY (first_run_id) REFERENCES public.runs(id) ON DELETE SET NULL;


--

-- Name: items items_topic_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_topic_id_fkey FOREIGN KEY (topic_id) REFERENCES public.topics(id) ON DELETE CASCADE;


--

-- Name: orders orders_plan_code_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.orders
    ADD CONSTRAINT orders_plan_code_fkey FOREIGN KEY (plan_code) REFERENCES public.plans(code);


--

-- Name: orders orders_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.orders
    ADD CONSTRAINT orders_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--

-- Name: payment_events payment_events_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.payment_events
    ADD CONSTRAINT payment_events_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--

-- Name: runs runs_topic_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.runs
    ADD CONSTRAINT runs_topic_id_fkey FOREIGN KEY (topic_id) REFERENCES public.topics(id) ON DELETE CASCADE;


--

-- Name: sessions sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--

-- Name: topics topics_owner_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topics
    ADD CONSTRAINT topics_owner_user_id_fkey FOREIGN KEY (owner_user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--


-- Initial catalog and public editorial topics. Runtime timestamps and run state
-- are deliberately left empty so the scheduler initializes them on first boot.
INSERT INTO plans (code, name, price_fen, duration_days, description, sort_weight) VALUES
    ('free', 'Free', 0, 0, '可阅读公共主题，新建个人抓取任务需 Pro', 10),
    ('pro', 'Pro', 9900, 31, '最多 4 个主题，最短 64 分钟抓取间隔', 20);

INSERT INTO user_metrics (id, registered_count) VALUES (TRUE, 0);

INSERT INTO topics (
    id, owner_user_id, name, name_en, description, brief, enabled, interval_minutes,
    window_start, window_end, weekdays, model, reasoning_effort, max_items,
    lookback_hours, daily_run_cap, sort_weight
) VALUES
    ('x', NULL, '𝐗', '𝐗', '全球 AI、科技产品与产业趋势', '中文：跟踪全球人工智能、AI 硬件、机器人、智能体、开发者工具、支付与科技公司的最新事实。优先新模型与产品、重要论文与开源项目、芯片与算力、价格变化、真实采用和明确的商业或就业信号。覆盖全球公司与权威媒体，但 OpenAI、Anthropic、中国 AI、GitHub 项目、Codex 故障和具体应用案例以专门主题为主；本栏只收录跨公司、跨行业或具有明显产业影响的进展。排除传闻、营销稿、重复报道、无来源 benchmark 和泛泛观点。 English: Track verifiable global developments in AI, hardware, robotics, agents, developer tools, payments and technology companies. Prioritize model and product releases, important research and open source work, chips and compute, pricing, real adoption, business impact and employment signals. Use this as the cross-industry overview; leave OpenAI, Anthropic, China AI, GitHub projects, Codex incidents and detailed use cases to their dedicated topics. Exclude rumors, marketing copy, duplicate coverage, unsupported benchmarks and generic commentary.', TRUE, 64, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 24, 36, 96, 0),
    ('p', NULL, '𝑷', '𝑷', '全球热点图片、X 视觉内容与公开影像趋势', '中文：收集最近 24 小时有明确背景的公开视觉内容，分为新闻现场、带图片且有传播证据的公开 X 帖子、以及有地点时间或传播背景的公开影像趋势。每条必须同时有可核实的文章或帖子 URL 和直接对应的 HTTPS 图片；优先官方、现场、原作者或权威媒体。允许 AI 生成图，但必须明确标注 AI generated，不能冒充现场照片。排除占位图、广告、图库包装、拼图、血腥、偷拍、私密、露骨和未成年人性化内容；没有可靠图片就不要返回。 English: Collect verifiable public visual stories from the last 24 hours: news scenes, public X posts with an image and clear circulation evidence, and photo trends with a verified place, time or context. Every item needs a matching article or post URL and a directly usable HTTPS image. Prefer official, eyewitness, original-author and authoritative-media sources. Label AI-generated images clearly and never present them as on-site photography. Exclude placeholders, ads, stock-image packaging, collages, graphic violence, voyeuristic, private, explicit or sexualized-minor content.', TRUE, 64, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 30, 24, 96, 15),
    ('china-ai', NULL, '𝑪', '𝑪', '中国 AI 模型、开源项目与应用生态', '中文：只关注中国大陆 AI 模型公司、开源社区、开发平台和应用生态的可核实进展。覆盖模型与版本、推理和代码能力、多模态、Agent、API 与价格、开源权重、技术报告、部署框架、开发者采用、企业落地和重要合作。优先厂商官网、博客、文档、GitHub、Hugging Face、技术报告和可信媒体，X 只作及时线索并尽量回到一手来源核实。排除刷榜、营销稿、无来源 benchmark、传闻、重复报道和泛泛行业新闻。 English: Cover only verifiable developments in China-based AI models, open-source communities, developer platforms and applications. Prioritize model releases, capability changes, multimodal and agent systems, APIs and pricing, open weights, technical reports, deployment frameworks, developer adoption, enterprise use and major partnerships. Prefer official sites, documentation, repositories, reports and reliable media; use X as a lead and corroborate with primary sources. Exclude artificial rankings, marketing copy, unsupported benchmarks, rumors, duplicates and generic industry news.', TRUE, 64, 5, 20, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 24, 24, 24, 18),
    ('openai', NULL, '𝑶𝒑𝒆𝒏𝑨𝑰', '𝑶𝒑𝒆𝒏𝑨𝑰', 'OpenAI、ChatGPT、Codex 与 API 官方动态', '中文：聚焦 OpenAI 官方产品和生态：新模型与能力、ChatGPT、Codex、API、开发者工具、价格配额、研究、安全、合作、服务状态和重要商业变化。优先 openai.com、官方文档、官方 GitHub、OpenAI 及核心成员的已验证账号；媒体报道必须能回到具体一手事实。不要把社区猜测、转述、普通 AI 新闻或 Codex 容量故障混入本栏。 English: Follow official OpenAI developments across models, ChatGPT, Codex, APIs, developer tools, pricing and quotas, research, safety, partnerships, service status and major business changes. Prefer openai.com, official documentation and GitHub, and verified OpenAI or staff accounts; corroborate media reports with a primary fact. Exclude speculation, reposts, generic AI news and routine Codex capacity incidents.', TRUE, 256, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 24, 36, 24, 20),
    ('anthropic', NULL, '𝑨𝒏𝒕𝒉𝒓𝒐𝒑𝒊𝒄', '𝑨𝒏𝒕𝒉𝒓𝒐𝒑𝒊𝒄', 'Anthropic、Claude 与 Claude Code 官方动态', '中文：聚焦 Anthropic 与 Claude 的一手动态：Claude 模型和能力、Claude Code、API、Agent SDK、开发者工具、价格配额、安全与对齐、服务状态、合作和商业变化。优先 anthropic.com、Claude 文档、官方仓库、官方博客与已验证账号，权威媒体只补充明确事实。排除传闻、营销转发、普通 AI 新闻和没有新事实的重复报道。 English: Cover first-party Anthropic and Claude developments: models and capabilities, Claude Code, APIs, the Agent SDK, developer tools, pricing and quotas, safety and alignment, service status, partnerships and business changes. Prefer anthropic.com, Claude documentation, official repositories, blogs and verified accounts. Exclude rumors, promotional reposts, generic AI news and duplicate stories without new facts.', TRUE, 256, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 24, 36, 24, 30),
    ('github', NULL, '𝑮𝒊𝒕𝑯𝒖𝒃', '𝑮𝒊𝒕𝑯𝒖𝒃', 'GitHub AI 项目、开发工具与开源生态', '中文：关注 GitHub 上有真实活动的 AI 项目、开源模型、开发框架、Agent 工具、推理基础设施和开发者服务。热度必须由发布版本、提交与 issue、下载采用、贡献者变化、真实用户反馈或 X 传播证据支撑；优先仓库、release、文档和作者说明。可补充重要开发者工具生态，但不要重复厂商官方主题。排除刷榜、营销文章、搬运、无代码项目和未经证实的传闻。 English: Track active GitHub AI projects, open-source models, developer frameworks, agent tools, inference infrastructure and developer services. Require evidence such as releases, commits and issues, adoption, contributor changes, real user reports or clear X circulation; prefer repositories, releases, documentation and author notes. Avoid duplicating vendor-specific topics. Exclude artificial rankings, marketing posts, reposts, projects without substance and unverified claims.', TRUE, 256, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 24, 48, 24, 40),
    ('events', NULL, '𝑬𝒗𝒆𝒏𝒕𝒔', '𝑬𝒗𝒆𝒏𝒕𝒔', '全球科技、社会与公共风险事件', '中文：跟踪最近发生且影响明确的全球科技、互联网、社会、经济、灾害、气候、公共安全、国际关系和冲突事件。每条说明事实、时间线、影响范围、来源质量和下一步风险；优先政府与机构通报、现场证据、原始文件和权威媒体。这里不重复 AI 产品专栏和具体应用案例，只保留跨区域或公共影响明显的事件。排除煽动性标题、未经证实的传闻、营销转发和重复报道。 English: Track recent global technology, internet, social, economic, disaster, climate, public-safety, international and conflict events with clear impact. Explain facts, timeline, scope, source quality and follow-up risk; prefer official statements, primary documents, direct evidence and authoritative media. Do not duplicate AI product topics or detailed use cases; keep events with meaningful public or cross-regional impact. Exclude sensational framing, unverified claims, promotional reposts and duplicates.', TRUE, 64, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 24, 36, 96, 600),
    ('cases', NULL, '𝑪𝒂𝒔𝒆𝒔', '𝑪𝒂𝒔𝒆𝒔', '真实 AI 应用、商业流程与行业案例', '中文：收集真实、可复用的 AI 应用案例，必须能说明组织或个人、具体任务、使用的模型或工具、流程变化、成本或效率、结果证据、限制和可迁移经验。覆盖医疗、教育、科研、金融支付、制造、营销、客服、内容、软件开发、个人效率和业务自动化。优先企业公告、技术报告、客户案例、论文和当事人记录；投资内容只做事实与风险分析。排除产品宣传、空泛观点、无法核实的成功故事和重复新闻。 English: Collect real, reusable AI use cases with a named organization or user, concrete task, model or tool, workflow change, cost or efficiency, evidence of results, limitations and transferable lessons. Cover healthcare, education, research, finance and payments, manufacturing, marketing, support, content, software, productivity and automation. Prefer company statements, technical reports, customer stories, papers and first-hand records. Treat investment material as factual risk analysis. Exclude product promotion, vague opinions, unverifiable success stories and duplicate news.', TRUE, 512, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'high', 24, 72, 24, 700),
    ('codex', NULL, '𝑪𝒐𝒅𝒆𝒙', '𝑪𝒐𝒅𝒆𝒙', 'Codex 使用体验、容量故障与开发者反馈', '中文：专门追踪 Codex 与相关开发者服务的真实使用信号，重点查找 Selected model is at capacity、server_overloaded、登录或额度异常、reset、版本行为变化和可复现的解决方案。来源以 X、Linux.do、GitHub issue、官方状态和一手用户记录为主；每条必须写清时间、环境、症状、证据和解决方案可信度。把 Claude 容量变化作为补充对照，不收录普通产品新闻、无证据抱怨或重复转发。 English: Track real operational signals around Codex and related developer services: capacity errors, server_overloaded, login or quota failures, reset messages, version behavior and reproducible fixes. Prefer X, Linux.do, GitHub issues, official status pages and first-hand reports. Each item must state time, environment, symptom, evidence and confidence in the proposed fix. Use Claude capacity changes only as context; exclude ordinary product news, unsupported complaints and duplicate reposts.', TRUE, 256, 0, 24, ARRAY[1,2,3,4,5,6,7]::smallint[], 'grok-4.6', 'medium', 30, 48, 24, 800);
