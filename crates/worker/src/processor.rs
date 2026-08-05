use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::{
    hash::Hash,
    message::Message,
    nonce::state::{State as NonceState, Versions as NonceVersions},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_instruction,
    transaction::{Transaction, TransactionError},
};
use sqlx::PgPool;
use std::str::FromStr;
use store::models::{WithdrawalRequest, WithdrawalStatus};

const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

#[derive(Debug)]
pub enum ProcessError {
    Database(sqlx::Error),
    Rpc(String),
    NoncePoolExhausted,
    Decode(String),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(e) => write!(f, "database error: {e}"),
            Self::Rpc(e) => write!(f, "rpc error: {e}"),
            Self::NoncePoolExhausted => write!(f, "nonce account pool exhausted"),
            Self::Decode(e) => write!(f, "decode error: {e}"),
        }
    }
}
impl std::error::Error for ProcessError {}
impl From<sqlx::Error> for ProcessError {
    fn from(e: sqlx::Error) -> Self {
        Self::Database(e)
    }
}

/// The single state-machine step for a withdrawal request. Reused by both
/// the Redis consumer and the reconciliation sweep, so crash-recovery logic
/// only ever exists in one place.
pub async fn process_withdrawal(
    pool: &PgPool,
    rpc: &RpcClient,
    fat_wallet: &Keypair,
    request_id: i32,
) -> Result<(), ProcessError> {
    let mut tx = pool.begin().await?;
    let request = store::withdrawals::fetch_withdrawal_request_for_update(&mut tx, request_id).await?;

    match request.status {
        WithdrawalStatus::Confirmed | WithdrawalStatus::Refunded | WithdrawalStatus::Failed => {
            tx.rollback().await?;
            Ok(())
        }

        WithdrawalStatus::Queued => {
            let nonce_pubkey = match store::withdrawals::claim_free_nonce_account(&mut tx, request_id).await? {
                Some(p) => p,
                None => {
                    tx.rollback().await?;
                    return Err(ProcessError::NoncePoolExhausted);
                }
            };
            // Release the DB row lock before doing any network I/O.
            tx.commit().await?;

            let nonce_pubkey_parsed = Pubkey::from_str(&nonce_pubkey)
                .map_err(|e| ProcessError::Decode(e.to_string()))?;
            let nonce_hash = fetch_durable_nonce(rpc, &nonce_pubkey_parsed).await?;

            let destination = Pubkey::from_str(&request.destination_pubkey)
                .map_err(|e| ProcessError::Decode(e.to_string()))?;
            let lamports = decimal_sol_to_lamports(request.amount);

            let signed_tx = build_and_sign(
                fat_wallet,
                &nonce_pubkey_parsed,
                nonce_hash,
                &destination,
                lamports,
            );
            let signature = signed_tx.signatures[0].to_string();
            let signed_bytes =
                bincode::serialize(&signed_tx).map_err(|e| ProcessError::Decode(e.to_string()))?;

            // *** Persist the signature BEFORE broadcasting. *** A crash
            // between broadcast and queue-ack still leaves a durable,
            // reproducible record of exactly what was (or was about to be)
            // sent - see recover_in_flight for how redelivery uses it.
            let updated = store::withdrawals::mark_submitting(
                pool,
                request_id,
                &signature,
                &signed_bytes,
                &nonce_pubkey,
                &nonce_hash.to_string(),
            )
            .await?;

            if updated.is_none() {
                // Lost the race - another worker already advanced this request.
                return Ok(());
            }

            broadcast_best_effort(rpc, &signed_tx, request_id, pool).await;
            Ok(())
        }

        WithdrawalStatus::Submitting | WithdrawalStatus::Submitted => {
            tx.rollback().await?; // read-only from here; release lock before RPC calls
            recover_in_flight(pool, rpc, &request).await
        }
    }
}

/// Crash-recovery / redelivery / poll-to-finalization, all in one place.
async fn recover_in_flight(
    pool: &PgPool,
    rpc: &RpcClient,
    request: &WithdrawalRequest,
) -> Result<(), ProcessError> {
    let signature_str = request
        .signature
        .as_ref()
        .expect("SUBMITTING/SUBMITTED requests always have a signature");
    let signature = solana_sdk::signature::Signature::from_str(signature_str)
        .map_err(|e| ProcessError::Decode(e.to_string()))?;

    match get_signature_status_with_history(rpc, &signature).await? {
        Some(Ok(())) => {
            store::withdrawals::mark_confirmed(pool, request.id).await?;
        }
        Some(Err(chain_err)) => {
            store::withdrawals::fail_and_refund(pool, request.id, &chain_err.to_string()).await?;
        }
        None => {
            let nonce_pubkey_str = request
                .nonce_account
                .as_ref()
                .expect("SUBMITTING/SUBMITTED requests always have a nonce_account");
            let nonce_pubkey = Pubkey::from_str(nonce_pubkey_str)
                .map_err(|e| ProcessError::Decode(e.to_string()))?;
            let onchain_nonce = fetch_durable_nonce(rpc, &nonce_pubkey).await?;
            let recorded_nonce = request
                .nonce_hash
                .as_ref()
                .expect("SUBMITTING/SUBMITTED requests always have a nonce_hash");

            if onchain_nonce.to_string() == *recorded_nonce {
                // Nonce unchanged: our transaction never landed (RPC was
                // down, dropped, etc). Safe to rebroadcast the IDENTICAL
                // signed bytes - same signature, same nonce, no double-spend
                // possible.
                if let Some(bytes) = &request.signed_tx_bytes {
                    if let Ok(signed_tx) = bincode::deserialize::<Transaction>(bytes) {
                        broadcast_best_effort(rpc, &signed_tx, request.id, pool).await;
                    }
                }
            } else {
                // Nonce advanced but the signature is nowhere to be found,
                // even with tx-history search. Only our locked nonce account
                // could have advanced it, so our tx almost certainly
                // executed and funds already left the wallet. Do NOT
                // auto-refund - that risks double-paying the user. Leave the
                // request in place and alert for manual review.
                tracing::error!(
                    request_id = request.id,
                    signature = %signature_str,
                    "CRITICAL: nonce advanced but signature not found on-chain \u{2014} \
                     possible unconfirmed fund movement, manual review required"
                );
            }
        }
    }

    Ok(())
}

async fn fetch_durable_nonce(rpc: &RpcClient, nonce_pubkey: &Pubkey) -> Result<Hash, ProcessError> {
    let account = rpc
        .get_account(nonce_pubkey)
        .await
        .map_err(|e| ProcessError::Rpc(e.to_string()))?;

    let versions: NonceVersions =
        bincode::deserialize(&account.data).map_err(|e| ProcessError::Decode(e.to_string()))?;

    match versions.state() {
        NonceState::Initialized(data) => Ok(data.blockhash()),
        NonceState::Uninitialized => Err(ProcessError::Decode(
            "nonce account is uninitialized".into(),
        )),
    }
}

fn build_and_sign(
    fat_wallet: &Keypair,
    nonce_pubkey: &Pubkey,
    nonce_hash: Hash,
    destination: &Pubkey,
    lamports: u64,
) -> Transaction {
    let instructions = [
        system_instruction::advance_nonce_account(nonce_pubkey, &fat_wallet.pubkey()),
        system_instruction::transfer(&fat_wallet.pubkey(), destination, lamports),
    ];
    let message = Message::new_with_nonce(
        instructions[1..].to_vec(),
        Some(&fat_wallet.pubkey()),
        nonce_pubkey,
        &fat_wallet.pubkey(),
    );
    let mut tx = Transaction::new_unsigned(message);
    tx.sign(&[fat_wallet], nonce_hash);
    tx
}

async fn broadcast_best_effort(rpc: &RpcClient, signed_tx: &Transaction, request_id: i32, pool: &PgPool) {
    match rpc.send_transaction(signed_tx).await {
        Ok(_) => {
            if let Err(e) = store::withdrawals::mark_submitted(pool, request_id).await {
                tracing::error!("failed to mark request {request_id} submitted: {e}");
            }
        }
        Err(e) => {
            // Blockchain down/unreachable, or transient rejection - leave
            // the request in SUBMITTING. The reconciler/consumer redelivery
            // path will retry via recover_in_flight, which is safe to call
            // any number of times.
            tracing::warn!("broadcast failed for request {request_id}, will retry: {e}");
        }
    }
}

async fn get_signature_status_with_history(
    rpc: &RpcClient,
    signature: &solana_sdk::signature::Signature,
) -> Result<Option<Result<(), TransactionError>>, ProcessError> {
    rpc.get_signature_status_with_commitment_and_history(
        signature,
        solana_sdk::commitment_config::CommitmentConfig::finalized(),
        true,
    )
    .await
    .map_err(|e| ProcessError::Rpc(e.to_string()))
}

fn decimal_sol_to_lamports(amount: Decimal) -> u64 {
    (amount * Decimal::from(LAMPORTS_PER_SOL))
        .to_u64()
        .unwrap_or(0)
}
