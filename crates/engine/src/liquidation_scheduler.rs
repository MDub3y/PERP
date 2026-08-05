use crate::state::IntakeCommand;
use std::time::Duration;
use tokio::sync::mpsc;

/// Polling cadence for margin-health checks - a documented simplification
/// of "continuous" (recomputed synchronously on every fill/mark-price
/// tick). A few seconds is a reasonable balance between staying close to
/// real-time and not hammering Postgres with the cross/isolated equity
/// queries on every single event.
const CHECK_INTERVAL: Duration = Duration::from_secs(3);

pub async fn run_liquidation_scheduler(tx: mpsc::Sender<IntakeCommand>) {
    let mut ticker = tokio::time::interval(CHECK_INTERVAL);
    loop {
        ticker.tick().await;
        if tx.send(IntakeCommand::CheckLiquidations).await.is_err() {
            tracing::error!("liquidation-scheduler: match-loop channel closed");
            return;
        }
    }
}
