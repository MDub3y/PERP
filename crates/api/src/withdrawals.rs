use axum::{Extension, Json, extract::State, http::StatusCode};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::str::FromStr;

use crate::AppState;
use crate::auth::AuthUser;

#[derive(Deserialize)]
pub struct WithdrawalRequestPayload {
    pub amount: Decimal,
    pub destination_pubkey: String,
}

pub async fn request_withdrawal_handler(
    State(state): State<AppState>,
    Extension(AuthUser(user_id)): Extension<AuthUser>,
    Json(payload): Json<WithdrawalRequestPayload>,
) -> Result<(StatusCode, Json<store::models::WithdrawalRequest>), (StatusCode, String)> {
    if payload.amount <= Decimal::ZERO {
        return Err((StatusCode::BAD_REQUEST, "Amount must be positive".into()));
    }

    solana_sdk::pubkey::Pubkey::from_str(&payload.destination_pubkey)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid destination pubkey".into()))?;

    match store::withdrawals::request_withdrawal(
        &state.pool,
        user_id,
        payload.amount,
        &payload.destination_pubkey,
        state.withdrawal_rate_limit_per_day,
    )
    .await
    {
        Ok(request) => Ok((StatusCode::ACCEPTED, Json(request))),
        Err(store::withdrawals::RequestWithdrawalError::InsufficientBalance) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "Insufficient balance".into(),
        )),
        Err(store::withdrawals::RequestWithdrawalError::RateLimitExceeded) => Err((
            StatusCode::TOO_MANY_REQUESTS,
            "Withdrawal rate limit exceeded".into(),
        )),
        Err(store::withdrawals::RequestWithdrawalError::Database(e)) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
    }
}

pub async fn list_withdrawals_handler(
    State(state): State<AppState>,
    Extension(AuthUser(user_id)): Extension<AuthUser>,
) -> Result<Json<Vec<store::models::WithdrawalRequest>>, (StatusCode, String)> {
    store::withdrawals::fetch_withdrawals_for_user(&state.pool, user_id)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
