-- Pyth Hermes price-feed id per market, used by crates/engine/src/oracle.rs
-- to know which feed to poll for each market's external index price. NULL
-- means "no oracle configured for this market yet" - the oracle poller
-- skips such markets rather than erroring, so seeding a market without a
-- feed id (e.g. in local dev) doesn't break anything else.
ALTER TABLE markets ADD COLUMN pyth_price_feed_id TEXT;
