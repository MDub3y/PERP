-- Add migration script here

-- List of all the deposit addresses
CREATE TABLE deposit_addresses (
    pubkey VARCHAR(44) PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE SET NULL,
    assigned_at TIMESTAMPTZ,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);

-- List of unassigned addresses
CREATE INDEX idx_unassigned_addresses ON deposit_addresses(pubkey) WHERE user_id IS NULL;

-- List of all the deposits made
CREATE TABLE deposits (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE CASCADE NOT NULL,
    pubkey VARCHAR(44) REFERENCES deposit_addresses(pubkey) NOT NULL,
    signature VARCHAR(88) UNIQUE NOT NULL,
    amount NUMERIC(20, 8) NOT NULL,
    indexed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);