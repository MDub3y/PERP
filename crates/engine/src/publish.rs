use redis::AsyncCommands;
use rust_decimal::Decimal;
use serde_json::json;
use store::models::{MarketSymbol, OrderVariant};

/// Redis Pub/Sub (PUBLISH, not Streams) - fire-and-forget is correct here,
/// since these are live/ephemeral feeds. Durable history is the separate
/// engine:events -> ledger-writer/TimescaleDB path (events.rs). No
/// subscriber exists yet (a WebSocket gateway bridging these channels to
/// browser clients is explicitly out of scope for this pass) - these calls
/// just need to exist and be correct so that gateway is a thin addition
/// later.
pub fn market_str(market: MarketSymbol) -> &'static str {
    match market {
        MarketSymbol::Sol => "SOL",
        MarketSymbol::Eth => "ETH",
        MarketSymbol::Btc => "BTC",
    }
}

pub async fn publish_trade(
    conn: &mut redis::aio::MultiplexedConnection,
    market: MarketSymbol,
    price: Decimal,
    quantity: Decimal,
    taker_variant: OrderVariant,
    is_liquidation: bool,
) {
    let channel = format!("market:{}:trades", market_str(market));
    let payload = json!({
        "price": price,
        "quantity": quantity,
        "taker_variant": taker_variant,
        "is_liquidation": is_liquidation,
    })
    .to_string();
    let _: redis::RedisResult<i64> = conn.publish(channel, payload).await;
}

pub async fn publish_ticker(conn: &mut redis::aio::MultiplexedConnection, market: MarketSymbol, mark_price: Decimal) {
    let channel = format!("market:{}:ticker", market_str(market));
    let payload = json!({ "mark_price": mark_price }).to_string();
    let _: redis::RedisResult<i64> = conn.publish(channel, payload).await;
}

pub async fn publish_book_top(
    conn: &mut redis::aio::MultiplexedConnection,
    market: MarketSymbol,
    best_bid: Option<Decimal>,
    best_ask: Option<Decimal>,
) {
    let channel = format!("market:{}:book", market_str(market));
    let payload = json!({ "best_bid": best_bid, "best_ask": best_ask }).to_string();
    let _: redis::RedisResult<i64> = conn.publish(channel, payload).await;
}

/// Private per-user order-lifecycle channel - this is the concrete
/// implementation of "reuse the market-data pub/sub infra for a per-user
/// channel instead of making the client reload" already agreed on for the
/// order-confirmation UX.
pub async fn publish_user_order_event(
    conn: &mut redis::aio::MultiplexedConnection,
    user_id: i32,
    event_type: &str,
    order_id: i64,
    extra: serde_json::Value,
) {
    let channel = format!("user:{user_id}:orders");
    let mut payload = json!({ "event_type": event_type, "order_id": order_id });
    if let (Some(obj), Some(extra_obj)) = (payload.as_object_mut(), extra.as_object()) {
        for (k, v) in extra_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    let _: redis::RedisResult<i64> = conn.publish(channel, payload.to_string()).await;
}
