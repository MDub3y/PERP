mod auth;
mod markets;
mod orders;
mod withdrawals;

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    middleware,
    routing::{delete, get, post},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use auth::AuthUser;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub jwt_secret: String,
    pub withdrawal_rate_limit_per_day: i64,
}

#[derive(Deserialize)]
struct AuthPayload {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct AuthResponse {
    token: String,
    user: store::models::User,
}

#[derive(Serialize)]
struct JwtClaims {
    sub: String,
    exp: usize,
}

pub fn build_app(pool: PgPool, jwt_secret: String, withdrawal_rate_limit_per_day: i64) -> Router {
    let state = AppState {
        pool,
        jwt_secret,
        withdrawal_rate_limit_per_day,
    };

    let protected = Router::new()
        .route("/me", get(me_handler))
        .route(
            "/withdrawals",
            post(withdrawals::request_withdrawal_handler),
        )
        .route("/withdrawals", get(withdrawals::list_withdrawals_handler))
        .route("/orders", post(orders::place_order_handler))
        .route("/orders", get(orders::list_orders_handler))
        .route("/orders/{id}", delete(orders::cancel_order_handler))
        .route("/positions", get(orders::list_positions_handler))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    Router::new()
        .route("/health", get(health_check_handler))
        .route("/signup", post(signup_handler))
        .route("/signin", post(signin_handler))
        .route("/markets", get(markets::list_markets_handler))
        .merge(protected)
        .with_state(state)
}

async fn signup_handler(
    State(state): State<AppState>,
    Json(payload): Json<AuthPayload>,
) -> Result<(StatusCode, Json<store::models::User>), (StatusCode, String)> {
    if payload.username.trim().is_empty() || payload.password.len() < 6 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid username or password length".into(),
        ));
    }

    match store::users::create_user_with_deposit_address(
        &state.pool,
        &payload.username,
        &payload.password,
    )
    .await
    {
        Ok(user) => Ok((StatusCode::CREATED, Json(user))),
        Err(err) => {
            if let Some(sqlx_err) = err.downcast_ref::<sqlx::Error>() {
                if let sqlx::Error::Database(db_err) = sqlx_err {
                    if db_err.is_unique_violation() {
                        return Err((StatusCode::CONFLICT, "Username already exists".into()));
                    }
                }
            }
            Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
        }
    }
}

async fn signin_handler(
    State(state): State<AppState>,
    Json(payload): Json<AuthPayload>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    let user = store::users::find_user_by_username(&state.pool, &payload.username)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::UNAUTHORIZED, "Invalid credentials".into()))?;

    if !store::users::verify_password(&payload.password, &user.password_hash) {
        return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".into()));
    }

    let claims = JwtClaims {
        sub: user.id.to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_ref()),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Token generation failed".into(),
        )
    })?;

    Ok(Json(AuthResponse { token, user }))
}

async fn me_handler(
    State(state): State<AppState>,
    Extension(AuthUser(user_id)): Extension<AuthUser>,
) -> Result<Json<store::models::User>, (StatusCode, String)> {
    store::users::fetch_user_by_id(&state.pool, user_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "User not found".into()))
}

async fn health_check_handler(State(state): State<AppState>) -> &'static str {
    let check: Result<(i32,), _> = sqlx::query_as("SELECT 1").fetch_one(&state.pool).await;
    match check {
        Ok(_) => "OK",
        Err(_) => "DATABASE_UNHEALTHY",
    }
}
