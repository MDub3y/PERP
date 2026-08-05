-- Add migration script here

CREATE EXTENSION IF NOT EXISTS timescaledb;

-- ============================================================
-- Market configuration
-- ============================================================

CREATE TABLE markets (
    market market_name PRIMARY KEY,
    tick_size NUMERIC(20, 8) NOT NULL,
    lot_size NUMERIC(20, 8) NOT NULL,
    max_leverage SMALLINT NOT NULL,
    initial_margin_rate NUMERIC(6, 4) NOT NULL,
    maintenance_margin_rate NUMERIC(6, 4) NOT NULL,
    backstop_equity_ratio NUMERIC(6, 4) NOT NULL DEFAULT 0.25,
    liquidation_fee_rate NUMERIC(8, 6) NOT NULL DEFAULT 0.01,
    impact_notional NUMERIC(20, 8) NOT NULL DEFAULT 1000,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);
-- Seeded by crates/seeder (bin: seed_markets), not here - keeps risk
-- parameters changeable without a migration per tweak.

-- ============================================================
-- Orders / positions: the original tables from the very first migration
-- were unused scaffolding (no code ever read/wrote them - see README).
-- Safe to drop and recreate with the real shape.
-- ============================================================

DROP TABLE positions;
DROP TABLE orders;

ALTER TYPE order_status RENAME TO order_status_old;
CREATE TYPE order_status AS ENUM (
    'PENDING',          -- margin reserved in Postgres, not yet seen by the engine
    'OPEN',             -- resting on the engine's book (GTC, unfilled or partially filled)
    'PARTIALLY_FILLED',
    'FILLED',
    'CANCELLED',
    'REJECTED'          -- engine rejected on intake (e.g. account flagged for liquidation)
);
DROP TYPE order_status_old;

CREATE TYPE margin_mode AS ENUM ('CROSS', 'ISOLATED');
CREATE TYPE time_in_force AS ENUM ('GTC', 'IOC');

CREATE TABLE orders (
    id BIGSERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE RESTRICT NOT NULL,
    market market_name NOT NULL,
    variant order_variant NOT NULL,
    order_type order_type NOT NULL,
    tif time_in_force NOT NULL DEFAULT 'GTC',
    reduce_only BOOLEAN NOT NULL DEFAULT FALSE,
    leverage SMALLINT NOT NULL,
    margin_mode margin_mode NOT NULL DEFAULT 'CROSS',
    price NUMERIC(20, 8),                  -- NULL for MARKET orders
    quantity NUMERIC(20, 8) NOT NULL CHECK (quantity > 0),
    remaining_qty NUMERIC(20, 8) NOT NULL,
    reserved_margin NUMERIC(20, 8) NOT NULL,
    status order_status NOT NULL DEFAULT 'PENDING',
    is_liquidation BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK ((order_type = 'LIMIT' AND price IS NOT NULL) OR (order_type = 'MARKET' AND price IS NULL))
);
CREATE INDEX idx_orders_user ON orders (user_id, created_at DESC);
CREATE INDEX idx_orders_open ON orders (market, status) WHERE status IN ('PENDING', 'OPEN', 'PARTIALLY_FILLED');
CREATE TRIGGER trg_orders_updated_at
    BEFORE UPDATE ON orders
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Transactional outbox: identical shape/role to withdrawal_outbox, feeding
-- the engine's intake relay instead of the withdrawal worker.
CREATE TABLE orders_outbox (
    id BIGSERIAL PRIMARY KEY,
    order_id BIGINT REFERENCES orders(id) NOT NULL,
    event_type TEXT NOT NULL DEFAULT 'ORDER_PLACED', -- also ORDER_CANCEL_REQUESTED
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at TIMESTAMPTZ
);
CREATE INDEX idx_orders_outbox_unprocessed ON orders_outbox (id) WHERE processed_at IS NULL;

CREATE TABLE positions (
    id BIGSERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE RESTRICT NOT NULL,
    market market_name NOT NULL,
    variant order_variant NOT NULL,
    margin_mode margin_mode NOT NULL DEFAULT 'CROSS',
    leverage SMALLINT NOT NULL,
    quantity NUMERIC(20, 8) NOT NULL,
    average_price NUMERIC(20, 8) NOT NULL,
    allocated_margin NUMERIC(20, 8) NOT NULL DEFAULT 0, -- meaningful for ISOLATED only
    realized_pnl NUMERIC(20, 8) NOT NULL DEFAULT 0,
    funding_pnl NUMERIC(20, 8) NOT NULL DEFAULT 0,       -- tracked separately from trading PnL
    is_liquidating BOOLEAN NOT NULL DEFAULT FALSE,
    liquidation_flagged_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, market, variant)
);
CREATE INDEX idx_positions_liquidating ON positions (is_liquidating) WHERE is_liquidating = TRUE;
CREATE TRIGGER trg_positions_updated_at
    BEFORE UPDATE ON positions
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ============================================================
-- Trade history + funding - TimescaleDB hypertables
-- ============================================================

CREATE TABLE trades (
    time TIMESTAMPTZ NOT NULL,
    trade_id BIGINT NOT NULL,
    market market_name NOT NULL,
    price NUMERIC(20, 8) NOT NULL,
    quantity NUMERIC(20, 8) NOT NULL,
    maker_order_id BIGINT NOT NULL,
    taker_order_id BIGINT NOT NULL,
    maker_user_id INT NOT NULL,
    taker_user_id INT NOT NULL,
    taker_variant order_variant NOT NULL,
    maker_fee NUMERIC(20, 8) NOT NULL,
    taker_fee NUMERIC(20, 8) NOT NULL,
    is_liquidation BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (trade_id, time)
);
SELECT create_hypertable('trades', 'time', chunk_time_interval => INTERVAL '1 day');
CREATE INDEX idx_trades_market_time ON trades (market, time DESC);

CREATE TABLE funding_rate_samples (
    time TIMESTAMPTZ NOT NULL,
    market market_name NOT NULL,
    index_price NUMERIC(20, 8) NOT NULL,
    bid_impact NUMERIC(20, 8) NOT NULL,
    ask_impact NUMERIC(20, 8) NOT NULL,
    premium_index NUMERIC(20, 10) NOT NULL,
    PRIMARY KEY (market, time)
);
SELECT create_hypertable('funding_rate_samples', 'time', chunk_time_interval => INTERVAL '1 day');

CREATE TABLE funding_payments (
    id BIGSERIAL PRIMARY KEY,
    settlement_time TIMESTAMPTZ NOT NULL,
    market market_name NOT NULL,
    user_id INT REFERENCES users(id) NOT NULL,
    position_qty NUMERIC(20, 8) NOT NULL,
    funding_rate_hour NUMERIC(12, 8) NOT NULL,
    amount NUMERIC(20, 8) NOT NULL, -- signed: credit positive, debit negative
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (market, user_id, settlement_time)
);

-- ============================================================
-- Fees
-- ============================================================

CREATE TABLE fee_tiers (
    tier SMALLINT PRIMARY KEY,
    volume_threshold_30d NUMERIC(20, 2) NOT NULL,
    taker_rate NUMERIC(8, 6) NOT NULL,
    maker_rate NUMERIC(8, 6) NOT NULL -- negative = rebate
);
-- Rates are fractions (0.000400 = 0.0400%), not raw percentages - Fee =
-- Notional * rate, applied directly with no /100 step anywhere downstream.
INSERT INTO fee_tiers (tier, volume_threshold_30d, taker_rate, maker_rate) VALUES
    (0, 0,           0.000400,  0.000125),
    (1, 1000000,     0.000370,  0.000100),
    (2, 5000000,     0.000350,  0.000080),
    (3, 25000000,    0.000300,  0.000050),
    (4, 100000000,   0.000270,  0.000020),
    (5, 500000000,   0.000250,  0.000000),
    (6, 1000000000,  0.000200, -0.000050);

ALTER TABLE users ADD COLUMN fee_tier SMALLINT NOT NULL DEFAULT 0 REFERENCES fee_tiers(tier);
ALTER TABLE users ADD COLUMN fee_tier_updated_at TIMESTAMPTZ;

-- ============================================================
-- Insurance fund: a plain user row, not a bespoke account type, so it can
-- hold positions and a collateral balance through the exact same machinery
-- as any other account. Also doubles as the fee-recipient account.
-- ============================================================

INSERT INTO users (username, password_hash, collateral_available, collateral_locked)
VALUES ('__insurance_fund__', '!', 0, 0);

-- ============================================================
-- Engine event-log convergence + crash-recovery snapshots
-- ============================================================

-- Single-row cursor: last engine:events id the ledger writer has applied.
-- Guards every apply_* transaction so redelivery is a no-op.
CREATE TABLE engine_event_cursor (
    id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (id = TRUE), -- enforce single row
    last_applied_event_id BIGINT NOT NULL DEFAULT 0
);
INSERT INTO engine_event_cursor (last_applied_event_id) VALUES (0);

CREATE TABLE engine_snapshots (
    id BIGSERIAL PRIMARY KEY,
    taken_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_event_id BIGINT NOT NULL,
    payload BYTEA NOT NULL
);
CREATE INDEX idx_engine_snapshots_latest ON engine_snapshots (taken_at DESC);
