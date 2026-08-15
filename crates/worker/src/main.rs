use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::signer::Signer;
use sqlx::postgres::PgPoolOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use worker::{consumer, deposit_indexer, reconciler, relay, sweeper, wallet};

const STREAM_KEY: &str = "withdrawals:queue";
const GROUP_NAME: &str = "withdrawal-workers";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into());
    let solana_rpc_url =
        std::env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8899".into());
    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let concurrency: usize = std::env::var("WORKER_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    let mnemonic_path = PathBuf::from("keys/mnemonic.txt");
    let fat_wallet = Arc::new(
        wallet::load_fat_wallet_keypair(Path::new("keys/mnemonic.txt"))
            .expect("Failed to derive fat wallet keypair"),
    );
    let fat_wallet_pubkey = fat_wallet.pubkey();
    let rpc = Arc::new(RpcClient::new(solana_rpc_url));
    let redis_client = redis::Client::open(redis_url).expect("Invalid REDIS_URL");

    tracing::info!("worker starting, fat wallet pubkey = {}", fat_wallet_pubkey);

    let mut handles = Vec::new();

    handles.push(tokio::spawn(relay::run_relay(
        pool.clone(),
        redis_client.clone(),
        STREAM_KEY.into(),
    )));

    for i in 0..concurrency {
        handles.push(tokio::spawn(consumer::run_consumer(
            pool.clone(),
            rpc.clone(),
            fat_wallet.clone(),
            redis_client.clone(),
            STREAM_KEY.into(),
            GROUP_NAME.into(),
            format!("worker-{i}"),
        )));
    }

    handles.push(tokio::spawn(reconciler::run_reconciler(
        pool.clone(),
        rpc.clone(),
        fat_wallet.clone(),
    )));

    handles.push(tokio::spawn(deposit_indexer::run_deposit_indexer(
        pool.clone(),
        rpc.clone(),
    )));

    handles.push(tokio::spawn(sweeper::run_sweeper(
        pool.clone(),
        rpc.clone(),
        mnemonic_path.clone(),
        fat_wallet_pubkey,
    )));

    for handle in handles {
        let _ = handle.await;
    }
}
