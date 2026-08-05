use crate::models::{
    Market, MarginMode, MarketSymbol, Order, OrderOutboxEvent, OrderType, OrderVariant, Position,
    TimeInForce,
};
use rust_decimal::Decimal;
use sqlx::{PgPool, Postgres, Transaction};
use std::fmt;

/// Conservative reservation-only slippage bound for MARKET orders, whose
/// exact fill price isn't known until the engine matches them. Real
/// reconciliation against actual notional happens in apply_fill once the
/// fill lands; over-reservation here is always safe, under-reservation
/// never is.
const MARKET_ORDER_SLIPPAGE_BUFFER: &str = "0.05";

pub struct PlaceOrderParams {
    pub market: MarketSymbol,
    pub variant: OrderVariant,
    pub order_type: OrderType,
    pub tif: TimeInForce,
    pub reduce_only: bool,
    pub leverage: i16,
    pub margin_mode: MarginMode,
    pub price: Option<Decimal>,
    pub quantity: Decimal,
}

#[derive(Debug)]
pub enum PlaceOrderError {
    MarketInactive,
    InvalidLeverage,
    PriceRequiredForLimit,
    PriceMustBeAbsentForMarket,
    PriceNotOnTick,
    QuantityNotOnLot,
    MarketPriceUnavailable,
    InsufficientBalance,
    Database(sqlx::Error),
}

impl fmt::Display for PlaceOrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MarketInactive => write!(f, "market is not active"),
            Self::InvalidLeverage => write!(f, "leverage must be between 1 and the market max"),
            Self::PriceRequiredForLimit => write!(f, "price is required for LIMIT orders"),
            Self::PriceMustBeAbsentForMarket => write!(f, "price must be omitted for MARKET orders"),
            Self::PriceNotOnTick => write!(f, "price does not conform to the market tick size"),
            Self::QuantityNotOnLot => write!(f, "quantity does not conform to the market lot size"),
            Self::MarketPriceUnavailable => {
                write!(f, "market has no mark price yet, cannot size a MARKET order")
            }
            Self::InsufficientBalance => write!(f, "insufficient collateral_available balance"),
            Self::Database(e) => write!(f, "database error: {e}"),
        }
    }
}

impl std::error::Error for PlaceOrderError {}

impl From<sqlx::Error> for PlaceOrderError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

pub async fn fetch_market(pool: &PgPool, market: MarketSymbol) -> Result<Option<Market>, sqlx::Error> {
    sqlx::query_as::<_, Market>("SELECT * FROM markets WHERE market = $1")
        .bind(market)
        .fetch_optional(pool)
        .await
}

/// Last-writer-wins cache update, called by the engine on every trade. Not
/// part of the ledger's guarded-transaction machinery on purpose: this is
/// an estimate/display value (used to size MARKET order margin reservations
/// before the real fill price is known), not authoritative money movement.
pub async fn update_mark_price(pool: &PgPool, market: MarketSymbol, price: Decimal) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE markets SET mark_price = $1, mark_price_updated_at = NOW() WHERE market = $2")
        .bind(price)
        .bind(market)
        .execute(pool)
        .await?;
    Ok(())
}

/// One atomic transaction: validates the order against market config,
/// computes the required margin, row-locks the user, checks + debits
/// collateral_available -> collateral_locked, inserts the orders row AND
/// its orders_outbox row, commits. Mirrors store::withdrawals::
/// request_withdrawal exactly - this is the reservation that prevents
/// double-spending across concurrent order placements, done entirely in
/// Postgres before the engine ever sees the order.
pub async fn place_order(
    pool: &PgPool,
    user_id: i32,
    params: PlaceOrderParams,
) -> Result<Order, PlaceOrderError> {
    match params.order_type {
        OrderType::Limit if params.price.is_none() => {
            return Err(PlaceOrderError::PriceRequiredForLimit);
        }
        OrderType::Market if params.price.is_some() => {
            return Err(PlaceOrderError::PriceMustBeAbsentForMarket);
        }
        _ => {}
    }

    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    let market = sqlx::query_as::<_, Market>("SELECT * FROM markets WHERE market = $1")
        .bind(params.market)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(PlaceOrderError::MarketInactive)?;

    if !market.is_active {
        return Err(PlaceOrderError::MarketInactive);
    }
    if params.leverage < 1 || params.leverage > market.max_leverage {
        return Err(PlaceOrderError::InvalidLeverage);
    }
    if params.quantity % market.lot_size != Decimal::ZERO {
        return Err(PlaceOrderError::QuantityNotOnLot);
    }
    if let Some(price) = params.price {
        if price % market.tick_size != Decimal::ZERO {
            return Err(PlaceOrderError::PriceNotOnTick);
        }
    }

    let leverage_decimal = Decimal::from(params.leverage);
    let reserved_margin = match params.order_type {
        OrderType::Limit => {
            let price = params.price.expect("checked above");
            (price * params.quantity) / leverage_decimal
        }
        OrderType::Market => {
            let mark_price = market
                .mark_price
                .ok_or(PlaceOrderError::MarketPriceUnavailable)?;
            let slippage = MARKET_ORDER_SLIPPAGE_BUFFER.parse::<Decimal>().unwrap();
            let estimated_price = match params.variant {
                OrderVariant::Long => mark_price * (Decimal::ONE + slippage),
                OrderVariant::Short => mark_price * (Decimal::ONE - slippage),
            };
            (estimated_price * params.quantity) / leverage_decimal
        }
    };

    let available: Decimal =
        sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1 FOR UPDATE")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;

    if available < reserved_margin {
        return Err(PlaceOrderError::InsufficientBalance);
    }

    sqlx::query(
        "UPDATE users SET collateral_available = collateral_available - $1,
                           collateral_locked = collateral_locked + $1
         WHERE id = $2",
    )
    .bind(reserved_margin)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    let order = sqlx::query_as::<_, Order>(
        "INSERT INTO orders (
            user_id, market, variant, order_type, tif, reduce_only, leverage,
            margin_mode, price, quantity, remaining_qty, reserved_margin, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, $11, 'PENDING')
        RETURNING *",
    )
    .bind(user_id)
    .bind(params.market)
    .bind(params.variant)
    .bind(params.order_type)
    .bind(params.tif)
    .bind(params.reduce_only)
    .bind(params.leverage)
    .bind(params.margin_mode)
    .bind(params.price)
    .bind(params.quantity)
    .bind(reserved_margin)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO orders_outbox (order_id, event_type, payload)
         VALUES ($1, 'ORDER_PLACED', $2)",
    )
    .bind(order.id)
    .bind(serde_json::json!({ "order_id": order.id }))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(order)
}

/// Engine-generated reduce-only IOC unwind order for the liquidation
/// subsystem. Unlike place_order, this bypasses the collateral
/// check/reservation entirely - the margin backing this position is
/// already locked (collateral_locked for cross, positions.allocated_margin
/// for isolated), so there's nothing new to reserve. Still gets a normal
/// orders row (reserved_margin=0, is_liquidation=true) so it shows up in
/// order/trade history like any other order.
pub async fn create_liquidation_order(
    pool: &PgPool,
    user_id: i32,
    market: MarketSymbol,
    variant: OrderVariant,
    quantity: Decimal,
    leverage: i16,
    margin_mode: MarginMode,
) -> Result<Order, sqlx::Error> {
    sqlx::query_as::<_, Order>(
        "INSERT INTO orders (
            user_id, market, variant, order_type, tif, reduce_only, leverage,
            margin_mode, price, quantity, remaining_qty, reserved_margin, status, is_liquidation
        ) VALUES ($1, $2, $3, 'MARKET', 'IOC', TRUE, $4, $5, NULL, $6, $6, 0, 'PENDING', TRUE)
        RETURNING *",
    )
    .bind(user_id)
    .bind(market)
    .bind(variant)
    .bind(leverage)
    .bind(margin_mode)
    .bind(quantity)
    .fetch_one(pool)
    .await
}

#[derive(Debug)]
pub enum CancelError {
    NotFound,
    Database(sqlx::Error),
}

impl fmt::Display for CancelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "order not found, not owned by caller, or already terminal"),
            Self::Database(e) => write!(f, "database error: {e}"),
        }
    }
}
impl std::error::Error for CancelError {}
impl From<sqlx::Error> for CancelError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

/// Guarded cancel: only PENDING/OPEN/PARTIALLY_FILLED orders owned by the
/// caller can be cancelled. Refunds the unfilled proportion of the reserved
/// margin (reserved_margin * remaining_qty / quantity) back to
/// collateral_available in the same transaction, then records an outbox
/// event so the engine also stops matching against it - the Postgres cancel
/// is the authoritative "no more margin reserved" fact; if the engine
/// independently fully-fills it first, that race resolves via the fill
/// event's guarded status transition in the ledger writer.
pub async fn request_cancel(pool: &PgPool, user_id: i32, order_id: i64) -> Result<Order, CancelError> {
    let mut tx = pool.begin().await?;

    let order = sqlx::query_as::<_, Order>(
        "UPDATE orders SET status = 'CANCELLED'
         WHERE id = $1 AND user_id = $2 AND status IN ('PENDING', 'OPEN', 'PARTIALLY_FILLED')
         RETURNING *",
    )
    .bind(order_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(CancelError::NotFound)?;

    let refund = order.reserved_margin * order.remaining_qty / order.quantity;

    sqlx::query(
        "UPDATE users SET collateral_available = collateral_available + $1,
                           collateral_locked = collateral_locked - $1
         WHERE id = $2",
    )
    .bind(refund)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO orders_outbox (order_id, event_type, payload)
         VALUES ($1, 'ORDER_CANCEL_REQUESTED', $2)",
    )
    .bind(order.id)
    .bind(serde_json::json!({ "order_id": order.id }))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(order)
}

// ---- transactional-outbox relay support (mirrors withdrawals) ----

pub async fn fetch_unprocessed_orders_outbox(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<OrderOutboxEvent>, sqlx::Error> {
    sqlx::query_as::<_, OrderOutboxEvent>(
        "SELECT id, order_id FROM orders_outbox
         WHERE processed_at IS NULL ORDER BY id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn mark_orders_outbox_processed(pool: &PgPool, outbox_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE orders_outbox SET processed_at = NOW() WHERE id = $1")
        .bind(outbox_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---- reads ----

pub async fn fetch_open_orders_for_user(pool: &PgPool, user_id: i32) -> Result<Vec<Order>, sqlx::Error> {
    sqlx::query_as::<_, Order>(
        "SELECT * FROM orders
         WHERE user_id = $1 AND status IN ('PENDING', 'OPEN', 'PARTIALLY_FILLED')
         ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

pub async fn fetch_positions_for_user(pool: &PgPool, user_id: i32) -> Result<Vec<Position>, sqlx::Error> {
    sqlx::query_as::<_, Position>("SELECT * FROM positions WHERE user_id = $1 ORDER BY updated_at DESC")
        .bind(user_id)
        .fetch_all(pool)
        .await
}
