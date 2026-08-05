use axum::{
    Extension, Json,
    extract::{Path, State},
    http::StatusCode,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use store::models::{MarginMode, MarketSymbol, OrderType, OrderVariant, TimeInForce};
use store::orders::PlaceOrderParams;

use crate::AppState;
use crate::auth::AuthUser;

fn default_tif() -> TimeInForce {
    TimeInForce::Gtc
}

fn default_margin_mode() -> MarginMode {
    MarginMode::Cross
}

#[derive(Deserialize)]
pub struct PlaceOrderPayload {
    pub market: MarketSymbol,
    pub variant: OrderVariant,
    pub order_type: OrderType,
    #[serde(default = "default_tif")]
    pub tif: TimeInForce,
    #[serde(default)]
    pub reduce_only: bool,
    pub leverage: i16,
    #[serde(default = "default_margin_mode")]
    pub margin_mode: MarginMode,
    pub price: Option<Decimal>,
    pub quantity: Decimal,
}

pub async fn place_order_handler(
    State(state): State<AppState>,
    Extension(AuthUser(user_id)): Extension<AuthUser>,
    Json(payload): Json<PlaceOrderPayload>,
) -> Result<(StatusCode, Json<store::models::Order>), (StatusCode, String)> {
    if payload.quantity <= Decimal::ZERO {
        return Err((StatusCode::BAD_REQUEST, "Quantity must be positive".into()));
    }
    if let Some(price) = payload.price {
        if price <= Decimal::ZERO {
            return Err((StatusCode::BAD_REQUEST, "Price must be positive".into()));
        }
    }

    let params = PlaceOrderParams {
        market: payload.market,
        variant: payload.variant,
        order_type: payload.order_type,
        tif: payload.tif,
        reduce_only: payload.reduce_only,
        leverage: payload.leverage,
        margin_mode: payload.margin_mode,
        price: payload.price,
        quantity: payload.quantity,
    };

    match store::orders::place_order(&state.pool, user_id, params).await {
        Ok(order) => Ok((StatusCode::ACCEPTED, Json(order))),
        Err(store::orders::PlaceOrderError::InsufficientBalance) => {
            Err((StatusCode::UNPROCESSABLE_ENTITY, "Insufficient balance".into()))
        }
        Err(store::orders::PlaceOrderError::MarketPriceUnavailable) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Market has no mark price yet, try a LIMIT order or retry shortly".into(),
        )),
        Err(store::orders::PlaceOrderError::Database(e)) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
        Err(other) => Err((StatusCode::BAD_REQUEST, other.to_string())),
    }
}

pub async fn cancel_order_handler(
    State(state): State<AppState>,
    Extension(AuthUser(user_id)): Extension<AuthUser>,
    Path(order_id): Path<i64>,
) -> Result<Json<store::models::Order>, (StatusCode, String)> {
    match store::orders::request_cancel(&state.pool, user_id, order_id).await {
        Ok(order) => Ok(Json(order)),
        Err(store::orders::CancelError::NotFound) => Err((
            StatusCode::NOT_FOUND,
            "Order not found, not owned by caller, or already terminal".into(),
        )),
        Err(store::orders::CancelError::Database(e)) => {
            Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
        }
    }
}

pub async fn list_orders_handler(
    State(state): State<AppState>,
    Extension(AuthUser(user_id)): Extension<AuthUser>,
) -> Result<Json<Vec<store::models::Order>>, (StatusCode, String)> {
    store::orders::fetch_open_orders_for_user(&state.pool, user_id)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

pub async fn list_positions_handler(
    State(state): State<AppState>,
    Extension(AuthUser(user_id)): Extension<AuthUser>,
) -> Result<Json<Vec<store::models::Position>>, (StatusCode, String)> {
    store::orders::fetch_positions_for_user(&state.pool, user_id)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
