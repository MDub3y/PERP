mod common;

use common::seed_test_market;
use rust_decimal::Decimal;
use store::models::MarketSymbol;

#[tokio::test]
async fn fetch_all_markets_includes_seeded_market_with_mark_price() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Eth, Decimal::from(3000)).await;

    let markets = store::markets::fetch_all_markets(&pool).await.unwrap();
    let eth = markets.iter().find(|m| m.market == MarketSymbol::Eth).expect("ETH must be present");
    assert_eq!(eth.mark_price, Some(Decimal::from(3000)));
    assert!(eth.is_active);
}

#[tokio::test]
async fn update_mark_price_persists_new_price() {
    let pool = common::connect_test_pool().await;
    seed_test_market(&pool, MarketSymbol::Btc, Decimal::from(60_000)).await;

    store::orders::update_mark_price(&pool, MarketSymbol::Btc, Decimal::from(61_000))
        .await
        .unwrap();

    let market = store::orders::fetch_market(&pool, MarketSymbol::Btc).await.unwrap().unwrap();
    assert_eq!(market.mark_price, Some(Decimal::from(61_000)));
}
