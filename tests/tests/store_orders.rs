mod common;

use common::{create_test_user, credit_balance, seed_test_market};
use rust_decimal::Decimal;
use store::models::{MarginMode, MarketSymbol, OrderType, OrderVariant, TimeInForce};
use store::orders::{PlaceOrderError, PlaceOrderParams};

fn limit_params(price: Decimal, quantity: Decimal) -> PlaceOrderParams {
    PlaceOrderParams {
        market: MarketSymbol::Sol,
        variant: OrderVariant::Long,
        order_type: OrderType::Limit,
        tif: TimeInForce::Gtc,
        reduce_only: false,
        leverage: 5,
        margin_mode: MarginMode::Cross,
        price: Some(price),
        quantity,
    }
}

#[tokio::test]
async fn place_order_reserves_margin_and_locks_collateral() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;
    let user = create_test_user(&pool, "ord").await;
    credit_balance(&pool, user.id, Decimal::from(1000)).await;

    let order = store::orders::place_order(&pool, user.id, limit_params(Decimal::from(100), Decimal::from(10)))
        .await
        .expect("well-formed order within balance must be accepted");

    // reserved_margin = price * quantity / leverage = 100 * 10 / 5 = 200
    assert_eq!(order.reserved_margin, Decimal::from(200));

    let row: (Decimal, Decimal) =
        sqlx::query_as("SELECT collateral_available, collateral_locked FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, Decimal::from(800));
    assert_eq!(row.1, Decimal::from(200));
}

#[tokio::test]
async fn place_order_rejects_insufficient_balance() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;
    let user = create_test_user(&pool, "ord_insuf").await;
    credit_balance(&pool, user.id, Decimal::from(10)).await;

    let result = store::orders::place_order(&pool, user.id, limit_params(Decimal::from(100), Decimal::from(10))).await;
    assert!(matches!(result, Err(PlaceOrderError::InsufficientBalance)));
}

#[tokio::test]
async fn place_order_rejects_price_off_tick_and_quantity_off_lot() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;
    let user = create_test_user(&pool, "ord_tick").await;
    credit_balance(&pool, user.id, Decimal::from(10_000)).await;

    let bad_price = store::orders::place_order(&pool, user.id, limit_params(Decimal::new(1000005, 4), Decimal::from(1)))
        .await;
    assert!(matches!(bad_price, Err(PlaceOrderError::PriceNotOnTick)));

    let bad_qty = store::orders::place_order(&pool, user.id, limit_params(Decimal::from(100), Decimal::new(15, 3)))
        .await;
    assert!(matches!(bad_qty, Err(PlaceOrderError::QuantityNotOnLot)));
}

#[tokio::test]
async fn place_order_rejects_leverage_above_market_max() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;
    let user = create_test_user(&pool, "ord_lev").await;
    credit_balance(&pool, user.id, Decimal::from(10_000)).await;

    let mut params = limit_params(Decimal::from(100), Decimal::from(1));
    params.leverage = 100; // market max_leverage seeded as 20
    let result = store::orders::place_order(&pool, user.id, params).await;
    assert!(matches!(result, Err(PlaceOrderError::InvalidLeverage)));
}

#[tokio::test]
async fn request_cancel_refunds_reserved_margin() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;
    let user = create_test_user(&pool, "ord_cancel").await;
    credit_balance(&pool, user.id, Decimal::from(1000)).await;

    let order = store::orders::place_order(&pool, user.id, limit_params(Decimal::from(100), Decimal::from(10)))
        .await
        .unwrap();

    store::orders::request_cancel(&pool, user.id, order.id)
        .await
        .expect("cancelling a resting order must succeed");

    let row: (Decimal, Decimal) =
        sqlx::query_as("SELECT collateral_available, collateral_locked FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0, Decimal::from(1000), "cancelling must return the full reserved margin");
    assert_eq!(row.1, Decimal::ZERO);
}

#[tokio::test]
async fn fetch_open_orders_for_user_lists_placed_order() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Sol, Decimal::from(100)).await;
    let user = create_test_user(&pool, "ord_list").await;
    credit_balance(&pool, user.id, Decimal::from(1000)).await;

    let order = store::orders::place_order(&pool, user.id, limit_params(Decimal::from(100), Decimal::from(10)))
        .await
        .unwrap();

    let open = store::orders::fetch_open_orders_for_user(&pool, user.id).await.unwrap();
    assert!(open.iter().any(|o| o.id == order.id));
}
