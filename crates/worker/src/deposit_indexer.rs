use rust_decimal::Decimal;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_client::rpc_client::GetConfirmedSignaturesForAddress2Config;
use solana_client::rpc_config::RpcTransactionConfig;
use solana_sdk::commitment_config::CommitmentConfig;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use sqlx::PgPool;
use std::error::Error;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

const POLL_INTERVAL_SECS: u64 = 10;
const SIGNATURE_FETCH_LIMIT: usize = 50;

/// Polls every active deposit address for new incoming signatures and
/// credits users' collateral for confirmed SOL transfers. Runs forever;
/// any per-address failure (bad RPC response, unparsable pubkey, etc.) is
/// logged and skipped so one bad address never kills the whole loop.
pub async fn run_deposit_indexer(pool: PgPool, rpc: Arc<RpcClient>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(POLL_INTERVAL_SECS));
    loop {
        ticker.tick().await;

        let addresses = match store::deposits::fetch_active_deposit_addresses(&pool).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::error!("deposit_indexer: failed to fetch active deposit addresses: {e}");
                continue;
            }
        };

        for (pubkey_str, user_id, last_signature) in addresses {
            if let Err(e) =
                index_address(&pool, &rpc, &pubkey_str, user_id, last_signature.as_deref()).await
            {
                tracing::warn!("deposit_indexer: address {pubkey_str} failed: {e}");
            }
        }
    }
}

/// Fetches new signatures for a single deposit address (since its stored
/// cursor) and indexes each one, oldest-first, advancing the cursor after
/// each successfully-processed signature so a mid-batch crash doesn't lose
/// progress on the ones that already landed.
async fn index_address(
    pool: &PgPool,
    rpc: &RpcClient,
    pubkey_str: &str,
    user_id: i32,
    last_signature: Option<&str>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let pubkey = Pubkey::from_str(pubkey_str)?;
    let until = last_signature.and_then(|s| Signature::from_str(s).ok());

    let config = GetConfirmedSignaturesForAddress2Config {
        before: None,
        until,
        limit: Some(SIGNATURE_FETCH_LIMIT),
        commitment: None,
    };

    let mut statuses = rpc
        .get_signatures_for_address_with_config(&pubkey, config)
        .await?;
    // RPC returns newest-first; we want to credit deposits in the order
    // they happened.
    statuses.reverse();

    for status in statuses {
        let signature_str = status.signature.clone();

        if let Err(e) =
            index_signature(pool, rpc, pubkey_str, user_id, &signature_str).await
        {
            tracing::warn!(
                "deposit_indexer: failed processing signature {signature_str} for {pubkey_str}: {e}"
            );
            continue;
        }

        if let Err(e) = store::deposits::update_last_signature(pool, pubkey_str, &signature_str).await
        {
            tracing::warn!(
                "deposit_indexer: failed to advance cursor for {pubkey_str} to {signature_str}: {e}"
            );
        }
    }

    Ok(())
}

/// Fetches a single transaction and, if it credited native SOL lamports to
/// `pubkey_str`, records it as a deposit. Any transfer type that moves
/// lamports into the account is captured this way (not just
/// `system_instruction::transfer`), by diffing the account's pre/post
/// balances rather than parsing instruction data.
async fn index_signature(
    pool: &PgPool,
    rpc: &RpcClient,
    pubkey_str: &str,
    user_id: i32,
    signature_str: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let signature = Signature::from_str(signature_str)?;

    let config = RpcTransactionConfig {
        encoding: None,
        commitment: Some(CommitmentConfig::confirmed()),
        max_supported_transaction_version: Some(0),
    };

    let tx = rpc.get_transaction_with_config(&signature, config).await?;

    // Re-serialize to a generic JSON value rather than naming the
    // transaction-status types directly - avoids pulling in
    // solana-transaction-status-client-types as an extra dependency just
    // to read three fields out of the response.
    let value = serde_json::to_value(&tx)?;

    let meta = &value["meta"];
    if meta.is_null() {
        tracing::debug!("deposit_indexer: {signature_str} has no meta, skipping");
        return Ok(());
    }

    if !meta["err"].is_null() {
        tracing::debug!("deposit_indexer: {signature_str} errored on-chain, skipping");
        return Ok(());
    }

    let account_keys = value["transaction"]["message"]["accountKeys"]
        .as_array()
        .ok_or("missing accountKeys in transaction message")?;

    let account_index = account_keys
        .iter()
        .position(|k| k.as_str() == Some(pubkey_str));

    let account_index = match account_index {
        Some(i) => i,
        None => {
            tracing::debug!(
                "deposit_indexer: {pubkey_str} not present in account keys of {signature_str}"
            );
            return Ok(());
        }
    };

    let pre_balances = meta["preBalances"]
        .as_array()
        .ok_or("missing preBalances in transaction meta")?;
    let post_balances = meta["postBalances"]
        .as_array()
        .ok_or("missing postBalances in transaction meta")?;

    let pre: i64 = pre_balances
        .get(account_index)
        .and_then(|v| v.as_i64())
        .ok_or("preBalances missing entry for account index")?;
    let post: i64 = post_balances
        .get(account_index)
        .and_then(|v| v.as_i64())
        .ok_or("postBalances missing entry for account index")?;

    let delta_lamports = post - pre;
    if delta_lamports <= 0 {
        tracing::debug!(
            "deposit_indexer: {signature_str} on {pubkey_str} was not a net-incoming transfer (delta={delta_lamports})"
        );
        return Ok(());
    }

    let amount = Decimal::new(delta_lamports, 9);

    let credited =
        store::deposits::record_deposit(pool, user_id, pubkey_str, signature_str, amount).await?;

    if credited {
        tracing::info!(
            "deposit_indexer: credited user {user_id} {amount} SOL from {pubkey_str} (sig {signature_str})"
        );
    }

    Ok(())
}
