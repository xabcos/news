-- Raise the free plan's minimum topic refresh interval to four hours.
UPDATE plans
SET description = '最多 1 个主题，最短 240 分钟抓取间隔'
WHERE code = 'free';
