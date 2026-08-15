mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::unique_username;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn test_app(pool: sqlx::PgPool) -> axum::Router {
    api::build_app(pool, "test-jwt-secret".into(), 5)
}

#[tokio::test]
async fn health_check_returns_ok() {
    let pool = common::connect_test_pool().await;
    let app = test_app(pool);

    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn signup_then_signin_then_me_round_trip() {
    let pool = common::connect_test_pool().await;
    let pubkey = unique_username("api_addr").chars().take(44).collect::<String>();
    common::insert_free_deposit_address(&pool, &pubkey).await;
    let username = unique_username("api_user");

    let app = test_app(pool.clone());
    let signup_body = json!({ "username": username, "password": "password123" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(signup_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let app = test_app(pool.clone());
    let signin_body = json!({ "username": username, "password": "password123" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signin")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(signin_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let signin_json = body_json(response).await;
    let token = signin_json["token"].as_str().expect("signin must return a token").to_string();

    let app = test_app(pool.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let me = body_json(response).await;
    assert_eq!(me["username"], username);
}

#[tokio::test]
async fn signup_rejects_short_password() {
    let pool = common::connect_test_pool().await;
    let app = test_app(pool);

    let body = json!({ "username": unique_username("shortpw"), "password": "abc" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn protected_route_without_token_is_unauthorized() {
    let pool = common::connect_test_pool().await;
    let app = test_app(pool);

    let response = app
        .oneshot(Request::builder().uri("/me").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn markets_endpoint_is_public_and_returns_json_array() {
    let pool = common::connect_test_pool().await;
    let app = test_app(pool);

    let response = app
        .oneshot(Request::builder().uri("/markets").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let markets = body_json(response).await;
    assert!(markets.is_array());
}

#[tokio::test]
async fn withdrawal_request_through_http_debits_balance() {
    let pool = common::connect_test_pool().await;
    let user = common::create_test_user(&pool, "api_wd").await;
    common::credit_balance(&pool, user.id, rust_decimal::Decimal::from(500)).await;

    let claims = serde_json::json!({
        "sub": user.id.to_string(),
        "exp": (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize,
    });
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(b"test-jwt-secret"),
    )
    .unwrap();

    let app = test_app(pool.clone());
    // The Solana System Program ID - a real, always-valid 32-byte base58
    // pubkey, since the handler validates `destination_pubkey` by parsing it.
    let body = json!({ "amount": 100, "destination_pubkey": "11111111111111111111111111111111" });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/withdrawals")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "withdrawal request within balance must succeed"
    );

    let balance: rust_decimal::Decimal = sqlx::query_scalar("SELECT collateral_available FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, rust_decimal::Decimal::from(400));
}
