use sqlx::PgPool;
use std::time::Duration;

/// Tails orders_outbox and republishes each unprocessed row onto the
/// intake Redis stream. Identical role/shape to worker::relay for
/// withdrawals - Postgres stays the source of truth, this is just the
/// bridge to the fast dispatch layer.
pub async fn run_relay(pool: PgPool, redis_client: redis::Client, stream_key: String) {
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("relay: failed to connect to redis: {e}");
            return;
        }
    };

    loop {
        match store::orders::fetch_unprocessed_orders_outbox(&pool, 50).await {
            Ok(events) if !events.is_empty() => {
                for event in events {
                    let payload = serde_json::json!({ "order_id": event.order_id }).to_string();

                    let add_result: redis::RedisResult<String> = redis::cmd("XADD")
                        .arg(&stream_key)
                        .arg("*")
                        .arg("payload")
                        .arg(payload)
                        .query_async(&mut conn)
                        .await;

                    if let Err(e) = add_result {
                        tracing::error!("relay: XADD failed for outbox {}: {e}", event.id);
                        continue;
                    }

                    if let Err(e) = store::orders::mark_orders_outbox_processed(&pool, event.id).await {
                        tracing::error!("relay: failed to mark outbox {} processed: {e}", event.id);
                    }
                }
            }
            Ok(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            Err(e) => {
                tracing::error!("relay: failed to fetch outbox events: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}
