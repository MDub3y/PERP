-- Add migration script here
CREATE TYPE market_name AS ENUM ('SOL', 'ETH', 'BTC');
CREATE TYPE order_variant AS ENUM ('LONG', 'SHORT');
CREATE TYPE order_type AS ENUM ('LIMIT', 'MARKET');
CREATE TYPE order_status AS ENUM ('OPEN', 'FILLED', 'CANCELLED');

CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    username VARCHAR(255) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    collateral_available NUMERIC(20, 8) NOT NULL DEFAULT 0.0,
    collateral_locked NUMERIC(20, 8) NOT NULL DEFAULT 0.0
);

CREATE TABLE orders (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE CASCADE NOT NULL,
    market market_name NOT NULL,
    variant order_variant NOT NULL,
    quantity NUMERIC(20,8) NOT NULL,
    margin NUMERIC(20,8) NOT NULL,
    price NUMERIC(20,8) NOT NULL,
    status order_status  NOT NULL DEFAULT 'OPEN',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE positions (
    id SERIAL PRIMARY KEY,
    user_id INT REFERENCES users(id) ON DELETE CASCADE NOT NULL,
    market market_name NOT NULL,
    variant order_variant NOT NULL,
    quantity NUMERIC(20, 8) NOT NULL,
    margin NUMERIC(20, 8) NOT NULL,
    liquidation_price NUMERIC(20, 8) NOT NULL,
    avaerage_price NUMERIC(20, 8) NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_id, market, variant)
);