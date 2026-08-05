//! Applies engine-produced events (fills, funding, liquidation transfers)
//! back onto Postgres balances/positions. Every `apply_*` function here is
//! its own small guarded transaction, keyed by a monotonic `event_id`
//! cursor (`engine_event_cursor`), so redelivery of an already-applied
//! event from `engine:events` is a no-op - this is what lets the ledger
//! writer be at-least-once on the Redis side while staying exactly-once on
//! the Postgres side.
//!
//! Fee application is folded into `apply_fill` (via `maker_fee`/`taker_fee`
//! on the event) rather than a separate event type - every fee this
//! exchange charges is fill-associated (including the liquidation fee
//! surcharge), so a standalone `apply_fee` would just be `apply_fill` with
//! extra steps.

use crate::models::{MarginMode, MarketSymbol, Order, OrderStatus, OrderVariant, Position};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillEvent {
    pub event_id: i64,
    pub trade_id: i64,
    pub time: DateTime<Utc>,
    pub market: MarketSymbol,
    pub price: Decimal,
    pub quantity: Decimal, // always positive
    pub maker_order_id: i64,
    pub taker_order_id: i64,
    pub maker_user_id: i32,
    pub taker_user_id: i32,
    pub taker_variant: OrderVariant, // maker's variant is the opposite
    pub maker_fee: Decimal,          // signed: negative = rebate paid to the maker
    pub taker_fee: Decimal,
    pub is_liquidation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FundingPaymentEvent {
    pub event_id: i64,
    pub settlement_time: DateTime<Utc>,
    pub market: MarketSymbol,
    pub user_id: i32,
    pub position_qty: Decimal,
    pub funding_rate_hour: Decimal,
    pub amount: Decimal, // signed: credit positive, debit negative
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidationTransferEvent {
    pub event_id: i64,
    pub user_id: i32,
    pub market: MarketSymbol,
    pub mark_price: Decimal,
}

/// Emitted when a GTC LIMIT order rests with zero fills - the only case a
/// fill event never touches (any order with at least one fill already gets
/// its status set to OPEN/PARTIALLY_FILLED/FILLED by apply_fill_side).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRestedEvent {
    pub event_id: i64,
    pub order_id: i64,
}

/// Emitted for IOC/MARKET orders whose unmatched remainder was dropped
/// rather than rested (whether or not any of the order filled first).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderCancelledRemainderEvent {
    pub event_id: i64,
    pub order_id: i64,
}

/// Emitted when the engine rejects an order on intake because the
/// account/market is flagged for liquidation (order never got a chance to
/// match at all - full margin refund).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRejectedEvent {
    pub event_id: i64,
    pub order_id: i64,
}

/// Advances engine_event_cursor iff `event_id` is newer than what's already
/// applied. Returns false (and the transaction is rolled back by the
/// caller) if this event was already applied - the standard idempotency
/// guard every apply_* function starts with.
async fn advance_cursor(tx: &mut Transaction<'_, Postgres>, event_id: i64) -> Result<bool, sqlx::Error> {
    let advanced: Option<(i64,)> = sqlx::query_as(
        "UPDATE engine_event_cursor SET last_applied_event_id = $1
         WHERE last_applied_event_id < $1
         RETURNING last_applied_event_id",
    )
    .bind(event_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(advanced.is_some())
}

/// Net position update for one side of a fill. Handles opening, adding,
/// partial-closing, full-closing, and flipping uniformly: `closing_qty` is
/// the portion of this fill that reduced existing opposite exposure,
/// `opening_qty` is the portion that added new same-direction exposure
/// (either a plain add, or the flip-through remainder after a close).
struct PositionUpdate {
    new_qty: Decimal,
    new_avg: Decimal,
    realized_pnl_delta: Decimal,
    closing_qty: Decimal,
}

fn net_position(existing_qty: Decimal, existing_avg: Decimal, delta: Decimal, fill_price: Decimal) -> PositionUpdate {
    let opening_only = existing_qty == Decimal::ZERO || (existing_qty > Decimal::ZERO) == (delta > Decimal::ZERO);

    if opening_only {
        let new_qty = existing_qty + delta;
        let new_avg = if new_qty == Decimal::ZERO {
            existing_avg
        } else {
            (existing_qty.abs() * existing_avg + delta.abs() * fill_price) / new_qty.abs()
        };
        return PositionUpdate {
            new_qty,
            new_avg,
            realized_pnl_delta: Decimal::ZERO,
            closing_qty: Decimal::ZERO,
        };
    }

    let closing_qty = delta.abs().min(existing_qty.abs());
    let opening_qty = delta.abs() - closing_qty;
    let sign = if existing_qty > Decimal::ZERO { Decimal::ONE } else { -Decimal::ONE };
    let realized_pnl_delta = closing_qty * (fill_price - existing_avg) * sign;
    let new_qty = existing_qty + delta;
    let new_avg = if opening_qty > Decimal::ZERO {
        fill_price
    } else if new_qty == Decimal::ZERO {
        Decimal::ZERO
    } else {
        existing_avg
    };

    PositionUpdate {
        new_qty,
        new_avg,
        realized_pnl_delta,
        closing_qty,
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_fill_side(
    tx: &mut Transaction<'_, Postgres>,
    user_id: i32,
    market: MarketSymbol,
    order_id: i64,
    fill_qty: Decimal,
    fill_price: Decimal,
    variant: OrderVariant,
    fee: Decimal,
) -> Result<(), sqlx::Error> {
    let order = sqlx::query_as::<_, Order>("SELECT * FROM orders WHERE id = $1 FOR UPDATE")
        .bind(order_id)
        .fetch_one(&mut **tx)
        .await?;

    let new_remaining = (order.remaining_qty - fill_qty).max(Decimal::ZERO);
    let new_status = if new_remaining == Decimal::ZERO {
        OrderStatus::Filled
    } else {
        OrderStatus::PartiallyFilled
    };
    sqlx::query("UPDATE orders SET remaining_qty = $1, status = $2 WHERE id = $3")
        .bind(new_remaining)
        .bind(new_status)
        .bind(order_id)
        .execute(&mut **tx)
        .await?;

    let order_margin_portion = if order.quantity == Decimal::ZERO {
        Decimal::ZERO
    } else {
        order.reserved_margin * fill_qty / order.quantity
    };

    let existing = sqlx::query_as::<_, Position>(
        "SELECT * FROM positions WHERE user_id = $1 AND market = $2 FOR UPDATE",
    )
    .bind(user_id)
    .bind(market)
    .fetch_optional(&mut **tx)
    .await?;

    let (existing_qty, existing_avg, existing_allocated) = match &existing {
        Some(p) => (p.quantity, p.average_price, p.allocated_margin),
        None => (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO),
    };

    let signed_delta = if variant == OrderVariant::Long { fill_qty } else { -fill_qty };
    let update = net_position(existing_qty, existing_avg, signed_delta, fill_price);

    let closing_margin_portion = if fill_qty == Decimal::ZERO {
        Decimal::ZERO
    } else {
        order_margin_portion * update.closing_qty / fill_qty
    };
    let opening_margin_portion = order_margin_portion - closing_margin_portion;

    // Every order's reserved margin leaves collateral_locked once filled -
    // it's either returned to available (cross; or the closing leg of an
    // isolated order) or re-homed into the position's allocated_margin (the
    // opening leg of an isolated order).
    let mut collateral_delta = Decimal::ZERO;
    let locked_delta = -order_margin_portion;
    let mut new_allocated = existing_allocated;

    match order.margin_mode {
        MarginMode::Cross => {
            collateral_delta += order_margin_portion;
        }
        MarginMode::Isolated => {
            if update.closing_qty > Decimal::ZERO && existing_qty != Decimal::ZERO {
                let released_allocated = existing_allocated * update.closing_qty / existing_qty.abs();
                new_allocated -= released_allocated;
                collateral_delta += released_allocated;
            }
            collateral_delta += closing_margin_portion;
            new_allocated += opening_margin_portion;
        }
    }

    collateral_delta += update.realized_pnl_delta;
    collateral_delta -= fee;

    sqlx::query(
        "UPDATE users SET collateral_available = collateral_available + $1,
                           collateral_locked = collateral_locked + $2
         WHERE id = $3",
    )
    .bind(collateral_delta)
    .bind(locked_delta)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;

    if update.new_qty == Decimal::ZERO {
        sqlx::query("DELETE FROM positions WHERE user_id = $1 AND market = $2")
            .bind(user_id)
            .bind(market)
            .execute(&mut **tx)
            .await?;
    } else {
        let new_variant = if update.new_qty > Decimal::ZERO {
            OrderVariant::Long
        } else {
            OrderVariant::Short
        };
        sqlx::query(
            "INSERT INTO positions (user_id, market, variant, margin_mode, leverage, quantity, average_price, allocated_margin, realized_pnl)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (user_id, market) DO UPDATE SET
                variant = EXCLUDED.variant,
                margin_mode = EXCLUDED.margin_mode,
                leverage = EXCLUDED.leverage,
                quantity = EXCLUDED.quantity,
                average_price = EXCLUDED.average_price,
                allocated_margin = EXCLUDED.allocated_margin,
                realized_pnl = positions.realized_pnl + $9",
        )
        .bind(user_id)
        .bind(market)
        .bind(new_variant)
        .bind(order.margin_mode)
        .bind(order.leverage)
        .bind(update.new_qty)
        .bind(update.new_avg)
        .bind(new_allocated)
        .bind(update.realized_pnl_delta)
        .execute(&mut **tx)
        .await?;
    }

    if fee != Decimal::ZERO {
        sqlx::query(
            "UPDATE users SET collateral_available = collateral_available + $1 WHERE username = '__insurance_fund__'",
        )
        .bind(fee)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

/// Applies one fill to both sides (maker + taker) plus the trades hypertable
/// row, all in one transaction. Returns `Ok(false)` without side effects if
/// `event.event_id` was already applied.
pub async fn apply_fill(pool: &PgPool, event: &FillEvent) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    if !advance_cursor(&mut tx, event.event_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }

    let maker_variant = event.taker_variant.opposite();

    apply_fill_side(
        &mut tx,
        event.taker_user_id,
        event.market,
        event.taker_order_id,
        event.quantity,
        event.price,
        event.taker_variant,
        event.taker_fee,
    )
    .await?;

    apply_fill_side(
        &mut tx,
        event.maker_user_id,
        event.market,
        event.maker_order_id,
        event.quantity,
        event.price,
        maker_variant,
        event.maker_fee,
    )
    .await?;

    sqlx::query(
        "INSERT INTO trades (
            time, trade_id, market, price, quantity, maker_order_id, taker_order_id,
            maker_user_id, taker_user_id, taker_variant, maker_fee, taker_fee, is_liquidation
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
         ON CONFLICT (trade_id, time) DO NOTHING",
    )
    .bind(event.time)
    .bind(event.trade_id)
    .bind(event.market)
    .bind(event.price)
    .bind(event.quantity)
    .bind(event.maker_order_id)
    .bind(event.taker_order_id)
    .bind(event.maker_user_id)
    .bind(event.taker_user_id)
    .bind(event.taker_variant)
    .bind(event.maker_fee)
    .bind(event.taker_fee)
    .bind(event.is_liquidation)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

/// Settles one position's funding payment. Idempotent both via the event
/// cursor and via funding_payments' own UNIQUE(market, user_id,
/// settlement_time), belt-and-suspenders since funding settlement is
/// replay-sensitive (double-applying would double-pay).
pub async fn apply_funding_payment(pool: &PgPool, event: &FundingPaymentEvent) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    if !advance_cursor(&mut tx, event.event_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }

    let inserted = sqlx::query(
        "INSERT INTO funding_payments (settlement_time, market, user_id, position_qty, funding_rate_hour, amount)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (market, user_id, settlement_time) DO NOTHING",
    )
    .bind(event.settlement_time)
    .bind(event.market)
    .bind(event.user_id)
    .bind(event.position_qty)
    .bind(event.funding_rate_hour)
    .bind(event.amount)
    .execute(&mut *tx)
    .await?;

    if inserted.rows_affected() > 0 {
        sqlx::query("UPDATE users SET collateral_available = collateral_available + $1 WHERE id = $2")
            .bind(event.amount)
            .bind(event.user_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query("UPDATE positions SET funding_pnl = funding_pnl + $1 WHERE user_id = $2 AND market = $3")
            .bind(event.amount)
            .bind(event.user_id)
            .bind(event.market)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(true)
}

/// Insurance-fund backstop: absorbs the account's remaining equity (cross)
/// or the position's remaining allocated margin (isolated) plus the
/// position itself into the insurance-fund account, via the same
/// positions/collateral machinery used everywhere else.
pub async fn apply_liquidation_transfer(pool: &PgPool, event: &LiquidationTransferEvent) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    if !advance_cursor(&mut tx, event.event_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }

    let position = sqlx::query_as::<_, Position>(
        "SELECT * FROM positions WHERE user_id = $1 AND market = $2 FOR UPDATE",
    )
    .bind(event.user_id)
    .bind(event.market)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(position) = position else {
        tx.commit().await?;
        return Ok(true);
    };

    let (insurance_fund_id,): (i32,) =
        sqlx::query_as("SELECT id FROM users WHERE username = '__insurance_fund__'")
            .fetch_one(&mut *tx)
            .await?;

    let transfer_amount = match position.margin_mode {
        MarginMode::Isolated => position.allocated_margin,
        MarginMode::Cross => {
            let (avail, locked): (Decimal, Decimal) = sqlx::query_as(
                "SELECT collateral_available, collateral_locked FROM users WHERE id = $1 FOR UPDATE",
            )
            .bind(event.user_id)
            .fetch_one(&mut *tx)
            .await?;
            avail + locked
        }
    };

    match position.margin_mode {
        MarginMode::Isolated => {
            sqlx::query("UPDATE users SET collateral_locked = collateral_locked - $1 WHERE id = $2")
                .bind(position.allocated_margin)
                .bind(event.user_id)
                .execute(&mut *tx)
                .await?;
        }
        MarginMode::Cross => {
            sqlx::query("UPDATE users SET collateral_available = 0, collateral_locked = 0 WHERE id = $1")
                .bind(event.user_id)
                .execute(&mut *tx)
                .await?;
        }
    }

    sqlx::query("UPDATE users SET collateral_available = collateral_available + $1 WHERE id = $2")
        .bind(transfer_amount)
        .bind(insurance_fund_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM positions WHERE user_id = $1 AND market = $2")
        .bind(event.user_id)
        .bind(event.market)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO positions (user_id, market, variant, margin_mode, leverage, quantity, average_price, allocated_margin)
         VALUES ($1, $2, $3, 'CROSS', $4, $5, $6, 0)
         ON CONFLICT (user_id, market) DO UPDATE SET
            quantity = positions.quantity + EXCLUDED.quantity,
            variant = CASE WHEN positions.quantity + EXCLUDED.quantity >= 0 THEN 'LONG'::order_variant ELSE 'SHORT'::order_variant END,
            average_price = EXCLUDED.average_price",
    )
    .bind(insurance_fund_id)
    .bind(event.market)
    .bind(position.variant)
    .bind(position.leverage)
    .bind(position.quantity)
    .bind(event.mark_price)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(true)
}

pub async fn apply_order_rested(pool: &PgPool, event: &OrderRestedEvent) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    if !advance_cursor(&mut tx, event.event_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }

    sqlx::query("UPDATE orders SET status = 'OPEN' WHERE id = $1 AND status = 'PENDING'")
        .bind(event.order_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(true)
}

/// Cancels the order and refunds the unfilled proportion of its reserved
/// margin - same formula as store::orders::request_cancel, applied here for
/// the engine-initiated (IOC/MARKET remainder) cancel path instead of the
/// user-initiated one. Whichever of the two paths gets there first wins the
/// guarded status transition; the other becomes a no-op, so a client
/// DELETE racing an IOC resolution can never double-refund.
pub async fn apply_order_cancelled_remainder(
    pool: &PgPool,
    event: &OrderCancelledRemainderEvent,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    if !advance_cursor(&mut tx, event.event_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }

    let order = sqlx::query_as::<_, Order>(
        "UPDATE orders SET status = 'CANCELLED'
         WHERE id = $1 AND status IN ('PENDING', 'OPEN', 'PARTIALLY_FILLED')
         RETURNING *",
    )
    .bind(event.order_id)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(order) = order {
        let refund = order.reserved_margin * order.remaining_qty / order.quantity;
        if refund != Decimal::ZERO {
            sqlx::query(
                "UPDATE users SET collateral_available = collateral_available + $1,
                                   collateral_locked = collateral_locked - $1
                 WHERE id = $2",
            )
            .bind(refund)
            .bind(order.user_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(true)
}

/// Same shape as apply_order_cancelled_remainder, but for orders the engine
/// never even attempted to match because the account/market was flagged for
/// liquidation - sets status = REJECTED (distinct from CANCELLED) since the
/// order never had a chance to trade at all.
pub async fn apply_order_rejected(pool: &PgPool, event: &OrderRejectedEvent) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;

    if !advance_cursor(&mut tx, event.event_id).await? {
        tx.rollback().await?;
        return Ok(false);
    }

    let order = sqlx::query_as::<_, Order>(
        "UPDATE orders SET status = 'REJECTED'
         WHERE id = $1 AND status IN ('PENDING', 'OPEN', 'PARTIALLY_FILLED')
         RETURNING *",
    )
    .bind(event.order_id)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(order) = order {
        let refund = order.reserved_margin * order.remaining_qty / order.quantity;
        if refund != Decimal::ZERO {
            sqlx::query(
                "UPDATE users SET collateral_available = collateral_available + $1,
                                   collateral_locked = collateral_locked - $1
                 WHERE id = $2",
            )
            .bind(refund)
            .bind(order.user_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;
    Ok(true)
}

// ---- crash-recovery snapshots ----

const SNAPSHOTS_TO_KEEP: i64 = 5;

pub async fn save_snapshot(pool: &PgPool, payload: &[u8], last_event_id: i64) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    sqlx::query("INSERT INTO engine_snapshots (last_event_id, payload) VALUES ($1, $2)")
        .bind(last_event_id)
        .bind(payload)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "DELETE FROM engine_snapshots WHERE id NOT IN (
            SELECT id FROM engine_snapshots ORDER BY id DESC LIMIT $1
         )",
    )
    .bind(SNAPSHOTS_TO_KEEP)
    .execute(&mut *tx)
    .await?;

    tx.commit().await
}

pub async fn load_latest_snapshot(pool: &PgPool) -> Result<Option<(Vec<u8>, i64)>, sqlx::Error> {
    let row: Option<(Vec<u8>, i64)> = sqlx::query_as(
        "SELECT payload, last_event_id FROM engine_snapshots ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ---- fee tiers ----

/// Recomputes trailing 30-day volume per account from `trades` and updates
/// `users.fee_tier` for anyone whose tier changed. Meant to be called once
/// per UTC day.
pub async fn refresh_fee_tiers(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "WITH volume AS (
            SELECT user_id, SUM(price * quantity) AS vol FROM (
                SELECT maker_user_id AS user_id, price, quantity FROM trades WHERE time > NOW() - INTERVAL '30 days'
                UNION ALL
                SELECT taker_user_id AS user_id, price, quantity FROM trades WHERE time > NOW() - INTERVAL '30 days'
            ) legs
            GROUP BY user_id
        ), tiered AS (
            SELECT v.user_id, MAX(ft.tier) AS tier
            FROM volume v
            JOIN fee_tiers ft ON ft.volume_threshold_30d <= v.vol
            GROUP BY v.user_id
        )
        UPDATE users SET fee_tier = tiered.tier, fee_tier_updated_at = NOW()
        FROM tiered
        WHERE users.id = tiered.user_id AND users.fee_tier IS DISTINCT FROM tiered.tier",
    )
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

// ---- funding ----

pub async fn insert_funding_sample(
    pool: &PgPool,
    market: MarketSymbol,
    index_price: Decimal,
    bid_impact: Decimal,
    ask_impact: Decimal,
    premium_index: Decimal,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO funding_rate_samples (time, market, index_price, bid_impact, ask_impact, premium_index)
         VALUES (NOW(), $1, $2, $3, $4, $5)",
    )
    .bind(market)
    .bind(index_price)
    .bind(bid_impact)
    .bind(ask_impact)
    .bind(premium_index)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mean PremiumIndex over samples since `since` - the mean_P input to the
/// hourly funding-rate formula.
pub async fn mean_premium_index(
    pool: &PgPool,
    market: MarketSymbol,
    since: DateTime<Utc>,
) -> Result<Option<Decimal>, sqlx::Error> {
    let row: (Option<Decimal>,) =
        sqlx::query_as("SELECT AVG(premium_index) FROM funding_rate_samples WHERE market = $1 AND time >= $2")
            .bind(market)
            .bind(since)
            .fetch_one(pool)
            .await?;
    Ok(row.0)
}
