use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signature::Keypair;
use sqlx::PgPool;
use std::sync::Arc;
use std::time::Duration;
use store::models::WithdrawalStatus;

use crate::processor::process_withdrawal;

const SWEEP_INTERVAL_SECS: u64 = 30;
const STUCK_AFTER_MINUTES: i64 = 2;

/// Final safety net, independent of Redis entirely: re-drives anything
/// that's been sitting in an in-flight status too long back through
/// process_withdrawal. Covers a lost/trimmed stream entry, a relay outage,
/// a deleted consumer group - anything that would otherwise strand a
/// request that Postgres still knows about.
pub async fn run_reconciler(pool: PgPool, rpc: Arc<RpcClient>, fat_wallet: Arc<Keypair>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(SWEEP_INTERVAL_SECS));
    loop {
        ticker.tick().await;

        let stuck = match store::withdrawals::fetch_stuck_requests(
            &pool,
            &[
                WithdrawalStatus::Queued,
                WithdrawalStatus::Submitting,
                WithdrawalStatus::Submitted,
            ],
            STUCK_AFTER_MINUTES,
        )
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("reconciler: failed to fetch stuck requests: {e}");
                continue;
            }
        };

        for request in stuck {
            if let Err(e) = process_withdrawal(&pool, &rpc, &fat_wallet, request.id).await {
                tracing::error!("reconciler: processing {} failed: {e}", request.id);
            }
        }
    }
}
