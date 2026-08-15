use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;
use sqlx::PgPool;
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_SWEEP_INTERVAL_SECS: u64 = 60;
const DEFAULT_SWEEP_MIN_LAMPORTS: u64 = 10_000_000;

/// Periodically moves any sweepable SOL sitting in active deposit
/// addresses into the fat wallet, leaving each address at its
/// rent-exempt minimum. Purely an on-chain custody operation - the
/// ledger was already credited by the deposit indexer, so a failed or
/// skipped sweep never risks user balances, only leaves funds parked at
/// the deposit address until the next tick.
pub async fn run_sweeper(
    pool: PgPool,
    rpc: Arc<RpcClient>,
    mnemonic_path: PathBuf,
    fat_wallet_pubkey: Pubkey,
) {
    let interval_secs: u64 = std::env::var("SWEEP_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SWEEP_INTERVAL_SECS);
    let min_lamports: u64 = std::env::var("SWEEP_MIN_LAMPORTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SWEEP_MIN_LAMPORTS);

    let keypairs = match build_deposit_keypair_map(&mnemonic_path) {
        Ok(map) => map,
        Err(e) => {
            tracing::error!("sweeper: failed to derive deposit address keypairs, exiting: {e}");
            return;
        }
    };

    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
    loop {
        ticker.tick().await;

        let addresses = match store::deposits::fetch_active_deposit_addresses(&pool).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("sweeper: failed to fetch active deposit addresses: {e}");
                continue;
            }
        };

        for (pubkey_str, _user_id, _last_signature) in addresses {
            if let Err(e) = sweep_address(
                &rpc,
                &keypairs,
                &pubkey_str,
                &fat_wallet_pubkey,
                min_lamports,
            )
            .await
            {
                tracing::warn!("sweeper: address {pubkey_str} failed: {e}");
            }
        }
    }
}

/// Derives all 1000 deposit-address keypairs once at startup, matching the
/// exact index range/derivation scheme the seeder used, so lookups here
/// always resolve to the pubkeys already stored in Postgres.
fn build_deposit_keypair_map(
    mnemonic_path: &std::path::Path,
) -> Result<HashMap<Pubkey, Keypair>, Box<dyn Error + Send + Sync>> {
    let contents = std::fs::read_to_string(mnemonic_path)?;
    let mnemonic = bip39::Mnemonic::parse_normalized(contents.trim())?;
    let seed = mnemonic.to_seed("");

    let mut map = HashMap::with_capacity(1000);
    for i in 0..1000 {
        let path_str = format!("{i}'/0'");
        let derivation_path = solana_sdk::derivation_path::DerivationPath::try_from(path_str.as_str())
            .map_err(|e| format!("invalid derivation path for index {i}: {e:?}"))?;
        let keypair =
            solana_sdk::signer::keypair::keypair_from_seed_and_derivation_path(&seed, Some(derivation_path))
                .map_err(|_| format!("failed to derive keypair at index {i}"))?;
        map.insert(keypair.pubkey(), keypair);
    }

    Ok(map)
}

async fn sweep_address(
    rpc: &RpcClient,
    keypairs: &HashMap<Pubkey, Keypair>,
    pubkey_str: &str,
    fat_wallet_pubkey: &Pubkey,
    min_lamports: u64,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let pubkey = Pubkey::from_str(pubkey_str)?;

    let balance = rpc.get_balance(&pubkey).await?;
    let rent_exempt_min = rpc.get_minimum_balance_for_rent_exemption(0).await?;
    let sweepable = balance.saturating_sub(rent_exempt_min);

    if sweepable < min_lamports {
        return Ok(());
    }

    let keypair = keypairs
        .get(&pubkey)
        .ok_or("no derived keypair matches this deposit address")?;

    let instruction = system_instruction::transfer(&pubkey, fat_wallet_pubkey, sweepable);
    let message = Message::new(&[instruction], Some(&pubkey));
    let blockhash = rpc.get_latest_blockhash().await?;
    let tx = Transaction::new(&[keypair], message, blockhash);

    match rpc.send_and_confirm_transaction(&tx).await {
        Ok(signature) => {
            tracing::info!(
                "sweeper: swept {sweepable} lamports from {pubkey_str} to fat wallet (sig {signature})"
            );
        }
        Err(e) => {
            tracing::warn!("sweeper: failed to sweep {pubkey_str}: {e}");
        }
    }

    Ok(())
}
