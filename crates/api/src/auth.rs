use axum::{
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{DecodingKey, Validation, decode};
use serde::Deserialize;

use crate::AppState;

#[derive(Deserialize)]
pub struct JwtClaims {
    pub sub: String,
    pub exp: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct AuthUser(pub i32);

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "Missing Authorization header".into()))?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or((StatusCode::UNAUTHORIZED, "Malformed Authorization header".into()))?;

    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(state.jwt_secret.as_ref()),
        &Validation::default(),
    )
    .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid or expired token".into()))?;

    let user_id: i32 = data
        .claims
        .sub
        .parse()
        .map_err(|_| (StatusCode::UNAUTHORIZED, "Invalid token subject".into()))?;

    req.extensions_mut().insert(AuthUser(user_id));
    Ok(next.run(req).await)
}
