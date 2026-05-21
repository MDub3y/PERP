use crate::models::User;
use argon2::{
    Argon2,
    password_hash::{
        self, PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
    },
};
use sqlx::{PgPool, Postgres, Transaction};

pub async fn create_user_with_deposit_address(
    pool: &PgPool,
    username: &str,
    password_plain: &str,
) -> Result<User, Box<dyn std::error::Error + Send + Sync>> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hashed_password = argon2
        .hash_password(password_plain.as_bytes(), &salt)
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
        .to_string();

    let mut tx: Transaction<'_, Postgres> = pool.begin().await?;

    let raw_user = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash)
        VALUES ($1, $2)
        RETURNING id, username, password_hash, collateral_available, collateral_locked",
    )
    .bind(username)
    .bind(hashed_password)
    .fetch_one(&mut *tx)
    .await?;

    let assigned_address = sqlx::query!(
        "UPDATE deposit_addresses
        SET user_id = $1, assigned_at = NOW()
        WHERE pubkey = (
            SELECT pubkey FROM deposit_addresses
            WHERE user_id IS NULL AND is_active = TRUE
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        )
        RETURNING pubkey",
        raw_user.id
    )
    .fetch_optional(&mut *tx)
    .await?;

    let address_record = match assigned_address {
        Some(record) => record,
        None => return Err("Deposit key pool exhausted! Generate more addresses.".into()),
    };

    tx.commit().await?;

    Ok(User {
        id: raw_user.id,
        username: raw_user.username,
        password_hash: raw_user.password_hash,
        collateral_available: raw_user.collateral_available,
        collateral_locked: raw_user.collateral_locked,
        pubkey: address_record.pubkey,
    })
}

pub async fn find_user_by_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<User>, sqlx::Error> {
    let user = sqlx::query_as::<_, User>(
        "SELECT
            u.id,
            u.username,
            u.password_hash,
            u.collateral_available,
            u.collateral_locked,
            d.pubkey
        FROM users u
        INNER JOIN DEPOSIT_ADDRESSES d ON u.id = d.user_id
        WHERE u.username = $1",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(user)
}

pub fn verify_password(password_plain: &str, password_hash: &str) -> bool {
    let parsed_hash = match PasswordHash::new(password_hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password_plain.as_bytes(), &parsed_hash)
        .is_ok()
}
