use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, FromRow, Serialize)]
pub struct User {
    pub id: i32,
    pub username: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub collateral_available: rust_decimal::Decimal,
    pub collateral_locked: rust_decimal::Decimal,
    pub pubkey: String,
}
