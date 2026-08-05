use redis::AsyncCommands;
use redis::streams::{StreamReadOptions, StreamReadReply};
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::state::IntakeCommand;

const IDLE_CLAIM_MS: usize = 30_000;

/// XREADGROUP + XACK for normal delivery, XAUTOCLAIM on an idle timeout to
/// reclaim entries abandoned by a crashed consumer - identical shape to
/// worker::consumer. Every entry is resolved to a full order row (or a
/// cancel request) and forwarded to the single match-loop task over an
/// mpsc channel, since the match loop is the sole owner of book state.
pub async fn run_intake_consumer(
    pool: PgPool,
    redis_client: redis::Client,
    stream_key: String,
    group: String,
    consumer_name: String,
    tx: mpsc::Sender<IntakeCommand>,
) {
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("intake {consumer_name}: redis connect failed: {e}");
            return;
        }
    };

    let _: redis::RedisResult<()> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(&stream_key)
        .arg(&group)
        .arg("0")
        .arg("MKSTREAM")
        .query_async(&mut conn)
        .await; // BUSYGROUP (already exists) is fine, ignore

    loop {
        let opts = StreamReadOptions::default()
            .group(&group, &consumer_name)
            .block(5000)
            .count(10);

        let reply: StreamReadReply = match conn.xread_options(&[&stream_key], &[">"], &opts).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("intake {consumer_name}: XREADGROUP failed: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        for stream in &reply.keys {
            for entry in &stream.ids {
                if let Some(order_id) = extract_order_id(entry) {
                    resolve_and_forward(&pool, order_id, &tx).await;
                }
                let _: redis::RedisResult<()> = conn.xack(&stream_key, &group, &[&entry.id]).await;
            }
        }

        let claimed: redis::RedisResult<redis::streams::StreamAutoClaimReply> = redis::cmd("XAUTOCLAIM")
            .arg(&stream_key)
            .arg(&group)
            .arg(&consumer_name)
            .arg(IDLE_CLAIM_MS)
            .arg("0-0")
            .query_async(&mut conn)
            .await;

        if let Ok(reply) = claimed {
            for entry in reply.claimed {
                if let Some(order_id) = extract_order_id(&entry) {
                    resolve_and_forward(&pool, order_id, &tx).await;
                }
                let _: redis::RedisResult<()> = conn.xack(&stream_key, &group, &[&entry.id]).await;
            }
        }
    }
}

/// The outbox payload only ever carries an order id (mirrors the withdrawal
/// worker's pattern) - resolve it to the current row and decide whether
/// this is a fresh order to match or a cancel to drop from the book, based
/// on its Postgres status. A CANCELLED status here means
/// store::orders::request_cancel already ran, so this is a cancel signal;
/// anything else is treated as a (re-)match request.
async fn resolve_and_forward(pool: &PgPool, order_id: i64, tx: &mpsc::Sender<IntakeCommand>) {
    let order = match sqlx::query_as::<_, store::models::Order>("SELECT * FROM orders WHERE id = $1")
        .bind(order_id)
        .fetch_optional(pool)
        .await
    {
        Ok(Some(o)) => o,
        Ok(None) => return,
        Err(e) => {
            tracing::error!("intake: failed to load order {order_id}: {e}");
            return;
        }
    };

    let command = if order.status == store::models::OrderStatus::Cancelled {
        IntakeCommand::CancelOrder {
            order_id: order.id,
            market: order.market,
        }
    } else {
        IntakeCommand::NewOrder(order)
    };

    if tx.send(command).await.is_err() {
        tracing::error!("intake: match-loop channel closed, dropping order {order_id}");
    }
}

fn extract_order_id(entry: &redis::streams::StreamId) -> Option<i64> {
    let payload: String = entry.get("payload")?;
    let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
    value.get("order_id")?.as_i64()
}
