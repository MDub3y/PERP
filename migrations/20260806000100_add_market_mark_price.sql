-- Add migration script here

-- Cached last-trade mark price per market, kept in Postgres (rather than
-- only Redis) so store::orders::place_order can synchronously estimate
-- required margin for MARKET orders without api/store taking on a Redis
-- dependency. The engine writes this directly (last-writer-wins, not routed
-- through the ledger event-cursor convergence machinery) on every trade -
-- it's an estimate/display cache, not authoritative money movement; the
-- real fill price always reconciles reserved_margin afterwards.
ALTER TABLE markets ADD COLUMN mark_price NUMERIC(20, 8);
ALTER TABLE markets ADD COLUMN mark_price_updated_at TIMESTAMPTZ;
