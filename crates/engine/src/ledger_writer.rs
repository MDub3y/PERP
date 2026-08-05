use redis::AsyncCommands;
use redis::streams::{StreamId, StreamReadOptions, StreamReadReply};
use sqlx::PgPool;
use std::time::Duration;
use store::ledger::{
    FillEvent, FundingPaymentEvent, LiquidationTransferEvent, OrderCancelledRemainderEvent, OrderRejectedEvent,
    OrderRestedEvent,
};

const IDLE_CLAIM_MS: usize = 30_000;
const GROUP_NAME: &str = "ledger-writers";

/// Consumes engine:events (an independent consumer group from any other
/// reader of the same stream, e.g. a future TimescaleDB-only consumer) and
/// applies each event to Postgres via store::ledger::apply_*. Every apply_*
/// call is already idempotent via the engine_event_cursor guard, so
/// at-least-once delivery here (XREADGROUP/XACK + XAUTOCLAIM, same shape as
/// worker::consumer) is sufficient on its own - no reconciler-equivalent is
/// needed the way withdrawals need one, since there's no external system
/// (like a blockchain) to poll here; every event already carries everything
/// required to apply it.
pub async fn run_ledger_writer(pool: PgPool, redis_client: redis::Client, stream_key: String, consumer_name: String) {
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("ledger-writer {consumer_name}: redis connect failed: {e}");
            return;
        }
    };

    let _: redis::RedisResult<()> = redis::cmd("XGROUP")
        .arg("CREATE")
        .arg(&stream_key)
        .arg(GROUP_NAME)
        .arg("0")
        .arg("MKSTREAM")
        .query_async(&mut conn)
        .await;

    loop {
        let opts = StreamReadOptions::default()
            .group(GROUP_NAME, &consumer_name)
            .block(5000)
            .count(50);

        let reply: StreamReadReply = match conn.xread_options(&[&stream_key], &[">"], &opts).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("ledger-writer {consumer_name}: XREADGROUP failed: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        for stream in &reply.keys {
            for entry in &stream.ids {
                apply_entry(&pool, entry).await;
                let _: redis::RedisResult<()> = conn.xack(&stream_key, GROUP_NAME, &[&entry.id]).await;
            }
        }

        let claimed: redis::RedisResult<redis::streams::StreamAutoClaimReply> = redis::cmd("XAUTOCLAIM")
            .arg(&stream_key)
            .arg(GROUP_NAME)
            .arg(&consumer_name)
            .arg(IDLE_CLAIM_MS)
            .arg("0-0")
            .query_async(&mut conn)
            .await;

        if let Ok(reply) = claimed {
            for entry in reply.claimed {
                apply_entry(&pool, &entry).await;
                let _: redis::RedisResult<()> = conn.xack(&stream_key, GROUP_NAME, &[&entry.id]).await;
            }
        }
    }
}

async fn apply_entry(pool: &PgPool, entry: &StreamId) {
    let Some(event_type): Option<String> = entry.get("event_type") else {
        tracing::error!("ledger-writer: entry {} missing event_type", entry.id);
        return;
    };
    let Some(payload): Option<String> = entry.get("payload") else {
        tracing::error!("ledger-writer: entry {} missing payload", entry.id);
        return;
    };

    let result = match event_type.as_str() {
        "FILL" => match serde_json::from_str::<FillEvent>(&payload) {
            Ok(event) => store::ledger::apply_fill(pool, &event).await.map(|_| ()),
            Err(e) => {
                tracing::error!("ledger-writer: bad FILL payload: {e}");
                return;
            }
        },
        "ORDER_RESTED" => match serde_json::from_str::<OrderRestedEvent>(&payload) {
            Ok(event) => store::ledger::apply_order_rested(pool, &event).await.map(|_| ()),
            Err(e) => {
                tracing::error!("ledger-writer: bad ORDER_RESTED payload: {e}");
                return;
            }
        },
        "ORDER_CANCELLED_REMAINDER" => match serde_json::from_str::<OrderCancelledRemainderEvent>(&payload) {
            Ok(event) => store::ledger::apply_order_cancelled_remainder(pool, &event).await.map(|_| ()),
            Err(e) => {
                tracing::error!("ledger-writer: bad ORDER_CANCELLED_REMAINDER payload: {e}");
                return;
            }
        },
        "FUNDING_SETTLED" => match serde_json::from_str::<FundingPaymentEvent>(&payload) {
            Ok(event) => store::ledger::apply_funding_payment(pool, &event).await.map(|_| ()),
            Err(e) => {
                tracing::error!("ledger-writer: bad FUNDING_SETTLED payload: {e}");
                return;
            }
        },
        "LIQUIDATION_TRANSFER" => match serde_json::from_str::<LiquidationTransferEvent>(&payload) {
            Ok(event) => store::ledger::apply_liquidation_transfer(pool, &event).await.map(|_| ()),
            Err(e) => {
                tracing::error!("ledger-writer: bad LIQUIDATION_TRANSFER payload: {e}");
                return;
            }
        },
        "ORDER_REJECTED" => match serde_json::from_str::<OrderRejectedEvent>(&payload) {
            Ok(event) => store::ledger::apply_order_rejected(pool, &event).await.map(|_| ()),
            Err(e) => {
                tracing::error!("ledger-writer: bad ORDER_REJECTED payload: {e}");
                return;
            }
        },
        other => {
            tracing::error!("ledger-writer: unknown event_type {other}");
            return;
        }
    };

    if let Err(e) = result {
        tracing::error!("ledger-writer: failed to apply {event_type}: {e}");
    }
}
