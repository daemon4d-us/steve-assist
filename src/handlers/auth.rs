use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde::Deserialize;

use crate::state::AppState;

#[derive(Deserialize)]
struct GoogleUserInfo {
    email: String,
    email_verified: Option<bool>,
}

pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Let CORS preflight through
    if request.method() == Method::OPTIONS {
        return Ok(next.run(request).await);
    }

    let header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Validate the access token against Google's userinfo endpoint
    let client = reqwest::Client::new();
    let user_info: GoogleUserInfo = client
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Failed to call Google userinfo: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .error_for_status()
        .map_err(|_| StatusCode::UNAUTHORIZED)?
        .json()
        .await
        .map_err(|e| {
            tracing::error!("Failed to parse Google userinfo: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if user_info.email_verified == Some(false) {
        tracing::warn!("Rejected unverified email: {}", user_info.email);
        return Err(StatusCode::FORBIDDEN);
    }

    let email = user_info.email.to_lowercase();

    if !state.allowed_emails.is_empty() && !state.allowed_emails.contains(&email) {
        tracing::warn!("Rejected unauthorized email: {email}");
        return Err(StatusCode::FORBIDDEN);
    }

    tracing::info!("Authenticated user: {email}");
    Ok(next.run(request).await)
}
