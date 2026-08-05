use redis::AsyncCommands;
use redis::streams::{StreamReadOptions, StreamReadReply};
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Keypair;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;

use crate::processor::process_withdrawal;

const IDLE_CLAIM_MS: usize = 30_000;

pub async fn run_consumer(
    pool: PgPool,
    rpc: Arc<RpcClient>,
    fat_wallet: Arc<Keypair>,
    redis_client: redis::Client,
    stream_key: String,
    group: String,
    consumer_name: String,
) {
    let mut conn = match redis_client.get_multiplexed_async_connection().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("consumer {consumer_name}: redis connect failed: {e}");
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

        let reply: StreamReadReply = match conn
            .xread_options(&[&stream_key], &[">"], &opts)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("consumer {consumer_name}: XREADGROUP failed: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        for stream in &reply.keys {
            for entry in &stream.ids {
                if let Some(id) = extract_request_id(entry) {
                    if let Err(e) = process_withdrawal(&pool, &rpc, &fat_wallet, id).await {
                        tracing::error!("consumer {consumer_name}: processing {id} failed: {e}");
                    }
                }
                let _: redis::RedisResult<()> = conn.xack(&stream_key, &group, &[&entry.id]).await;
            }
        }

        // Reclaim entries abandoned by crashed consumers.
        let claimed: redis::RedisResult<redis::streams::StreamAutoClaimReply> =
            redis::cmd("XAUTOCLAIM")
                .arg(&stream_key)
                .arg(&group)
                .arg(&consumer_name)
                .arg(IDLE_CLAIM_MS)
                .arg("0-0")
                .query_async(&mut conn)
                .await;

        if let Ok(reply) = claimed {
            for entry in reply.claimed {
                if let Some(id) = extract_request_id(&entry) {
                    if let Err(e) = process_withdrawal(&pool, &rpc, &fat_wallet, id).await {
                        tracing::error!("consumer {consumer_name}: reclaim-processing {id} failed: {e}");
                    }
                }
                let _: redis::RedisResult<()> = conn.xack(&stream_key, &group, &[&entry.id]).await;
            }
        }
    }
}

fn extract_request_id(entry: &redis::streams::StreamId) -> Option<i32> {
    let payload: String = entry.get("payload")?;
    let value: serde_json::Value = serde_json::from_str(&payload).ok()?;
    value.get("withdrawal_request_id")?.as_i64().map(|v| v as i32)
}
