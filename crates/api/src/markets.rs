use axum::{Json, extract::State, http::StatusCode};

use crate::AppState;

pub async fn list_markets_handler(
    State(state): State<AppState>,
) -> Result<Json<Vec<store::models::Market>>, (StatusCode, String)> {
    store::markets::fetch_all_markets(&state.pool)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}
