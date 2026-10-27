-- 股市观察 and its source, the 股市资讯 topic (`equity`), are gone. The topic's runs and items go
-- with it, and `ai_insights` keeps XAI 日报 alone, so the column that told the two apart and its
-- indexes go too.
DELETE FROM ai_insights WHERE kind = 'equity';
ALTER TABLE ai_insights DROP COLUMN kind;
DELETE FROM topics WHERE id = 'equity';
