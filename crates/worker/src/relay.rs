use sqlx::PgPool;
use std::time::Duration;

/// Tails withdrawal_outbox and republishes each unprocessed row onto the
/// Redis stream. This is the only bridge between Postgres (source of truth)
/// and Redis (fast ack-based dispatch layer) - Redis never has to be
/// consulted to know what withdrawals exist.
///
/// At-least-once by construction: if this process crashes between XADD and
/// mark_outbox_processed, the same event gets republished on restart. That's
/// safe because downstream processing (processor::process_withdrawal) is
/// idempotent per persisted request status.
pub async fn run_relay(pool: PgPool, redis_client: redis::Client, stream_key: String) {
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("relay: failed to connect to redis: {e}");
            return;
        }
    };

    loop {
        match store::withdrawals::fetch_unprocessed_outbox_events(&pool, 50).await {
            Ok(events) if !events.is_empty() => {
                for event in events {
                    let payload =
                        serde_json::json!({ "withdrawal_request_id": event.withdrawal_request_id })
                            .to_string();

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

                    if let Err(e) = store::withdrawals::mark_outbox_processed(&pool, event.id).await {
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
