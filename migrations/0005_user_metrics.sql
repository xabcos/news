-- Keep the total user count constant-time to read even when users becomes very large.
-- The trigger below changes it in the same transaction as the users row change.
CREATE TABLE user_metrics (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id = TRUE),
    registered_count BIGINT NOT NULL CHECK (registered_count >= 0)
);

-- Use the current row count as the one-time baseline. Login history itself is
-- intentionally not reconstructed.
INSERT INTO user_metrics (id, registered_count)
SELECT TRUE, COUNT(*)
FROM users;

ALTER TABLE users
    ADD COLUMN last_login_at TIMESTAMPTZ;

-- Existing rows remain NULL until their next successful login; no login history
-- is reconstructed by the migration.
CREATE INDEX users_active_last_login_idx
    ON users (last_login_at DESC)
    WHERE status AND last_login_at IS NOT NULL;

CREATE FUNCTION maintain_user_metrics() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        UPDATE user_metrics
        SET registered_count = registered_count + 1
        WHERE id = TRUE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'user_metrics singleton row is missing';
        END IF;
    ELSIF TG_OP = 'DELETE' THEN
        UPDATE user_metrics
        SET registered_count = registered_count - 1
        WHERE id = TRUE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'user_metrics singleton row is missing';
        END IF;
    ELSIF TG_OP = 'TRUNCATE' THEN
        UPDATE user_metrics
        SET registered_count = 0
        WHERE id = TRUE;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'user_metrics singleton row is missing';
        END IF;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER users_metrics_count
AFTER INSERT OR DELETE ON users
FOR EACH ROW
EXECUTE FUNCTION maintain_user_metrics();

CREATE TRIGGER users_metrics_truncate
AFTER TRUNCATE ON users
EXECUTE FUNCTION maintain_user_metrics();
