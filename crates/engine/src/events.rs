use redis::AsyncCommands;
use serde::Serialize;

/// Serializes an engine event (FillEvent, FundingPaymentEvent,
/// LiquidationTransferEvent, ...) to JSON and XADDs it to `engine:events`.
/// This is the durable-history bridge the ledger writer (Phase 5) and the
/// TimescaleDB path consume from - Redis Pub/Sub (Phase 8) is a separate,
/// ephemeral fan-out of the same underlying facts, not a substitute for
/// this stream.
pub async fn publish_event<T: Serialize>(
    conn: &mut redis::aio::MultiplexedConnection,
    stream_key: &str,
    event_type: &str,
    event: &T,
) {
    let payload = match serde_json::to_string(event) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("events: failed to serialize {event_type}: {e}");
            return;
        }
    };

    let result: redis::RedisResult<String> = conn
        .xadd(stream_key, "*", &[("event_type", event_type), ("payload", &payload)])
        .await;

    if let Err(e) = result {
        tracing::error!("events: XADD failed for {event_type}: {e}");
    }
}
