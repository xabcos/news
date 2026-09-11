ALTER TABLE items
    ADD COLUMN image JSONB;

-- 𝐏 is a visual public feed. It sits immediately after 𝐗 in the public catalogue.
INSERT INTO topics (
    id, owner_user_id, name, name_en, description, brief, enabled, interval_minutes,
    window_start, window_end, weekdays, model, reasoning_effort, max_items,
    lookback_hours, daily_run_cap, sort_weight
)
VALUES (
    'p', NULL, '𝐏', '𝐏', '全球热点图片新闻',
    '中文：收集最近 24 小时全球最值得关注的热点图片新闻，覆盖科技、商业、社会、文化、公共事件与自然变化。每条必须有可以核实的真实照片，优先使用官方、现场或权威媒体发布的图片；说明照片所展示的事实、时间和影响。排除广告、商标占位图、拼图、AI 生成图、血腥画面、未经证实的传闻和重复报道。没有可靠图片的新闻不要返回。
English: Collect the most important visual news from the past 24 hours across technology, business, society, culture, public events and natural change. Every item must include a verifiable real photograph from an official, eyewitness or authoritative media source, with a factual caption describing what it shows, when and why it matters. Exclude ads, logos, collages, AI-generated images, graphic violence, rumors and duplicates. Omit stories without a reliable photograph.',
    TRUE, 120, 0, 24, ARRAY[1,2,3,4,5,6,7]::SMALLINT[], 'grok-4.6', 'high', 12, 24, 12, 15
)
ON CONFLICT (id) DO UPDATE SET
    name = EXCLUDED.name,
    name_en = EXCLUDED.name_en,
    description = EXCLUDED.description,
    brief = EXCLUDED.brief,
    sort_weight = EXCLUDED.sort_weight;
