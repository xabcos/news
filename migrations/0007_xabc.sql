-- XABC, the site's own pen name, writes one 分享 article every morning (articles::daily).
-- Made here so the name is taken before any reader can choose it; the account never signs
-- in, because no mail reaches an .invalid address.
INSERT INTO users (email) VALUES ('xabc@xabcnews.invalid') ON CONFLICT DO NOTHING;
INSERT INTO authors (user_id, name)
SELECT id, 'XABC' FROM users WHERE email = 'xabc@xabcnews.invalid'
ON CONFLICT DO NOTHING;
-- A reader who already writes as XABC keeps the name; then the migration stops here, so the
-- collision is settled by hand instead of the articles failing every morning.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM authors a JOIN users u ON u.id = a.user_id
        WHERE u.email = 'xabc@xabcnews.invalid' AND a.name = 'XABC'
    ) THEN
        RAISE EXCEPTION 'the pen name XABC belongs to another account';
    END IF;
END $$;
