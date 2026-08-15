//! The one suite that needs a live `solana-test-validator` (see
//! docker-compose.yml). Ignored by default - opt in with
//! `cargo test -p perp-integration-tests -- --ignored` after
//! `docker compose up -d solana-test-validator`, or via
//! `scripts/run-tests.sh --with-validator`.
//!
//! Exercises the full custody loop end to end: airdrop -> deposit indexing
//! credits collateral_available -> sweeping moves the SOL into the fat
//! wallet -> a withdrawal request is processed back out. This is the
//! concrete verification that the README's "deposit crediting is
//! manual/future work" gap is actually closed.

mod common;

use common::{create_test_user, credit_balance};
use rust_decimal::Decimal;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::derivation_path::DerivationPath;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signer::Signer;
use solana_sdk::signer::keypair::keypair_from_seed_and_derivation_path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const SOLANA_RPC_URL_DEFAULT: &str = "http://127.0.0.1:8899";
const TEST_DEPOSIT_INDEX: u32 = 999; // high index, well clear of anything the seeder assigns first

fn mnemonic_path() -> PathBuf {
    // CARGO_MANIFEST_DIR for this crate is <repo>/tests - the mnemonic
    // lives at <repo>/keys/mnemonic.txt regardless of the invoking cwd.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../keys/mnemonic.txt")
}

fn derive_test_deposit_keypair() -> solana_sdk::signature::Keypair {
    let contents = std::fs::read_to_string(mnemonic_path()).expect("keys/mnemonic.txt must exist (see crates/seeder)");
    let mnemonic = bip39::Mnemonic::parse_normalized(contents.trim()).expect("mnemonic must parse");
    let seed = mnemonic.to_seed("");
    let path = DerivationPath::try_from(format!("{TEST_DEPOSIT_INDEX}'/0'").as_str()).unwrap();
    keypair_from_seed_and_derivation_path(&seed, Some(path)).unwrap()
}

async fn rpc_client() -> Arc<RpcClient> {
    let url = std::env::var("SOLANA_RPC_URL").unwrap_or_else(|_| SOLANA_RPC_URL_DEFAULT.into());
    Arc::new(RpcClient::new(url))
}

#[tokio::test]
#[ignore = "requires `docker compose up -d solana-test-validator` (see scripts/run-tests.sh --with-validator)"]
async fn deposit_then_sweep_then_withdraw_round_trip() {
    let pool = common::connect_test_pool().await;
    let rpc = rpc_client().await;

    let deposit_keypair = derive_test_deposit_keypair();
    let deposit_pubkey = deposit_keypair.pubkey();

    common::insert_free_deposit_address(&pool, &deposit_pubkey.to_string()).await;
    let user = create_test_user(&pool, "e2e").await;
    assert_eq!(
        user.pubkey.as_deref(),
        Some(deposit_pubkey.to_string().as_str()),
        "test relies on being the sole free address so signup claims exactly this one"
    );

    // 1. Airdrop SOL to the deposit address and wait for confirmation.
    let airdrop_lamports = 2_000_000_000; // 2 SOL
    let sig = rpc
        .request_airdrop(&deposit_pubkey, airdrop_lamports)
        .await
        .expect("airdrop request must succeed against a local test-validator");
    wait_for_confirmation(&rpc, &sig).await;

    // 2. Run the deposit indexer for long enough to complete one poll tick
    // and credit the deposit, then stop it.
    let indexer_handle = tokio::spawn(worker::deposit_indexer::run_deposit_indexer(pool.clone(), rpc.clone()));
    tokio::time::sleep(Duration::from_secs(15)).await;
    indexer_handle.abort();

    let balance: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(balance > Decimal::ZERO, "deposit indexer must have credited the airdropped SOL");

    // 3. Run the sweeper for one tick and confirm the deposit address's
    // on-chain balance drops back toward the rent-exempt minimum.
    let fat_wallet = worker::wallet::load_fat_wallet_keypair(&mnemonic_path()).expect("fat wallet must derive");
    let fat_wallet_pubkey = fat_wallet.pubkey();
    // SWEEP_INTERVAL_SECS=1 so the sweeper's first tick fires almost
    // immediately instead of waiting the 60s production default.
    unsafe {
        std::env::set_var("SWEEP_INTERVAL_SECS", "1");
    }
    let sweeper_handle = tokio::spawn(worker::sweeper::run_sweeper(
        pool.clone(),
        rpc.clone(),
        mnemonic_path(),
        fat_wallet_pubkey,
    ));
    tokio::time::sleep(Duration::from_secs(5)).await;
    sweeper_handle.abort();

    let rent_exempt_min = rpc.get_minimum_balance_for_rent_exemption(0).await.unwrap();
    let remaining = rpc.get_balance(&deposit_pubkey).await.unwrap();
    assert!(
        remaining <= rent_exempt_min + 1_000,
        "sweeper must have moved the deposit address's balance down to ~rent-exempt minimum, got {remaining}"
    );

    // 4. Request and process a withdrawal back out of the fat wallet, using
    // the already-credited ledger balance.
    credit_balance(&pool, user.id, Decimal::from(1)).await; // ensure comfortably above the withdrawal amount
    let destination = Pubkey::new_unique();
    let request = store::withdrawals::request_withdrawal(&pool, user.id, Decimal::new(1_000_000, 8), &destination.to_string(), 5)
        .await
        .expect("withdrawal request within balance must succeed");

    worker::processor::process_withdrawal(&pool, &rpc, &fat_wallet, request.id)
        .await
        .expect("processing a fresh QUEUED withdrawal must succeed");

    let status: String = sqlx::query_scalar("SELECT status::text FROM withdrawal_requests WHERE id = $1")
        .bind(request.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        matches!(status.as_str(), "SUBMITTING" | "SUBMITTED" | "CONFIRMED"),
        "withdrawal must have progressed past QUEUED, got {status}"
    );
}

async fn wait_for_confirmation(rpc: &RpcClient, sig: &solana_sdk::signature::Signature) {
    for _ in 0..30 {
        if let Ok(Some(status)) = rpc.get_signature_status_with_commitment(sig, CommitmentConfig::confirmed()).await {
            if status.is_ok() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("airdrop signature never confirmed within timeout");
}
