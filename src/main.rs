use axum::{Router, extract::State, routing::get};
use dotenvy::dotenv;
use sqlx::PgPool;
use std::env;
use std::net::SocketAddr;

type RuntimeResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::main]
async fn main() -> RuntimeResult<()> {
    dotenv().ok();

    let db_url = env::var("DATABASE_URL")?;
    let server_addr: SocketAddr = env::var("SERVER_ADDRESS")?.parse()?;

    let pool = PgPool::connect(&db_url).await?;
    println!("✅ Database connection pool established");

    let app = Router::new()
        .route("/health", get(health_check_handler))
        .with_state(pool);

    let listener = tokio::net::TcpListener::bind(server_addr).await?;
    println!("🚀 Server spinning up on http://{}", server_addr);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check_handler(State(pool): State<PgPool>) -> &'static str {
    let check: Result<(i32,), _> = sqlx::query_as("SELECT 1").fetch_one(&pool).await;

    match check {
        Ok((1,)) => "OK",
        _ => "DATABASE_UNHEALTHY",
    }
}
