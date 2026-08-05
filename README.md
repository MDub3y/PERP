# PERP Engine

A centralized perpetuals exchange on Solana. Users hold a custodial balance managed by this backend and trade it against an in-memory matching engine. This document covers only what currently exists in the codebase.

## Workspace layout

- `crates/api` — public HTTP API (axum). Signup/signin, JWT auth, withdrawal requests, order placement/cancel, positions.
- `crates/store` — shared Postgres data-access layer (sqlx), used by `api`, `worker`, and `engine`.
- `crates/worker` — background process that owns the fat-wallet signing key, drains the withdrawal queue, and talks to Solana. Never exposed to the network.
- `crates/engine` — the matching engine: in-memory order book, margin/leverage, funding, tiered fees, and liquidation. Owns no signing key and is never exposed to the network either.
- `crates/seeder` — one-off bootstrap scripts: deposit-address pool generation, fat-wallet + durable-nonce pool creation, market config seeding.

## Custodial account model

Each user is a row in `users` with two balance columns: `collateral_available` (free balance) and `collateral_locked` (reserved for open-position margin — no code currently moves funds into this column, since no matching engine exists yet). Passwords are hashed with argon2 (`crates/store/src/users.rs`). Signin issues a JWT whose `sub` claim is the numeric user id; `crates/api/src/auth.rs` validates that token on protected routes and injects the user id as an `AuthUser` extractor.

## Dedicated deposit addresses

Every user gets their own deposit address, assigned atomically at signup:

- `crates/seeder` derives 1,000 Solana keypairs from a single BIP39 mnemonic (`keys/mnemonic.txt`) using per-index derivation paths, and inserts the public keys into `deposit_addresses` as an unassigned pool.
- `store::users::create_user_with_deposit_address` claims one of those addresses in the same transaction that creates the user row (`UPDATE ... WHERE pubkey = (SELECT ... FOR UPDATE SKIP LOCKED)`), so address assignment can't race or double-assign under concurrent signups.

**Not implemented:** nothing currently watches the chain for incoming transfers to these addresses or credits `collateral_available` from them. The `deposits` table exists in the schema to record indexed deposits, but no indexer writes to it yet — deposit crediting is manual/future work.

## Withdrawals

The withdrawal path is the most guarded part of the system, since it moves funds out of a centrally-held fat wallet. It's built as a two-phase-commit pipeline: Postgres is the durable source of truth, Redis Streams is a fast ack-based dispatch layer that is never treated as authoritative.

**Request path** (`POST /withdrawals`, JWT-protected):
`store::withdrawals::request_withdrawal` runs one Postgres transaction that: row-locks the user (`SELECT ... FOR UPDATE`), enforces a rolling 24-hour rate limit (`WITHDRAWAL_RATE_LIMIT_PER_DAY`, default 5), checks and debits `collateral_available`, and inserts both the `withdrawal_requests` row and a `withdrawal_outbox` row. All four steps commit together, which is what prevents a check-then-lock race across concurrent requests from the same user.

**Dispatch:** `crates/worker`'s relay tails `withdrawal_outbox` and republishes each row onto a Redis stream (`XADD`). This is a pure transactional-outbox relay — Redis holds no state that Postgres doesn't already have.

**Processing** (`crates/worker/src/processor.rs`), per request:
1. Claim a durable-nonce account from the `fee_payer_nonces` pool (same `FOR UPDATE SKIP LOCKED` pattern as deposit-address assignment).
2. Build and sign the transfer using that nonce. The signature is computed locally and is fully deterministic given the nonce value — no network round-trip needed to know it.
3. **Persist the signature and signed transaction bytes before broadcasting** (`mark_submitting`). This is the core correctness property: if the process crashes between broadcasting and acknowledging the queue entry, a durable, reproducible record of exactly what was sent already exists.
4. Broadcast. Failure (including the chain being unreachable) just leaves the request in `SUBMITTING` for retry — nothing is lost.

**Recovery**, on redelivery of a `SUBMITTING`/`SUBMITTED` request (from Redis `XAUTOCLAIM`, a fresh consumer, or the reconciler):
- Query the chain for the persisted signature. Found and finalized → mark `CONFIRMED`. Found and pending → stays `SUBMITTED`. Chain rejected it → refund atomically (`fail_and_refund`, one transaction: mark `REFUNDED` + credit `collateral_available` back + release the nonce).
- Not found: compare the on-chain nonce value against the one persisted with the request. Unchanged means the transaction never landed — safe to rebroadcast the *identical* signed bytes (same signature, no double-spend). Changed means the transaction almost certainly executed; the request is deliberately **not** auto-refunded, and a critical log line is emitted for manual review, since guessing wrong here risks double-paying the user.

**Redelivery mechanics:** Redis consumer groups (`XREADGROUP`/`XACK`) handle normal processing; `XAUTOCLAIM` reclaims entries abandoned by a crashed consumer after an idle timeout. Independently of Redis, a reconciler sweeps `withdrawal_requests` every 30s for anything stuck in a non-terminal status for more than 2 minutes and re-drives it through the same processing step — so a lost or trimmed Redis entry, a relay outage, or a deleted consumer group still can't strand a request.

**Isolation:** the fat-wallet private key (derived from the same mnemonic as deposit addresses, at a dedicated index) only ever exists in the `worker` process's memory. `api` never has access to it — the pubkey-format validation `api` does on `destination_pubkey` is parsing only.

## Withdrawal state machine

```
QUEUED -> SUBMITTING -> SUBMITTED -> CONFIRMED
   \           \____________\
    \-----------------------> REFUNDED
```

## Matching Engine

The engine (`crates/engine`) is a separate process from `api`, matching orders in memory for speed while Postgres stays the durable source of truth for balances. It reuses the withdrawal pipeline's pattern throughout: transactional outbox → Redis Streams relay → consumer-group processing → idempotent guarded state transitions.

**Consistency model:** margin is reserved in Postgres *before* the engine ever sees an order (`POST /orders` → `store::orders::place_order`, one transaction: row-lock the user, check + debit `collateral_available` → `collateral_locked`, insert the `orders` row + an `orders_outbox` row). The engine's relay drains that outbox onto a Redis stream; the engine matches in memory; every balance-affecting outcome (fill, fee, funding, liquidation transfer) is appended to a second stream, `engine:events`, which an independent ledger-writer consumer applies back to Postgres as small guarded transactions. Postgres balances therefore converge with bounded lag (normally sub-second) rather than being touched on the matching hot path at all.

**Orders:** LIMIT and MARKET, GTC or IOC, LONG or SHORT, reduce-only, cross or isolated margin. LIMIT+GTC rests on the book if unfilled; LIMIT+IOC and MARKET (always treated as IOC) fill what they can and drop the remainder. The book is a per-market `BTreeMap<price, VecDeque<order>>` (price-time priority), matched against `rust_decimal::Decimal` directly rather than a custom fixed-point type — simpler, and this codebase already leans on `Decimal` everywhere else.

**Fees:** tiered maker/taker rates by trailing 30-day volume (`fee_tiers`, refreshed once per UTC day), applied inline per fill so `engine:events` fill records already carry final amounts. The top tier's maker rate is negative — a plain sign-flipped transfer handles the rebate, no special-cased branch.

**Funding:** a 5-second sampler walks the book for the bid/ask VWAP that would fill `impact_notional`, computes the premium index against the mark price, and writes to the `funding_rate_samples` hypertable. Once per UTC hour, `mean_P` over the trailing window is turned into `FR_hour` per the standard 8-hour-formula-divided-by-8 approach and settled against every open position, credited/debited directly to `collateral_available` and tracked in `positions.funding_pnl` separately from trading PnL.

**Liquidation:** margin health (`Equity` vs `MaintenanceMargin`) is polled every 3 seconds rather than recomputed synchronously on every fill — a deliberate simplification of "continuous". Cross evaluates an account's combined cross exposure; isolated evaluates one position independently. A flagged scope is blocked from new order intake (checked in-memory, no Postgres round-trip) and unwound with engine-generated reduce-only IOC orders. If the book can't absorb an unwind and equity has fallen to a small fraction of maintenance margin, the remaining position is transferred directly to the insurance-fund account instead (`backstop_equity_ratio`, per-market). Liquidation fills carry an extra `liquidation_fee_rate` surcharge on top of the normal taker rate.

**Mark/index price:** the engine's own last-trade price, mirrored to `markets.mark_price` so `place_order` can size MARKET-order margin without depending on the engine being reachable. There is no external oracle in this codebase — this is a documented stub; Pyth would be the natural integration given the stack is already Solana-native.

**Crash recovery:** the match-loop task snapshots its full state (books, positions, fee/mark-price caches, event/trade id counters) to Postgres every 10 seconds. On restart it loads the latest snapshot and resumes — no replay from the event log — so at most ~10 seconds of activity is lost on a crash, a bound accepted up front rather than engineered around.

**Pub/sub:** Redis Streams (`engine:events`) is durable history; Redis Pub/Sub is a separate, ephemeral live feed of the same facts — public per-market channels (`market:{SYMBOL}:trades|book|ticker`) and a private per-user channel (`user:{id}:orders`, `ACCEPTED`/`FILL`/`RESTED`/`CANCELED`/`REJECTED`) so a client can watch its own order resolve in real time instead of reloading. No consumer of these channels exists yet — a WebSocket gateway bridging them to browsers is the next piece, not built here.

## Running locally

```
docker compose up -d                      # postgres (timescaledb), redis, local solana validator
cargo run -p seeder --bin seeder          # populate the deposit-address pool
cargo run -p seeder --bin fund_fatwallet  # fund the fat wallet + create the nonce pool
cargo run -p seeder --bin seed_markets    # seed SOL/ETH/BTC market config
cargo run -p api                          # public API
cargo run -p worker                       # withdrawal processor
cargo run -p engine                       # matching engine
```

Env vars: `DATABASE_URL`, `JWT_SECRET`, `SERVER_ADDRESS` (api); `SOLANA_RPC_URL`, `REDIS_URL`, `WORKER_CONCURRENCY` (worker); `WITHDRAWAL_RATE_LIMIT_PER_DAY` (api, default 5); `DATABASE_URL`, `REDIS_URL`, `ENGINE_INTAKE_CONCURRENCY` (engine, default 2).

The local validator image is pinned to `solanalabs/solana:v1.18.26` — newer agave/solana images require `io_uring`, which WSL2's kernel does not support, and `solana-test-validator` panics on startup there.

## API

| Route | Auth | Description |
|---|---|---|
| `GET /health` | none | DB connectivity check |
| `POST /signup` | none | Create a user, assign a deposit address |
| `POST /signin` | none | Returns a JWT |
| `POST /withdrawals` | bearer JWT | Queue a withdrawal (`amount`, `destination_pubkey`) |
| `POST /orders` | bearer JWT | Place an order (`market`, `variant`, `order_type`, `tif`, `reduce_only`, `leverage`, `margin_mode`, `price`, `quantity`) |
| `DELETE /orders/{id}` | bearer JWT | Cancel an open order, refunding unfilled margin |
| `GET /orders` | bearer JWT | List the caller's open orders |
| `GET /positions` | bearer JWT | List the caller's open positions |

## Not yet built

- Deposit indexing (crediting `collateral_available` from on-chain transfers)
- Sweeping deposit-address balances into the fat wallet
- External price oracle (mark/index price is currently the engine's own last-trade price — see Matching Engine)
- WebSocket gateway bridging the engine's Redis Pub/Sub channels to browser clients
- Cross liquidation currently unwinds the first open position it finds rather than largest-notional-first
