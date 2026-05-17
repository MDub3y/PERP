use crate::models::User;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use sqlx::PgPool;

pub async fn create_user(
    pool: &PgPool,
    username: &str,
    password_plain: &str,
) -> Result<User, sqlx::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hashed_password = argon2
        .hash_password(password_plain.as_bytes(), &salt)
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?
        .to_string();

    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (username, password_hash)
        VALUES ($1, $2)
        RETURNING id, username, password_hash, collateral_available, collateral_locked",
    )
    .bind(username)
    .bind(hashed_password)
    .fetch_one(pool)
    .await?;

    Ok(user)
}

pub async fn find_user_by_username(
    pool: &PgPool,
    username: &str,
) -> Result<Option<User>, sqlx::Error> {
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, collateral_available, collateral_locked 
         FROM users WHERE username = $1",
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
