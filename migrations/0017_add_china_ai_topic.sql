DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM topics
        WHERE id = 'china-ai' AND owner_user_id IS NOT NULL
    ) THEN
        RAISE EXCEPTION 'topic id china-ai is already used by a personal topic';
    END IF;
END $$;

INSERT INTO topics (
    id, owner_user_id, name, name_en, description, brief, enabled, interval_minutes,
    window_start, window_end, weekdays, model, reasoning_effort, max_items,
    lookback_hours, daily_run_cap, sort_weight
)
VALUES (
    'china-ai', NULL, '𝑪', '𝑪', '国产大模型与 AI 生态动态',
    $$中文：集中关注中国大陆 AI 模型公司、开源社区、开发平台和应用生态的最新动态。覆盖 DeepSeek、Kimi／月之暗面、智谱／Z.ai／GLM、通义千问／Qwen、豆包／字节 Seed、MiniMax、百度文心／ERNIE、腾讯混元、华为盘古、讯飞星火、阶跃星辰 Step、01.AI／Yi、百川、书生·浦语／InternLM、OpenBMB／MiniCPM、商汤日日新、小米 MiMo 等主流模型与生态，也纳入后续出现并获得实际关注的新团队、新模型和重要开源项目。重点采集新模型和新版本、推理与代码能力、多模态与生成模型、Agent 与 Coding 工具、API 与价格、开源权重、技术报告、部署与推理框架、开发者平台、企业落地、重要合作和真实使用反馈。优先模型厂商官网、官方博客与文档、GitHub／Hugging Face 仓库、技术报告、发布记录和可信媒体；使用 X.com 追踪官方账号、研究者和开发者的及时传播，并尽量用一手来源核实。按最近 24 小时的综合热度选择项目和事件，热度须有发布、版本变化、开发者采用、社区讨论或其他可验证证据；同一事件只保留一条，排除刷榜、营销稿、无来源 benchmark、未经证实的传闻、重复报道和泛泛的 AI 行业新闻。
English: Track the latest developments across China-based AI model companies, open-source communities, developer platforms and applications. Cover major model families and ecosystems including DeepSeek, Kimi/Moonshot, Zhipu/Z.ai/GLM, Qwen, Doubao/ByteDance Seed, MiniMax, Baidu ERNIE, Tencent Hunyuan, Huawei Pangu, iFlytek Spark, StepFun Step, 01.AI Yi, Baichuan, InternLM, OpenBMB/MiniCPM, SenseTime SenseNova and Xiaomi MiMo, while adding new teams, models and important open-source projects that gain meaningful attention. Prioritize new models and releases, reasoning and coding capabilities, multimodal and generative models, agents and coding tools, APIs and pricing, open weights, technical reports, deployment and inference frameworks, developer platforms, enterprise adoption, major partnerships and real usage evidence. Prefer model providers' official sites, blogs and documentation, GitHub/Hugging Face repositories, technical reports, release notes and reliable reporting. Use X.com to follow timely signals from official accounts, researchers and developers, and corroborate them with primary sources whenever possible. Select items by verifiable overall heat from the last 24 hours, including releases, meaningful version changes, developer adoption, community discussion or other concrete evidence; keep one item per event and exclude artificial rankings, marketing copy, unsupported benchmarks, rumors, duplicates and generic AI industry news.$$,
    TRUE, 64, 5, 20, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 24, 24, 24, 18
)
ON CONFLICT (id) DO UPDATE SET
    owner_user_id = EXCLUDED.owner_user_id,
    name = EXCLUDED.name,
    name_en = EXCLUDED.name_en,
    description = EXCLUDED.description,
    brief = EXCLUDED.brief,
    enabled = EXCLUDED.enabled,
    interval_minutes = EXCLUDED.interval_minutes,
    window_start = EXCLUDED.window_start,
    window_end = EXCLUDED.window_end,
    weekdays = EXCLUDED.weekdays,
    model = EXCLUDED.model,
    reasoning_effort = EXCLUDED.reasoning_effort,
    max_items = EXCLUDED.max_items,
    lookback_hours = EXCLUDED.lookback_hours,
    daily_run_cap = EXCLUDED.daily_run_cap,
    sort_weight = EXCLUDED.sort_weight,
    updated_at = NOW()
WHERE topics.owner_user_id IS NULL;
