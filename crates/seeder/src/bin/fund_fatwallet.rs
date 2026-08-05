use bip39::Mnemonic;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::derivation_path::DerivationPath;
use solana_sdk::nonce::State as NonceState;
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::signer::keypair::keypair_from_seed_and_derivation_path;
use solana_sdk::system_instruction;
use solana_sdk::transaction::Transaction;
use sqlx::postgres::PgPoolOptions;
use std::fs;
use std::path::Path;

/// Must match crates/worker/src/wallet.rs::FAT_WALLET_ACCOUNT_INDEX exactly -
/// both derive from the same mnemonic and must land on the same keypair.
const FAT_WALLET_ACCOUNT_INDEX: u32 = 1_000_000;
const NONCE_POOL_SIZE: usize = 16;
const AIRDROP_LAMPORTS: u64 = 10 * 1_000_000_000; // 10 SOL, local validator only

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into());
    let solana_rpc_url =
        std::env::var("SOLANA_RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8899".into());

    let mnemonic_path = Path::new("keys/mnemonic.txt");
    let contents = fs::read_to_string(mnemonic_path)
        .map_err(|_| "keys/mnemonic.txt not found - run the deposit-address seeder first")?;
    let mnemonic = Mnemonic::parse_normalized(contents.trim())?;
    let seed = mnemonic.to_seed("");

    let path = DerivationPath::try_from(format!("{FAT_WALLET_ACCOUNT_INDEX}'/0'").as_str())
        .map_err(|e| format!("Invalid fat wallet derivation path: {:?}", e))?;
    let fat_wallet: Keypair = keypair_from_seed_and_derivation_path(&seed, Some(path))
        .map_err(|_| "Failed to derive fat wallet keypair")?;

    println!("Fat wallet pubkey: {}", fat_wallet.pubkey());

    let rpc = RpcClient::new(solana_rpc_url);

    let balance = rpc.get_balance(&fat_wallet.pubkey()).await.unwrap_or(0);
    if balance < AIRDROP_LAMPORTS / 2 {
        println!("Requesting local-validator airdrop for the fat wallet...");
        match rpc.request_airdrop(&fat_wallet.pubkey(), AIRDROP_LAMPORTS).await {
            Ok(sig) => {
                rpc.confirm_transaction(&sig).await.ok();
                // confirm_transaction only reports confirmation status; poll
                // the actual balance so we don't build the next transaction
                // against a fee payer the validator hasn't caught up on yet.
                for _ in 0..30 {
                    if rpc.get_balance(&fat_wallet.pubkey()).await.unwrap_or(0) > 0 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                println!("Airdrop confirmed.");
            }
            Err(e) => println!("Airdrop failed (fine outside local/devnet): {e}"),
        }
    }

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    let nonce_account_size = NonceState::size();
    let rent_exempt_lamports = rpc
        .get_minimum_balance_for_rent_exemption(nonce_account_size)
        .await?;

    println!("Creating {NONCE_POOL_SIZE} durable-nonce accounts owned by the fat wallet...");

    for i in 0..NONCE_POOL_SIZE {
        let nonce_keypair = Keypair::new();
        let instructions = system_instruction::create_nonce_account(
            &fat_wallet.pubkey(),
            &nonce_keypair.pubkey(),
            &fat_wallet.pubkey(), // nonce authority
            rent_exempt_lamports,
        );

        let blockhash = rpc.get_latest_blockhash().await?;
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&fat_wallet.pubkey()),
            &[&fat_wallet, &nonce_keypair],
            blockhash,
        );

        rpc.send_and_confirm_transaction(&tx).await?;

        sqlx::query(
            "INSERT INTO fee_payer_nonces (nonce_pubkey) VALUES ($1) ON CONFLICT (nonce_pubkey) DO NOTHING",
        )
        .bind(nonce_keypair.pubkey().to_string())
        .execute(&pool)
        .await?;

        println!("  [{}/{NONCE_POOL_SIZE}] {}", i + 1, nonce_keypair.pubkey());
    }

    println!("Fat wallet + nonce pool bootstrap complete.");
    Ok(())
}
