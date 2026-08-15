//! Shared harness for integration tests: a real Postgres connection pool
//! (migrated) plus fixture helpers built directly on `store::*` functions,
//! per the plan's "no live infra beyond docker-compose postgres/redis"
//! design (the one exception, worker_deposit_withdrawal_e2e, is gated
//! separately - see that file).
//!
//! Tests run concurrently by default (`cargo test`'s normal behavior), so
//! every test must create its own user/market rows rather than sharing
//! global fixtures - there is no truncate-between-tests reset here, only
//! unique usernames per test (see `unique_username`).

use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use store::models::{MarketSymbol, User};

/// Falls back to the docker-compose default connection string (see
/// docker-compose.yml's postgres service) if TEST_DATABASE_URL isn't set,
/// so `cargo test -p perp-integration-tests` works out of the box against
/// `docker compose up -d postgres`.
pub fn test_database_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into())
}

/// Connects and runs every migration in ../migrations (idempotent - sqlx
/// tracks applied migrations in `_sqlx_migrations`, so calling this from
/// every test file that needs a pool is safe and cheap after the first
/// call).
pub async fn connect_test_pool() -> PgPool {
    let pool = PgPool::connect(&test_database_url())
        .await
        .expect("failed to connect to TEST_DATABASE_URL - is `docker compose up -d postgres` running?");
    sqlx::migrate!("../migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations against test database");
    pool
}

static USERNAME_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A username unique within this test process run, so concurrent tests
/// creating users never collide on the `users.username` UNIQUE constraint.
pub fn unique_username(prefix: &str) -> String {
    let n = USERNAME_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    format!("{prefix}_{nanos}_{n}")
}

/// Inserts a free (unassigned) deposit address, required before
/// `create_user_with_deposit_address` will succeed (see
/// crates/store/src/users.rs - it claims a row with `user_id IS NULL AND
/// is_active = TRUE` via `FOR UPDATE SKIP LOCKED`, and errors with "pool
/// exhausted" if none exists).
pub async fn insert_free_deposit_address(pool: &PgPool, pubkey: &str) {
    sqlx::query("INSERT INTO deposit_addresses (pubkey) VALUES ($1) ON CONFLICT (pubkey) DO NOTHING")
        .bind(pubkey)
        .execute(pool)
        .await
        .expect("failed to insert free deposit address fixture");
}

/// Creates a fully-onboarded test user: seeds a free deposit address (a
/// deterministic-but-unique fake pubkey, since the real chain is never
/// touched by these tests) then signs the user up through the real
/// `store::users::create_user_with_deposit_address` path.
pub async fn create_test_user(pool: &PgPool, username_prefix: &str) -> User {
    let username = unique_username(username_prefix);
    let pubkey = format!("TEST{}", unique_username("addr"));
    let pubkey = pubkey.chars().take(44).collect::<String>();
    insert_free_deposit_address(pool, &pubkey).await;

    store::users::create_user_with_deposit_address(pool, &username, "test-password-123")
        .await
        .expect("failed to create test user fixture")
}

/// Credits a user's `collateral_available` directly (bypassing the deposit
/// pipeline entirely) - tests that only care about "this user has balance
/// to trade/withdraw with" shouldn't have to route through deposit
/// indexing to set that up.
pub async fn credit_balance(pool: &PgPool, user_id: i32, amount: Decimal) {
    sqlx::query("UPDATE users SET collateral_available = collateral_available + $1 WHERE id = $2")
        .bind(amount)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("failed to credit test user balance");
}

/// Ensures a market row exists and is active with a known mark price, using
/// the same `ON CONFLICT` upsert shape as `crates/seeder/src/bin/seed_markets.rs`
/// so tests don't depend on that binary having been run first.
pub async fn seed_test_market(pool: &PgPool, market: MarketSymbol, mark_price: Decimal) {
    sqlx::query(
        "INSERT INTO markets (market, tick_size, lot_size, max_leverage, initial_margin_rate, maintenance_margin_rate, mark_price, mark_price_updated_at)
         VALUES ($1, 0.01, 0.01, 20, 0.05, 0.025, $2, NOW())
         ON CONFLICT (market) DO UPDATE SET
            is_active = TRUE,
            mark_price = EXCLUDED.mark_price,
            mark_price_updated_at = NOW()",
    )
    .bind(market)
    .bind(mark_price)
    .execute(pool)
    .await
    .expect("failed to seed test market fixture");
}
