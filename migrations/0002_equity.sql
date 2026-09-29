ALTER TABLE topics ADD COLUMN visible boolean NOT NULL DEFAULT true;

-- Existing collection settings are preserved; equity becomes an internal source.
INSERT INTO topics (id, name, name_en, description, brief, visible, sort_weight)
VALUES (
    'equity', '𝑬𝒒𝒖𝒊𝒕𝒚', '𝑬𝒒𝒖𝒊𝒕𝒚', '中国与美国上市公司的公告、经营变化与市场事件',
    '采集中国 A 股、港股和美国上市公司的可核验研究线索。优先交易所公告、公司投资者关系、监管文件及可靠媒体，明确公司名称、证券代码、交易所、发布时间与原文链接。关注业绩及指引、订单、并购、回购执行、监管与行业政策。区分计划与已发生事实，保留金额、单位和适用期间，不把事件利好直接等同于股价上涨。同一事件合并，不收录无来源荐股、目标价和收益承诺。',
    false, 95
)
ON CONFLICT (id) DO NOTHING;
UPDATE topics SET visible = false WHERE id = 'equity' AND owner_user_id IS NULL;

ALTER TABLE ai_insights ADD COLUMN kind text NOT NULL DEFAULT 'ai'
    CHECK (kind IN ('ai', 'equity'));
CREATE INDEX ai_insights_kind_day_idx
    ON ai_insights (kind, briefing_day DESC, finished_at DESC) WHERE status = 'ok';
