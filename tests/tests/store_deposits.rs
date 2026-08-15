mod common;

use common::{create_test_user, unique_username};
use rust_decimal::Decimal;

#[tokio::test]
async fn record_deposit_credits_balance_once() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "dep").await;
    let pubkey = user.pubkey.clone().unwrap();
    let signature = unique_username("sig");
    let amount = Decimal::new(150_000_000, 8); // 1.5

    let credited = store::deposits::record_deposit(&pool, user.id, &pubkey, &signature, amount)
        .await
        .unwrap();
    assert!(credited, "first call with a new signature must credit");

    let balance: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, amount);
}

#[tokio::test]
async fn record_deposit_is_idempotent_on_signature() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "dep_idem").await;
    let pubkey = user.pubkey.clone().unwrap();
    let signature = unique_username("sig_idem");
    let amount = Decimal::new(100_000_000, 8); // 1.0

    let first = store::deposits::record_deposit(&pool, user.id, &pubkey, &signature, amount)
        .await
        .unwrap();
    let second = store::deposits::record_deposit(&pool, user.id, &pubkey, &signature, amount)
        .await
        .unwrap();

    assert!(first, "first insert with this signature should credit");
    assert!(!second, "replaying the same signature must be a no-op");

    let balance: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    // Credited exactly once despite two calls - this is the idempotency
    // guarantee the whole deposit indexer relies on to survive retries.
    assert_eq!(balance, amount);
}

#[tokio::test]
async fn fetch_active_deposit_addresses_only_returns_assigned_active_rows() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "dep_active").await;
    let pubkey = user.pubkey.clone().unwrap();

    let addresses = store::deposits::fetch_active_deposit_addresses(&pool).await.unwrap();
    assert!(
        addresses.iter().any(|(pk, uid, _)| pk == &pubkey && *uid == user.id),
        "this user's freshly-assigned address must show up as active"
    );
}

#[tokio::test]
async fn update_last_signature_advances_the_cursor() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "dep_cursor").await;
    let pubkey = user.pubkey.clone().unwrap();
    let sig = unique_username("cursor_sig");

    store::deposits::update_last_signature(&pool, &pubkey, &sig).await.unwrap();

    let stored: Option<String> =
        sqlx::query_scalar("SELECT last_signature FROM deposit_addresses WHERE pubkey = $1")
            .bind(&pubkey)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored.as_deref(), Some(sig.as_str()));
}
