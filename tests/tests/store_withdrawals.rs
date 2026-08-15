mod common;

use common::{create_test_user, credit_balance};
use rust_decimal::Decimal;
use store::withdrawals::RequestWithdrawalError;

const DEST_PUBKEY: &str = "11111111111111111111111111111111111111111";

#[tokio::test]
async fn request_withdrawal_debits_balance_and_records_request() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "wd").await;
    credit_balance(&pool, user.id, Decimal::new(1000_00000000, 8)).await; // 1000.0

    let request = store::withdrawals::request_withdrawal(&pool, user.id, Decimal::from(100), DEST_PUBKEY, 5)
        .await
        .expect("withdrawal within balance and rate limit must succeed");
    assert_eq!(request.amount, Decimal::from(100));

    let balance: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, Decimal::from(900));
}

#[tokio::test]
async fn request_withdrawal_rejects_insufficient_balance() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "wd_insuf").await;
    credit_balance(&pool, user.id, Decimal::from(10)).await;

    let result = store::withdrawals::request_withdrawal(&pool, user.id, Decimal::from(100), DEST_PUBKEY, 5).await;
    assert!(matches!(result, Err(RequestWithdrawalError::InsufficientBalance)));

    // Balance must be untouched by the rejected attempt.
    let balance: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, Decimal::from(10));
}

#[tokio::test]
async fn request_withdrawal_enforces_rolling_rate_limit() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "wd_rate").await;
    credit_balance(&pool, user.id, Decimal::from(1000)).await;

    let limit = 3i64;
    for _ in 0..limit {
        store::withdrawals::request_withdrawal(&pool, user.id, Decimal::from(1), DEST_PUBKEY, limit)
            .await
            .expect("withdrawals within the limit must succeed");
    }

    let result = store::withdrawals::request_withdrawal(&pool, user.id, Decimal::from(1), DEST_PUBKEY, limit).await;
    assert!(
        matches!(result, Err(RequestWithdrawalError::RateLimitExceeded)),
        "the (limit+1)th withdrawal within the window must be rejected"
    );
}

#[tokio::test]
async fn fail_and_refund_restores_balance_and_marks_refunded() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "wd_refund").await;
    credit_balance(&pool, user.id, Decimal::from(500)).await;

    let request = store::withdrawals::request_withdrawal(&pool, user.id, Decimal::from(200), DEST_PUBKEY, 5)
        .await
        .unwrap();

    let balance_after_request: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance_after_request, Decimal::from(300));

    store::withdrawals::fail_and_refund(&pool, request.id, "test-induced failure")
        .await
        .expect("fail_and_refund should succeed on a QUEUED request");

    let balance_after_refund: Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance_after_refund, Decimal::from(500), "refund must restore the full debited amount");

    let status: String = sqlx::query_scalar("SELECT status::text FROM withdrawal_requests WHERE id = $1")
        .bind(request.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "REFUNDED");
}

#[tokio::test]
async fn fetch_withdrawals_for_user_returns_newest_first() {
    let pool = common::connect_test_pool().await;
    let user = create_test_user(&pool, "wd_list").await;
    credit_balance(&pool, user.id, Decimal::from(100)).await;

    let first = store::withdrawals::request_withdrawal(&pool, user.id, Decimal::from(1), DEST_PUBKEY, 5)
        .await
        .unwrap();
    let second = store::withdrawals::request_withdrawal(&pool, user.id, Decimal::from(1), DEST_PUBKEY, 5)
        .await
        .unwrap();

    let list = store::withdrawals::fetch_withdrawals_for_user(&pool, user.id).await.unwrap();
    assert!(list.len() >= 2);
    let ids: Vec<i32> = list.iter().map(|w| w.id).collect();
    assert!(ids.contains(&first.id));
    assert!(ids.contains(&second.id));
}
