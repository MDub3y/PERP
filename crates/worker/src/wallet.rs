use bip39::Mnemonic;
use solana_sdk::derivation_path::DerivationPath;
use solana_sdk::signer::keypair::{Keypair, keypair_from_seed_and_derivation_path};
use std::fs;
use std::path::Path;

/// Index reserved for the fat wallet, well outside the seeder's 0..1000
/// deposit-address derivation range, so the two pools can never collide.
pub const FAT_WALLET_ACCOUNT_INDEX: u32 = 1_000_000;

/// Derives the fat-wallet keypair from the same master mnemonic the seeder
/// uses for deposit addresses. The resulting private key only ever lives in
/// this process's memory - it is never written to Postgres or Redis.
pub fn load_fat_wallet_keypair(
    mnemonic_path: &Path,
) -> Result<Keypair, Box<dyn std::error::Error + Send + Sync>> {
    let contents = fs::read_to_string(mnemonic_path)?;
    let mnemonic = Mnemonic::parse_normalized(contents.trim())?;
    let seed = mnemonic.to_seed("");

    let path_str = format!("{FAT_WALLET_ACCOUNT_INDEX}'/0'");
    let derivation_path = DerivationPath::try_from(path_str.as_str())
        .map_err(|e| format!("Invalid fat wallet derivation path: {:?}", e))?;

    keypair_from_seed_and_derivation_path(&seed, Some(derivation_path))
        .map_err(|_| "Failed to derive fat wallet keypair".into())
}
