use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use jsonwebtoken::{EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

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
        .route("/signup", post(signup_handler))
        .route("/signin", post(signin_handler))
        .with_state(pool);

    let server_addr = std::env::var("SERVER_ADDRESS").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&server_addr).await.unwrap();

    println!("PERP Engine listening on http://{}", server_addr);
    axum::serve(listener, app).await.unwrap();
}

async fn signup_handler(
    State(pool): State<PgPool>,
    Json(payload): Json<AuthPayload>,
) -> Result<(StatusCode, Json<store::models::User>), (StatusCode, String)> {
    if payload.username.trim().is_empty() || payload.password.len() < 6 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid username or password length".into(),
        ));
    }

    match store::users::create_user_with_deposit_address(
        &pool,
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
    State(pool): State<PgPool>,
    Json(payload): Json<AuthPayload>,
) -> Result<Json<AuthResponse>, (StatusCode, String)> {
    let user = store::users::find_user_by_username(&pool, &payload.username)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or((StatusCode::UNAUTHORIZED, "Invalid credentials".into()))?;

    if !store::users::verify_password(&payload.password, &user.password_hash) {
        return Err((StatusCode::UNAUTHORIZED, "Invalid credentials".into()));
    }

    let claims = JwtClaims {
        sub: user.username.clone(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
    };

    let jwt_secret = std::env::var("JWT_SECRET").expect("JWT_SECRET must be set");

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(jwt_secret.as_ref()),
    )
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Token generation failed".into(),
        )
    })?;

    Ok(Json(AuthResponse { token, user }))
}

async fn health_check_handler(State(pool): State<PgPool>) -> &'static str {
    let check: Result<(i32,), _> = sqlx::query_as("SELECT 1").fetch_one(&pool).await;
    match check {
        Ok(_) => "OK",
        Err(_) => "DATABASE_UNHEALTHY",
    }
}
