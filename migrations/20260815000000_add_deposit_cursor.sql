-- Add migration script here

-- Cursor column so the deposit indexer only scans new signatures for an
-- address on each poll instead of re-walking full history every tick.
ALTER TABLE deposit_addresses ADD COLUMN last_signature VARCHAR(88) NULL;
