use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5432/perp_exchange".into());

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("Failed to connect to database");

    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");
    let withdrawal_rate_limit_per_day = std::env::var("WITHDRAWAL_RATE_LIMIT_PER_DAY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);

    let app = api::build_app(pool, jwt_secret, withdrawal_rate_limit_per_day);

    let server_addr = std::env::var("SERVER_ADDRESS").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&server_addr).await.unwrap();

    println!("PERP Engine listening on http://{}", server_addr);
    axum::serve(listener, app).await.unwrap();
}
