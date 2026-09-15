-- Align persisted plan descriptions with the current personal-topic limits.
UPDATE plans
SET description = '最多 1 个主题，最短 512 分钟抓取间隔'
WHERE code = 'free';

UPDATE plans
SET description = '最多 4 个主题，最短 64 分钟抓取间隔'
WHERE code = 'pro';
