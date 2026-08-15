mod common;

use common::{create_test_user, insert_free_deposit_address, unique_username};

#[tokio::test]
async fn signup_assigns_a_free_deposit_address() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "signup").await;
    assert!(user.pubkey.is_some(), "signup must assign a deposit address");
    assert_eq!(user.collateral_available, rust_decimal::Decimal::ZERO);
}

#[tokio::test]
async fn claimed_deposit_address_cannot_be_claimed_twice() {
    let pool = common::connect_test_pool().await;
    // This DB isn't reset between test runs or between concurrently-running
    // tests in this binary, and `FOR UPDATE SKIP LOCKED` claims *some* free
    // row, not necessarily the one this test just inserted (an older
    // never-claimed row from a prior run could be picked instead) - so the
    // only thing safe to assert on is whichever address the signup actually
    // returned, not the pubkey this test happened to insert.
    let pubkey = unique_username("exhaust_addr").chars().take(44).collect::<String>();
    insert_free_deposit_address(&pool, &pubkey).await;

    let username_a = unique_username("exhaust_a");
    let user_a = store::users::create_user_with_deposit_address(&pool, &username_a, "password123")
        .await
        .expect("first signup should succeed and claim a free address");
    let claimed_pubkey = user_a.pubkey.expect("signup must assign some deposit address");

    let assigned_user_id: Option<i32> =
        sqlx::query_scalar("SELECT user_id FROM deposit_addresses WHERE pubkey = $1")
            .bind(&claimed_pubkey)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        assigned_user_id,
        Some(user_a.id),
        "the address returned by signup must be recorded as assigned to that same user"
    );
}

#[tokio::test]
async fn signup_rejects_duplicate_username() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "dup").await;

    let pubkey2 = unique_username("dup_addr2").chars().take(44).collect::<String>();
    insert_free_deposit_address(&pool, &pubkey2).await;

    let result = store::users::create_user_with_deposit_address(&pool, &user.username, "password123").await;
    assert!(result.is_err(), "creating a second user with the same username must fail");
}

#[tokio::test]
async fn signin_verifies_password_and_rejects_wrong_one() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "signin").await;

    let fetched = store::users::find_user_by_username(&pool, &user.username)
        .await
        .unwrap()
        .expect("user must be findable by username");

    assert!(store::users::verify_password("test-password-123", &fetched.password_hash));
    assert!(!store::users::verify_password("wrong-password", &fetched.password_hash));
}

#[tokio::test]
async fn fetch_user_by_id_roundtrips() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "fetch").await;

    let fetched = store::users::fetch_user_by_id(&pool, user.id).await.unwrap();
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().username, user.username);

    let missing = store::users::fetch_user_by_id(&pool, -1).await.unwrap();
    assert!(missing.is_none());
}
