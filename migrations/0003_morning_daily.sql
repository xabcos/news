-- Fixed editorial windows make retries and manual regeneration reproducible.
ALTER TABLE ai_insights ADD COLUMN window_end timestamptz;
ALTER TABLE runs ADD COLUMN window_end timestamptz;
ALTER TABLE runs DROP CONSTRAINT runs_trigger_valid;
ALTER TABLE runs ADD CONSTRAINT runs_trigger_valid
    CHECK (triggered_by IN ('schedule', 'manual', 'briefing'));
CREATE INDEX runs_briefing_window_idx ON runs (window_end, topic_id)
    WHERE triggered_by = 'briefing' AND status = 'ok';
CREATE INDEX items_event_time_idx ON items (COALESCE(published_at, first_seen_at))
    WHERE NOT hidden;

-- Existing installations may already have an editor-configured Gossip column.
INSERT INTO topics
    (id, name, name_en, description, brief, weekdays, window_start, window_end,
     interval_minutes, reasoning_effort, max_items, lookback_hours, daily_run_cap, sort_weight)
VALUES
    ('gossip', '𝑮𝒐𝒔𝒔𝒊𝒑', '𝑮𝒐𝒔𝒔𝒊𝒑', 'AI 圈爆料、回应与公开热议',
     '收集 AI 行业最近 24 小时的公开爆料、产品传闻、人物回应、开发者趣闻和用户反馈。每条提供可追溯的原始帖子或报道，清楚区分已确认事实、消息源说法与推测；优先原作者和当事人，查找官方回应。同一事件合并，更新确认或否认状态；没有真实传播证据时不要宣称热议。排除无来源编造、营销搬运和私人生活猜测。',
     ARRAY[1,2,3,4,5,6,7]::smallint[], 0, 24, 128, 'high', 12, 24, 24, 20)
ON CONFLICT (id) DO NOTHING;
