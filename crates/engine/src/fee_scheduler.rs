use crate::state::IntakeCommand;
use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Once per UTC day (computed against actual midnight, not a fixed 24h
/// ticker from process start): recomputes trailing-30d volume -> tier via
/// store::ledger::refresh_fee_tiers, then reloads every account's tier and
/// hands the fresh map to the match-loop task. The match loop itself never
/// touches Postgres for this - it just swaps in whatever this task sends.
pub async fn run_fee_tier_scheduler(pool: PgPool, tx: mpsc::Sender<IntakeCommand>) {
    loop {
        let now = Utc::now();
        let next_midnight = (now.date_naive() + ChronoDuration::days(1))
            .and_hms_opt(0, 0, 0)
            .expect("00:00:00 is always valid")
            .and_utc();
        let sleep_for = (next_midnight - now)
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(86_400));

        tokio::time::sleep(sleep_for).await;

        if let Err(e) = store::ledger::refresh_fee_tiers(&pool).await {
            tracing::error!("fee-scheduler: refresh_fee_tiers failed: {e}");
            continue;
        }

        let rows: Vec<(i32, i16)> = match sqlx::query_as("SELECT id, fee_tier FROM users").fetch_all(&pool).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("fee-scheduler: failed to reload fee tiers: {e}");
                continue;
            }
        };

        let map: HashMap<i32, i16> = rows.into_iter().collect();
        if tx.send(IntakeCommand::ReloadFeeTiers(map)).await.is_err() {
            tracing::error!("fee-scheduler: match-loop channel closed");
            return;
        }
    }
}
