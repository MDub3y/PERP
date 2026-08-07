use crate::models::{OutboxEvent, WithdrawalRequest, WithdrawalStatus};
use rust_decimal::Decimal;
use sqlx::{PgPool, Postgres, Transaction};
use std::fmt;

#[derive(Debug)]
pub enum RequestWithdrawalError {
    InsufficientBalance,
    RateLimitExceeded,
    Database(sqlx::Error),
}

impl fmt::Display for RequestWithdrawalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientBalance => write!(f, "insufficient collateral_available balance"),
            Self::RateLimitExceeded => write!(f, "withdrawal rate limit exceeded"),
            Self::Database(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for RequestWithdrawalError {}

impl From<sqlx::Error> for RequestWithdrawalError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

/// One atomic transaction: row-locks the user, enforces the 24h rate limit,
/// checks + debits collateral_available, and inserts both the
/// withdrawal_requests row and its withdrawal_outbox row. This is the only
/// writer of withdrawal_requests rows, so the `FOR UPDATE` row lock below
/// serializes every concurrent withdrawal request from a given user and
/// prevents a check-then-lock race across separate transactions.
pub async fn request_withdrawal(
    pool: &PgPool,
    user_id: i32,
    amount: Decimal,
    destination_pubkey: &str,
    rate_limit_per_day: i64,
) -> Result<WithdrawalRequest, RequestWithdrawalError> {
    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    let available: Decimal =
        sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;

    let recent_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM withdrawal_requests
         WHERE user_id = $1 AND created_at > NOW() - INTERVAL '24 hours'",
    )
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;

    if recent_count >= rate_limit_per_day {
        return Err(RequestWithdrawalError::RateLimitExceeded);
    }
    if available < amount {
        return Err(RequestWithdrawalError::InsufficientBalance);
    }

    sqlx::query("UPDATE users SET collateral_available = collateral_available - $1 WHERE id = $2")
        .bind(amount)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    let request = sqlx::query_as::<_, WithdrawalRequest>(
        "INSERT INTO withdrawal_requests (user_id, amount, destination_pubkey, status)
         VALUES ($1, $2, $3, 'QUEUED')
         RETURNING *",
    )
    .bind(user_id)
    .bind(amount)
    .bind(destination_pubkey)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO withdrawal_outbox (withdrawal_request_id, event_type, payload)
         VALUES ($1, 'WITHDRAWAL_QUEUED', $2)",
    )
    .bind(request.id)
    .bind(serde_json::json!({ "withdrawal_request_id": request.id }))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(request)
}

pub async fn fetch_withdrawals_for_user(
    pool: &PgPool,
    user_id: i32,
) -> Result<Vec<WithdrawalRequest>, sqlx::Error> {
    sqlx::query_as::<_, WithdrawalRequest>(
        "SELECT * FROM withdrawal_requests WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

// ---- transactional-outbox relay support ----

pub async fn fetch_unprocessed_outbox_events(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<OutboxEvent>, sqlx::Error> {
    sqlx::query_as::<_, OutboxEvent>(
        "SELECT id, withdrawal_request_id FROM withdrawal_outbox
         WHERE processed_at IS NULL ORDER BY id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn mark_outbox_processed(pool: &PgPool, outbox_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE withdrawal_outbox SET processed_at = NOW() WHERE id = $1")
        .bind(outbox_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---- worker read/transition helpers ----

pub async fn fetch_withdrawal_request_for_update(
    tx: &mut Transaction<'_, Postgres>,
    id: i32,
) -> Result<WithdrawalRequest, sqlx::Error> {
    sqlx::query_as::<_, WithdrawalRequest>(
        "SELECT * FROM withdrawal_requests WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await
}

/// Same FOR UPDATE SKIP LOCKED idiom used for deposit_addresses assignment.
pub async fn claim_free_nonce_account(
    tx: &mut Transaction<'_, Postgres>,
    request_id: i32,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE fee_payer_nonces
         SET is_locked = TRUE, locked_by_request_id = $1
         WHERE nonce_pubkey = (
             SELECT nonce_pubkey FROM fee_payer_nonces
             WHERE is_locked = FALSE
             LIMIT 1
             FOR UPDATE SKIP LOCKED
         )
         RETURNING nonce_pubkey",
    )
    .bind(request_id)
    .fetch_optional(&mut **tx)
    .await?;

    Ok(row.map(|(pubkey,)| pubkey))
}

pub async fn release_nonce_account(pool: &PgPool, nonce_pubkey: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE fee_payer_nonces SET is_locked = FALSE, locked_by_request_id = NULL
         WHERE nonce_pubkey = $1",
    )
    .bind(nonce_pubkey)
    .execute(pool)
    .await?;
    Ok(())
}

/// Guarded transition QUEUED -> SUBMITTING. The signature + signed bytes are
/// persisted here, BEFORE the caller ever broadcasts the transaction, so a
/// crash between broadcast and queue-ack still leaves a durable record of
/// exactly what was (or was about to be) sent.
pub async fn mark_submitting(
    pool: &PgPool,
    id: i32,
    signature: &str,
    signed_tx_bytes: &[u8],
    nonce_account: &str,
    nonce_hash: &str,
) -> Result<Option<WithdrawalRequest>, sqlx::Error> {
    sqlx::query_as::<_, WithdrawalRequest>(
        "UPDATE withdrawal_requests
         SET status = 'SUBMITTING', signature = $2, signed_tx_bytes = $3,
             nonce_account = $4, nonce_hash = $5, submitted_at = NOW()
         WHERE id = $1 AND status = 'QUEUED'
         RETURNING *",
    )
    .bind(id)
    .bind(signature)
    .bind(signed_tx_bytes)
    .bind(nonce_account)
    .bind(nonce_hash)
    .fetch_optional(pool)
    .await
}

pub async fn mark_submitted(pool: &PgPool, id: i32) -> Result<Option<WithdrawalRequest>, sqlx::Error> {
    sqlx::query_as::<_, WithdrawalRequest>(
        "UPDATE withdrawal_requests SET status = 'SUBMITTED'
         WHERE id = $1 AND status IN ('SUBMITTING', 'SUBMITTED')
         RETURNING *",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn mark_confirmed(pool: &PgPool, id: i32) -> Result<Option<WithdrawalRequest>, sqlx::Error> {
    let request = sqlx::query_as::<_, WithdrawalRequest>(
        "UPDATE withdrawal_requests SET status = 'CONFIRMED', confirmed_at = NOW()
         WHERE id = $1 AND status IN ('SUBMITTING', 'SUBMITTED')
         RETURNING *",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    if let Some(req) = &request {
        if let Some(nonce) = &req.nonce_account {
            release_nonce_account(pool, nonce).await?;
        }
    }

    Ok(request)
}

/// Atomic: mark REFUNDED and credit collateral_available back, in one
/// transaction, so a request is never externally observable as "failed but
/// not yet refunded."
pub async fn fail_and_refund(
    pool: &PgPool,
    id: i32,
    error_message: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let row: Option<(i32, Decimal, Option<String>)> = sqlx::query_as(
        "UPDATE withdrawal_requests SET status = 'REFUNDED', error_message = $2
         WHERE id = $1 AND status IN ('QUEUED', 'SUBMITTING', 'SUBMITTED')
         RETURNING user_id, amount, nonce_account",
    )
    .bind(id)
    .bind(error_message)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some((user_id, amount, nonce_account)) = row {
        sqlx::query("UPDATE users SET collateral_available = collateral_available + $1 WHERE id = $2")
            .bind(amount)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;

        if let Some(nonce) = nonce_account {
            sqlx::query(
                "UPDATE fee_payer_nonces SET is_locked = FALSE, locked_by_request_id = NULL
                 WHERE nonce_pubkey = $1",
            )
            .bind(nonce)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await
}

/// Reconciliation sweep: requests that have sat in an in-flight status past
/// `older_than_minutes` get re-driven through the worker's state machine,
/// independent of whether Redis ever delivered (or lost) their entry.
pub async fn fetch_stuck_requests(
    pool: &PgPool,
    statuses: &[WithdrawalStatus],
    older_than_minutes: i64,
) -> Result<Vec<WithdrawalRequest>, sqlx::Error> {
    sqlx::query_as::<_, WithdrawalRequest>(
        "SELECT * FROM withdrawal_requests
         WHERE status = ANY($1) AND updated_at < NOW() - ($2 || ' minutes')::interval",
    )
    .bind(statuses)
    .bind(older_than_minutes.to_string())
    .fetch_all(pool)
    .await
}
