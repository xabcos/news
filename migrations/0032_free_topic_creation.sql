-- Free users can review the personal-topic configuration and usage, but creating
-- a new personal fetch task requires an active Pro subscription.
UPDATE plans
SET description = '可查看已有主题，新建个人抓取任务需 Pro'
WHERE code = 'free';
