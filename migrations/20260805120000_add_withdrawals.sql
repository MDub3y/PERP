-- Add migration script here

-- Withdrawal state machine
CREATE TYPE withdrawal_status AS ENUM (
    'QUEUED',      -- funds debited from collateral_available, outbox row inserted in same tx
    'SUBMITTING',  -- signature computed + signed tx bytes persisted, broadcast attempted/in-flight
    'SUBMITTED',   -- RPC accepted the broadcast, awaiting finalization
    'CONFIRMED',   -- finalized on-chain, terminal success
    'FAILED',      -- reserved for a future manual-review hold state
    'REFUNDED'     -- funds credited back to collateral_available, terminal failure
);

-- Pool of durable-nonce accounts owned by the fat wallet, assigned the same
-- way deposit_addresses hands out unassigned pubkeys.
CREATE TABLE fee_payer_nonces (
    nonce_pubkey VARCHAR(44) PRIMARY KEY,
    is_locked BOOLEAN NOT NULL DEFAULT FALSE,
    locked_by_request_id INT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_free_nonces ON fee_payer_nonces (nonce_pubkey) WHERE is_locked = FALSE;

CREATE TABLE withdrawal_requests (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE RESTRICT NOT NULL,
    amount NUMERIC(20, 8) NOT NULL CHECK (amount > 0),
    destination_pubkey VARCHAR(44) NOT NULL,
    status withdrawal_status NOT NULL DEFAULT 'QUEUED',
    signature VARCHAR(88) UNIQUE,
    signed_tx_bytes BYTEA,
    nonce_account VARCHAR(44) REFERENCES fee_payer_nonces(nonce_pubkey),
    nonce_hash VARCHAR(44),
    error_message TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    submitted_at TIMESTAMPTZ,
    confirmed_at TIMESTAMPTZ
);

-- Rate-limit window query (N withdrawals per user per rolling 24h)
CREATE INDEX idx_withdrawal_requests_user_created ON withdrawal_requests (user_id, created_at DESC);

-- Reconciliation sweep query
CREATE INDEX idx_withdrawal_requests_inflight ON withdrawal_requests (status, updated_at)
    WHERE status IN ('QUEUED', 'SUBMITTING', 'SUBMITTED');

-- Transactional outbox: pure relay hand-off from Postgres to the Redis
-- Streams dispatch layer, never a source of truth on its own.
CREATE TABLE withdrawal_outbox (
    id BIGSERIAL PRIMARY KEY,
    withdrawal_request_id INT REFERENCES withdrawal_requests(id) NOT NULL,
    event_type TEXT NOT NULL DEFAULT 'WITHDRAWAL_QUEUED',
    payload JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at TIMESTAMPTZ
);

CREATE INDEX idx_withdrawal_outbox_unprocessed ON withdrawal_outbox (id) WHERE processed_at IS NULL;

CREATE OR REPLACE FUNCTION set_updated_at() RETURNS TRIGGER AS $$
BEGIN NEW.updated_at = NOW(); RETURN NEW; END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_withdrawal_requests_updated_at
    BEFORE UPDATE ON withdrawal_requests
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
