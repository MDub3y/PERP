# PERP Engine

A centralized perpetuals exchange on Solana. Users hold a custodial balance managed by this backend; a matching engine (not yet built) would trade against that balance. This document covers only what currently exists in the codebase.

## Workspace layout

- `crates/api` — public HTTP API (axum). Signup/signin, JWT auth, withdrawal requests.
- `crates/store` — shared Postgres data-access layer (sqlx), used by both `api` and `worker`.
- `crates/worker` — background process that owns the fat-wallet signing key, drains the withdrawal queue, and talks to Solana. Never exposed to the network.
- `crates/seeder` — one-off bootstrap scripts: deposit-address pool generation, fat-wallet + durable-nonce pool creation.

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

## Running locally

```
docker compose up -d              # postgres, redis, local solana validator
cargo run -p seeder --bin seeder          # populate the deposit-address pool
cargo run -p seeder --bin fund_fatwallet  # fund the fat wallet + create the nonce pool
cargo run -p api                  # public API
cargo run -p worker               # withdrawal processor
```

Env vars: `DATABASE_URL`, `JWT_SECRET`, `SERVER_ADDRESS` (api); `SOLANA_RPC_URL`, `REDIS_URL`, `WORKER_CONCURRENCY` (worker); `WITHDRAWAL_RATE_LIMIT_PER_DAY` (api, default 5).

The local validator image is pinned to `solanalabs/solana:v1.18.26` — newer agave/solana images require `io_uring`, which WSL2's kernel does not support, and `solana-test-validator` panics on startup there.

## API

| Route | Auth | Description |
|---|---|---|
| `GET /health` | none | DB connectivity check |
| `POST /signup` | none | Create a user, assign a deposit address |
| `POST /signin` | none | Returns a JWT |
| `POST /withdrawals` | bearer JWT | Queue a withdrawal (`amount`, `destination_pubkey`) |

## Not yet built

- Deposit indexing (crediting `collateral_available` from on-chain transfers)
- Sweeping deposit-address balances into the fat wallet
- Order placement, order book, matching engine, positions/margin logic (the `orders`/`positions` tables exist in the schema but no code touches them)
