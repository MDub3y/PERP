use axum::{
    extract::{
        Query, State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::Deserialize;

use crate::AppState;
use crate::pubsub;

#[derive(Deserialize)]
struct JwtClaims {
    sub: String,
    #[allow(dead_code)]
    exp: usize,
}

#[derive(Deserialize)]
pub struct UserWsQuery {
    token: String,
}

pub async fn user_ws_handler(
    State(state): State<AppState>,
    Query(query): Query<UserWsQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let data = match decode::<JwtClaims>(
        &query.token,
        &DecodingKey::from_secret(state.jwt_secret.as_ref()),
        &Validation::default(),
    ) {
        Ok(d) => d,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid or expired token").into_response(),
    };

    let user_id: i32 = match data.claims.sub.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid token subject").into_response(),
    };

    ws.on_upgrade(move |socket| handle_socket(state, user_id, socket))
}

async fn handle_socket(state: AppState, user_id: i32, socket: WebSocket) {
    let channels = vec![format!("user:{user_id}:orders")];
    pubsub::relay(&state.redis_client, &channels, socket).await;
}
