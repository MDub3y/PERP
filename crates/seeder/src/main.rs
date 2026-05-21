use bip39::Mnemonic;
use solana_sdk::derivation_path::DerivationPath;
use solana_sdk::signer::{Signer, keypair::keypair_from_seed_and_derivation_path};
use sqlx::postgres::PgPoolOptions;
use std::fs;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into());

    let mnemonic_path = Path::new("keys/mnemonic.txt");
    let mnemonic = if mnemonic_path.exists() {
        println!("Loading existing master seed phrase from file system...");
        let contents = fs::read_to_string(mnemonic_path)?;
        Mnemonic::parse_normalized(contents.trim())?
    } else {
        println!("Generating a brand new 24-word secure master seed phrase...");
        let new_mnemonic = Mnemonic::generate(24)?;

        fs::write(mnemonic_path, new_mnemonic.to_string())?;
        println!("Master seed phrase successfully backed up inside keys/mnemonic.txt!");
        new_mnemonic
    };

    let seed = mnemonic.to_seed("");

    println!("Deriving 1,000 distinct Solana public keys...");
    let mut derived_pubkeys = Vec::with_capacity(1000); // Fixed typo here

    for i in 0..1000 {
        let path_str = format!("{}'/0'", i);
        let derivation_path = DerivationPath::try_from(path_str.as_str())
            .map_err(|e| format!("Invalid derivation path mapping: {:?}", e))?;

        let keypair = keypair_from_seed_and_derivation_path(&seed, Some(derivation_path))
            .map_err(|_| "Failed to derive keypair lineage")?;

        derived_pubkeys.push(keypair.pubkey().to_string());
    }

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    println!("Executing atomic batch population to database table...");

    let mut query_builder = sqlx::QueryBuilder::new("INSERT INTO deposit_addresses (pubkey) ");

    query_builder.push_values(derived_pubkeys, |mut b, pubkey| {
        b.push_bind(pubkey);
    });

    query_builder.push(" ON CONFLICT (pubkey) DO NOTHING");

    let result = query_builder.build().execute(&pool).await?;
    println!(
        "Successfully initialized {} new keys into the deposit pool!",
        result.rows_affected()
    );

    Ok(())
}
