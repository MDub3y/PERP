use serde::{Deserialize, Serialize};
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

// ============================================================
// Matching engine domain types
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "market_name", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum MarketSymbol {
    Sol,
    Eth,
    Btc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "order_variant", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderVariant {
    Long,
    Short,
}

impl OrderVariant {
    pub fn opposite(self) -> Self {
        match self {
            Self::Long => Self::Short,
            Self::Short => Self::Long,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "order_type", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum OrderType {
    Limit,
    Market,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "time_in_force", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum TimeInForce {
    Gtc,
    Ioc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "margin_mode", rename_all = "UPPERCASE")]
#[serde(rename_all = "UPPERCASE")]
pub enum MarginMode {
    Cross,
    Isolated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize)]
#[sqlx(type_name = "order_status", rename_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OrderStatus {
    Pending,
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

impl OrderStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }
}

#[derive(Debug, FromRow, Serialize)]
pub struct Order {
    pub id: i64,
    pub user_id: i32,
    pub market: MarketSymbol,
    pub variant: OrderVariant,
    pub order_type: OrderType,
    pub tif: TimeInForce,
    pub reduce_only: bool,
    pub leverage: i16,
    pub margin_mode: MarginMode,
    pub price: Option<rust_decimal::Decimal>,
    pub quantity: rust_decimal::Decimal,
    pub remaining_qty: rust_decimal::Decimal,
    pub reserved_margin: rust_decimal::Decimal,
    pub status: OrderStatus,
    pub is_liquidation: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, FromRow)]
pub struct OrderOutboxEvent {
    pub id: i64,
    pub order_id: i64,
}

#[derive(Debug, FromRow, Serialize)]
pub struct Position {
    pub id: i64,
    pub user_id: i32,
    pub market: MarketSymbol,
    pub variant: OrderVariant,
    pub margin_mode: MarginMode,
    pub leverage: i16,
    pub quantity: rust_decimal::Decimal,
    pub average_price: rust_decimal::Decimal,
    pub allocated_margin: rust_decimal::Decimal,
    pub realized_pnl: rust_decimal::Decimal,
    pub funding_pnl: rust_decimal::Decimal,
    pub is_liquidating: bool,
    pub liquidation_flagged_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Market {
    pub market: MarketSymbol,
    pub tick_size: rust_decimal::Decimal,
    pub lot_size: rust_decimal::Decimal,
    pub max_leverage: i16,
    pub initial_margin_rate: rust_decimal::Decimal,
    pub maintenance_margin_rate: rust_decimal::Decimal,
    pub backstop_equity_ratio: rust_decimal::Decimal,
    pub liquidation_fee_rate: rust_decimal::Decimal,
    pub impact_notional: rust_decimal::Decimal,
    pub is_active: bool,
    pub mark_price: Option<rust_decimal::Decimal>,
    pub mark_price_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct FeeTier {
    pub tier: i16,
    pub volume_threshold_30d: rust_decimal::Decimal,
    pub taker_rate: rust_decimal::Decimal,
    pub maker_rate: rust_decimal::Decimal,
}
