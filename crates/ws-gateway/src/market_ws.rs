use axum::{
    extract::{
        Path, State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::AppState;
use crate::pubsub;

fn is_known_market(symbol: &str) -> bool {
    matches!(symbol, "SOL" | "ETH" | "BTC")
}

pub async fn market_ws_handler(
    State(state): State<AppState>,
    Path(symbol): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let symbol = symbol.to_uppercase();
    if !is_known_market(&symbol) {
        return (StatusCode::NOT_FOUND, "unknown market").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(state, symbol, socket))
}

async fn handle_socket(state: AppState, symbol: String, socket: WebSocket) {
    let channels = vec![
        format!("market:{symbol}:trades"),
        format!("market:{symbol}:book"),
        format!("market:{symbol}:ticker"),
    ];
    pubsub::relay(&state.redis_client, &channels, socket).await;
}
