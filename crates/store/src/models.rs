use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize)]
#[sqlx(type_name = "withdrawal_status", rename_all = "UPPERCASE")]
pub enum WithdrawalStatus {
    Queued,
    Submitting,
    Submitted,
    Confirmed,
    Failed,
    Refunded,
}

#[derive(Debug, FromRow, Serialize)]
pub struct WithdrawalRequest {
    pub id: i32,
    pub user_id: i32,
    pub amount: rust_decimal::Decimal,
    pub destination_pubkey: String,
    pub status: WithdrawalStatus,
    pub signature: Option<String>,
    #[serde(skip_serializing)]
    pub signed_tx_bytes: Option<Vec<u8>>,
    pub nonce_account: Option<String>,
    pub nonce_hash: Option<String>,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub submitted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, FromRow)]
pub struct OutboxEvent {
    pub id: i64,
    pub withdrawal_request_id: i32,
}

#[derive(Debug, FromRow, Serialize)]
pub struct DbUser {
    pub id: i32,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub collateral_available: rust_decimal::Decimal,
    pub collateral_locked: rust_decimal::Decimal,
}

#[derive(Debug, FromRow, Serialize)]
pub struct User {
    pub id: i32,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub collateral_available: rust_decimal::Decimal,
    pub collateral_locked: rust_decimal::Decimal,
    pub pubkey: Option<String>,
}
