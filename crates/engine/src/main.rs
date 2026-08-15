//! The matching engine: consumes validated orders (margin already reserved
//! in Postgres by store::orders::place_order), matches them in memory
//! against a price-time-priority book, and streams the resulting fills to
//! `engine:events` for the ledger writer and TimescaleDB history to
//! consume. See migrations/20260806000000_add_matching_engine.sql and
//! crates/store/src/{orders,ledger}.rs for the surrounding pipeline.
//!
//! Deliberate simplification vs. a maximally-optimized engine: prices and
//! quantities use rust_decimal::Decimal throughout (consistent with the
//! rest of this codebase, exact, no float drift) rather than a bespoke
//! i64-ticks fixed-point representation. Decimal is itself fixed-point
//! under the hood; if profiling ever shows this is a bottleneck, swapping
//! the book's key type is a contained change.

mod book;
mod events;
mod fee_scheduler;
mod funding_scheduler;
mod intake;
mod ledger_writer;
mod liquidation_scheduler;
mod matcher;
mod oracle;
mod publish;
mod relay;
mod state;

use tokio::sync::mpsc;

const ORDERS_STREAM_KEY: &str = "orders:queue";
const ORDERS_GROUP_NAME: &str = "engine-workers";
const EVENTS_STREAM_KEY: &str = "engine:events";
const INTAKE_CHANNEL_CAPACITY: usize = 1024;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into());
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let intake_concurrency: usize = std::env::var("ENGINE_INTAKE_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    let redis_client = redis::Client::open(redis_url).expect("Invalid REDIS_URL");

    tracing::info!("engine starting");

    let (tx, rx) = mpsc::channel(INTAKE_CHANNEL_CAPACITY);

    let mut handles = Vec::new();

    handles.push(tokio::spawn(relay::run_relay(
        pool.clone(),
        redis_client.clone(),
        ORDERS_STREAM_KEY.into(),
    )));

    for i in 0..intake_concurrency {
        handles.push(tokio::spawn(intake::run_intake_consumer(
            pool.clone(),
            redis_client.clone(),
            ORDERS_STREAM_KEY.into(),
            ORDERS_GROUP_NAME.into(),
            format!("engine-{i}"),
            tx.clone(),
        )));
    }
    handles.push(tokio::spawn(fee_scheduler::run_fee_tier_scheduler(pool.clone(), tx.clone())));
    handles.push(tokio::spawn(funding_scheduler::run_funding_sampler(tx.clone())));
    handles.push(tokio::spawn(funding_scheduler::run_funding_settlement_scheduler(tx.clone())));
    handles.push(tokio::spawn(liquidation_scheduler::run_liquidation_scheduler(tx.clone())));

    let oracle_markets = store::markets::fetch_all_markets(&pool)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("failed to load markets for oracle poller: {e}");
            Vec::new()
        })
        .into_iter()
        .filter_map(|m| m.pyth_price_feed_id.map(|feed_id| (m.market, feed_id)))
        .collect();
    handles.push(tokio::spawn(oracle::run_oracle_poller(tx.clone(), oracle_markets)));

    drop(tx); // match-loop's rx closes once every clone (intake + schedulers + oracle) drops

    handles.push(tokio::spawn(state::run_match_loop(
        pool.clone(),
        redis_client.clone(),
        EVENTS_STREAM_KEY.into(),
        rx,
    )));

    handles.push(tokio::spawn(ledger_writer::run_ledger_writer(
        pool.clone(),
        redis_client.clone(),
        EVENTS_STREAM_KEY.into(),
        "ledger-writer-0".into(),
    )));

    for handle in handles {
        let _ = handle.await;
    }
}
