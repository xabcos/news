-- A checkout never remains locally pending for more than an hour. A provider
-- may return an earlier QR expiry, which remains authoritative for that QR.
ALTER TABLE orders ALTER COLUMN expires_at SET DEFAULT (NOW() + INTERVAL '1 hour');

CREATE INDEX orders_cleanup_idx ON orders (created_at, id)
    WHERE status IN ('pending', 'closed') AND paid_at IS NULL;

-- Expired checkout details can be removed while a minimal event still routes
-- a delayed, signed successful payment to the right account exactly once.
ALTER TABLE payment_events ADD COLUMN user_id bigint REFERENCES users(id) ON DELETE CASCADE;
UPDATE payment_events e SET user_id = o.user_id FROM orders o WHERE o.out_trade_no = e.out_trade_no;
ALTER TABLE payment_events ALTER COLUMN user_id SET NOT NULL;
ALTER TABLE payment_events DROP CONSTRAINT payment_events_out_trade_no_fkey;

-- A restart must not repeat the same day's cleanup.
CREATE TABLE maintenance_state (
    task text PRIMARY KEY,
    last_completed_day date NOT NULL
);
