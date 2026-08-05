-- Add migration script here

-- Correction: a position must net LONG/SHORT exposure into a single row per
-- (user, market) for margin/PnL/liquidation math to make sense (the
-- reference liquidation doc evaluates "each isolated position" and cross
-- exposure per market, not per variant). The original UNIQUE(user_id,
-- market, variant) would have let a user hold independent LONG and SHORT
-- rows for the same market simultaneously (hedge mode), which nothing in
-- this build implements or relies on. positions is still empty at this
-- point in development, so this is a free correction, not a data migration.
--
-- quantity becomes signed (positive = net long, negative = net short);
-- variant is kept as a redundant, always-recomputed display column (sign of
-- quantity) so API responses don't need extra client-side logic.
ALTER TABLE positions DROP CONSTRAINT positions_user_id_market_variant_key;
ALTER TABLE positions ADD CONSTRAINT positions_user_id_market_key UNIQUE (user_id, market);
