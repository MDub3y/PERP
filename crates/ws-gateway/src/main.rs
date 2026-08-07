mod market_ws;
mod pubsub;
mod user_ws;

use axum::{
    Router,
    routing::get,
};

#[derive(Clone)]
struct AppState {
    redis_client: redis::Client,
    jwt_secret: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let redis_url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    let redis_client = redis::Client::open(redis_url).expect("Invalid REDIS_URL");
    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

    let state = AppState {
        redis_client,
        jwt_secret,
    };

    let app = Router::new()
        .route("/ws/market/{symbol}", get(market_ws::market_ws_handler))
        .route("/ws/user", get(user_ws::user_ws_handler))
        .with_state(state);

    let server_addr = std::env::var("SERVER_ADDRESS").unwrap_or_else(|_| "127.0.0.1:3001".into());
    let listener = tokio::net::TcpListener::bind(&server_addr).await.unwrap();

    tracing::info!("WS gateway listening on ws://{}", server_addr);
    axum::serve(listener, app).await.unwrap();
}
