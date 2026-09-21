-- Keep fresh installations and existing topics on the current collection model.
-- Historical migrations stay unchanged so SQLx can validate their checksums.
ALTER TABLE topics ALTER COLUMN model SET DEFAULT 'grok-4.7';

-- Only replace the former default; preserve other model choices and run history.
UPDATE topics
SET model = 'grok-4.7', updated_at = now()
WHERE model = 'grok-4.6';
