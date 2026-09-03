-- Personal workspaces keep the existing public seed topics (owner_user_id IS NULL)
-- while topics created from /me/ belong to one account.
ALTER TABLE users
    ADD COLUMN role TEXT NOT NULL DEFAULT 'user',
    ADD COLUMN pro_until TIMESTAMPTZ;

ALTER TABLE users
    ADD CONSTRAINT users_role_valid CHECK (role IN ('user', 'admin'));

ALTER TABLE topics
    ADD COLUMN owner_user_id BIGINT REFERENCES users(id) ON DELETE CASCADE;

CREATE INDEX topics_owner_sort_idx
    ON topics (owner_user_id, sort_weight, created_at DESC);

CREATE INDEX topics_due_idx
    ON topics (next_run_at)
    WHERE enabled AND next_run_at IS NOT NULL;

-- The catalogue is intentionally tiny.  Prices and durations are copied into an
-- order at creation time so a later catalogue edit cannot change an existing order.
CREATE TABLE plans (
    code TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    price_fen BIGINT NOT NULL CHECK (price_fen >= 0),
    duration_days INTEGER NOT NULL CHECK (duration_days >= 0 AND duration_days <= 3650),
    description TEXT NOT NULL DEFAULT '',
    active BOOLEAN NOT NULL DEFAULT TRUE,
    sort_weight INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CONSTRAINT plans_code_valid CHECK (code ~ '^[a-z][a-z0-9_-]{0,31}$'),
    CONSTRAINT plans_paid_duration_valid CHECK (price_fen = 0 OR duration_days > 0)
);

INSERT INTO plans (code, name, price_fen, duration_days, description, sort_weight)
VALUES
    ('free', 'Free', 0, 0, '最多 1 个主题，最短 120 分钟抓取间隔', 10),
    ('pro', 'Pro', 9900, 31, '最多 8 个主题，最短 60 分钟抓取间隔', 20);

CREATE TABLE orders (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    plan_code TEXT NOT NULL REFERENCES plans(code),
    out_trade_no TEXT NOT NULL UNIQUE,
    provider_trade_no TEXT,
    amount_fen BIGINT NOT NULL CHECK (amount_fen > 0),
    duration_days INTEGER NOT NULL CHECK (duration_days > 0),
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'paid', 'closed')),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (NOW() + INTERVAL '30 minutes'),
    paid_at TIMESTAMPTZ,
    payment JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- A short lease prevents two browser tabs from creating the same YPay order
    -- while keeping the local transaction independent from the network call.
    payment_started_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX orders_user_created_idx
    ON orders (user_id, created_at DESC, id DESC);
CREATE INDEX orders_user_pending_idx
    ON orders (user_id, plan_code, created_at DESC)
    WHERE status = 'pending';

CREATE TABLE payment_events (
    id BIGSERIAL PRIMARY KEY,
    event_key TEXT NOT NULL UNIQUE,
    out_trade_no TEXT NOT NULL REFERENCES orders(out_trade_no) ON DELETE CASCADE,
    status TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX payment_events_order_idx
    ON payment_events (out_trade_no, created_at DESC);
