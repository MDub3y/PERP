use crate::state::IntakeCommand;
use chrono::{Timelike, Utc};
use std::time::Duration;
use tokio::sync::mpsc;

const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// Ticks every 5s and asks the match loop to sample the premium index.
/// Book state only exists there, so this task's only job is timing.
pub async fn run_funding_sampler(tx: mpsc::Sender<IntakeCommand>) {
    let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
    loop {
        ticker.tick().await;
        if tx.send(IntakeCommand::SampleFunding).await.is_err() {
            tracing::error!("funding-sampler: match-loop channel closed");
            return;
        }
    }
}

/// Fires once per UTC hour boundary (computed against actual wall-clock
/// hours, not a fixed ticker from process start) and asks the match loop to
/// settle funding for the window that just closed.
pub async fn run_funding_settlement_scheduler(tx: mpsc::Sender<IntakeCommand>) {
    loop {
        let now = Utc::now();
        let this_hour = now.date_naive().and_hms_opt(now.hour(), 0, 0).unwrap().and_utc();
        let next_boundary = this_hour + chrono::Duration::hours(1);

        let sleep_for = (next_boundary - now).to_std().unwrap_or(Duration::from_secs(3600));
        tokio::time::sleep(sleep_for).await;

        if tx.send(IntakeCommand::SettleFunding).await.is_err() {
            tracing::error!("funding-settlement-scheduler: match-loop channel closed");
            return;
        }
    }
}
