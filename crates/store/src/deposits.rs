use rust_decimal::Decimal;
use sqlx::{PgPool, Postgres, Transaction};

/// Idempotently records an on-chain deposit and credits the user's
/// available collateral in the same transaction. `deposits.signature` is
/// UNIQUE NOT NULL, so `ON CONFLICT (signature) DO NOTHING` makes this safe
/// to call repeatedly for the same signature (e.g. across indexer restarts
/// or overlapping polling windows) - only the first call actually credits
/// the user, every later call is a no-op.
///
/// Returns `true` if a new deposit was recorded and the balance credited,
/// `false` if this signature was already processed.
pub async fn record_deposit(
    pool: &PgPool,
    user_id: i32,
    pubkey: &str,
    signature: &str,
    amount: Decimal,
) -> Result<bool, sqlx::Error> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    let result = sqlx::query(
        "INSERT INTO deposits (user_id, pubkey, signature, amount) VALUES ($1, $2, $3, $4)
         ON CONFLICT (signature) DO NOTHING",
    )
    .bind(user_id)
    .bind(pubkey)
    .bind(signature)
    .bind(amount)
    .execute(&mut *tx)
    .await?;

    let inserted = result.rows_affected() > 0;

    if inserted {
        // Row-lock the user before crediting so concurrent deposit/withdrawal
        // activity for the same user serializes on this update, mirroring
        // the FOR UPDATE idiom used throughout withdrawals.rs.
        sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;

        sqlx::query(
            "UPDATE users SET collateral_available = collateral_available + $1 WHERE id = $2",
        )
        .bind(amount)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(inserted)
}

/// All deposit addresses currently assigned to a user, along with each
/// address's polling cursor (the last signature the indexer has already
/// processed, if any).
pub async fn fetch_active_deposit_addresses(
    pool: &PgPool,
) -> Result<Vec<(String, i32, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT pubkey, user_id, last_signature FROM deposit_addresses
         WHERE is_active = TRUE AND user_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await
}

/// Advances an address's polling cursor so the next indexer tick only scans
/// signatures newer than this one.
pub async fn update_last_signature(
    pool: &PgPool,
    pubkey: &str,
    signature: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE deposit_addresses SET last_signature = $1 WHERE pubkey = $2")
        .bind(signature)
        .bind(pubkey)
        .execute(pool)
        .await?;

    Ok(())
}
