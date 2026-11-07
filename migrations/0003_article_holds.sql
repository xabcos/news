-- Why an article waits for an administrator: the AI's reason for not passing it, that its
-- author's AI reviews for the day are used up, or that an administrator took it back. While it
-- is set, the AI doesn't review the article again, whatever its author changes; a ❤️ clears it.
-- `approvals` counts those ❤️: a refusal from a review that began before the latest holds nothing.
ALTER TABLE articles ADD COLUMN held text, ADD COLUMN approvals integer NOT NULL DEFAULT 0;
-- The AI reviews of the author's articles started on `day`.
ALTER TABLE authors ADD COLUMN reviews smallint NOT NULL DEFAULT 0;
-- What waited for an administrator before the AI reviewed submissions, or was public once and
-- isn't now, may have been taken back, so it stays with the administrators.
UPDATE articles SET held = '待管理员审核'
WHERE status = 'pending' OR (status = 'draft' AND published_at IS NOT NULL);
