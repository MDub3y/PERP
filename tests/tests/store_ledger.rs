mod common;

use chrono::Utc;
use common::{create_test_user, credit_balance, seed_test_market};
use rust_decimal::Decimal;
use store::ledger::{FundingPaymentEvent, LiquidationTransferEvent};
use store::models::MarketSymbol;

/// `engine_event_cursor` is a single global monotonic counter (see
/// crate::ledger's module doc), shared across every concurrently-running
/// test in this binary - so each test needs event ids guaranteed to be
/// strictly increasing relative to whatever else has already run. A
/// nanosecond timestamp is unique-enough and always greater than any
/// earlier call within the same test process.
fn fresh_event_id() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or(0)
}

#[tokio::test]
async fn apply_funding_payment_credits_balance_and_is_idempotent() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;
    let user = create_test_user(&pool, "fund").await;
    credit_balance(&pool, user.id, Decimal::from(1000)).await;

    let event = FundingPaymentEvent {
        event_id: fresh_event_id(),
        settlement_time: Utc::now(),
        market: MarketSymbol::Sol,
        user_id: user.id,
        position_qty: Decimal::from(10),
        funding_rate_hour: Decimal::new(1, 3), // 0.001
        amount: Decimal::from(50),
    };

    let applied_first = store::ledger::apply_funding_payment(&pool, &event).await.unwrap();
    assert!(applied_first, "a fresh event_id must apply");

    let balance_after_first: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance_after_first, Decimal::from(1050));

    // Redelivery of the exact same event (at-least-once semantics on the
    // Redis side) must not double-credit.
    let applied_second = store::ledger::apply_funding_payment(&pool, &event).await.unwrap();
    assert!(!applied_second, "replaying the same event_id must be a no-op");

    let balance_after_second: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance_after_second, Decimal::from(1050), "balance must not change on redelivery");
}

#[tokio::test]
async fn apply_funding_payment_handles_negative_amount_as_debit() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Eth, Decimal::from(2000)).await;
    let user = create_test_user(&pool, "fund_neg").await;
    credit_balance(&pool, user.id, Decimal::from(1000)).await;

    let event = FundingPaymentEvent {
        event_id: fresh_event_id(),
        settlement_time: Utc::now(),
        market: MarketSymbol::Eth,
        user_id: user.id,
        position_qty: Decimal::from(-5),
        funding_rate_hour: Decimal::new(1, 3),
        amount: Decimal::from(-30),
    };

    store::ledger::apply_funding_payment(&pool, &event).await.unwrap();

    let balance: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, Decimal::from(970));
}

#[tokio::test]
async fn apply_liquidation_transfer_zeroes_position_and_is_idempotent() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Btc, Decimal::from(60_000)).await;
    let user = create_test_user(&pool, "liq").await;
    credit_balance(&pool, user.id, Decimal::from(5000)).await;

    sqlx::query(
        "INSERT INTO positions (user_id, market, variant, margin_mode, leverage, quantity, average_price, allocated_margin)
         VALUES ($1, $2, 'LONG', 'ISOLATED', 10, 1, 60000, 6000)",
    )
    .bind(user.id)
    .bind(MarketSymbol::Btc)
    .execute(&pool)
    .await
    .unwrap();

    let event = LiquidationTransferEvent {
        event_id: fresh_event_id(),
        user_id: user.id,
        market: MarketSymbol::Btc,
        mark_price: Decimal::from(60_000),
    };

    let applied_first = store::ledger::apply_liquidation_transfer(&pool, &event).await.unwrap();
    assert!(applied_first);

    // The user's position is deleted outright (its notional is transferred
    // to the insurance fund's own position in the same market), not zeroed
    // in place - see apply_liquidation_transfer's DELETE + insurance-fund
    // upsert.
    let remaining: Option<Decimal> =
        sqlx::query_scalar("SELECT quantity FROM positions WHERE user_id = $1 AND market = $2")
            .bind(user.id)
            .bind(MarketSymbol::Btc)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, None, "liquidation transfer must delete the liquidated user's position");

    let applied_second = store::ledger::apply_liquidation_transfer(&pool, &event).await.unwrap();
    assert!(!applied_second, "replaying the same liquidation event_id must be a no-op");
}

#[tokio::test]
async fn insert_funding_sample_and_mean_premium_index_round_trip() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;

    let window_start = Utc::now() - chrono::Duration::minutes(1);
    store::ledger::insert_funding_sample(
        &pool,
        MarketSymbol::Sol,
        Decimal::from(100),
        Decimal::from(101),
        Decimal::from(99),
        Decimal::new(5, 3), // 0.005
    )
    .await
    .unwrap();

    let mean = store::ledger::mean_premium_index(&pool, MarketSymbol::Sol, window_start)
        .await
        .unwrap();
    assert!(mean.is_some(), "a sample inserted within the window must be picked up by the mean");
}
