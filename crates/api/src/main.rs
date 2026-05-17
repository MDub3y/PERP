use axum::{Router, extract::State, routing::get};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into());

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    let app = Router::new()
        .route("/health", get(health_check_handler))
        .with_state(pool);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health_check_handler(State(pool): State<PgPool>) -> &'static str {
    let check: Result<(i32,), _> = sqlx::query_as("SELECT 1").fetch_one(&pool).await;

    match check {
        Ok(_) => "OK",
        Err(_) => "DATABASE_UNHEALTHY",
    }
}
