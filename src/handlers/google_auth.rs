use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use chrono::{Duration, Utc};
use serde::Deserialize;

use crate::services::db;
use crate::services::google_calendar::{self, SCOPES};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct StartParams {
    /// label for this calendar account, e.g. "personal" or "work"
    pub label: String,
    pub secret: String,
}

pub async fn start(
    State(state): State<Arc<AppState>>,
    Query(params): Query<StartParams>,
) -> Result<Redirect, StatusCode> {
    let Some(cfg) = state.google_oauth.as_ref() else {
        tracing::error!("Google OAuth not configured");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    if params.secret != cfg.setup_secret {
        return Err(StatusCode::FORBIDDEN);
    }
    if params.label.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Use the label as the OAuth state so the callback knows which account this is.
    let state_param = params.label;

    let auth_url = url::Url::parse_with_params(
        "https://accounts.google.com/o/oauth2/v2/auth",
        &[
            ("client_id", cfg.client_id.as_str()),
            ("redirect_uri", cfg.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", state_param.as_str()),
        ],
    )
    .map_err(|e| {
        tracing::error!("Failed to build auth URL: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Redirect::temporary(auth_url.as_str()))
}

#[derive(Deserialize)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
) -> Result<impl IntoResponse, StatusCode> {
    let Some(cfg) = state.google_oauth.as_ref() else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };

    if let Some(err) = params.error {
        tracing::warn!("Google OAuth error: {err}");
        return Ok((StatusCode::BAD_REQUEST, format!("OAuth error: {err}")));
    }

    let code = params.code.ok_or(StatusCode::BAD_REQUEST)?;
    let label = params.state.ok_or(StatusCode::BAD_REQUEST)?;

    let exchange = google_calendar::exchange_code(
        &cfg.client_id,
        &cfg.client_secret,
        &cfg.redirect_uri,
        &code,
    )
    .await
    .map_err(|e| {
        tracing::error!("Token exchange failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let refresh_token = exchange.refresh_token.ok_or_else(|| {
        tracing::error!("Google did not return a refresh token — try revoking app access and re-authorizing with prompt=consent");
        StatusCode::BAD_GATEWAY
    })?;

    let email = google_calendar::fetch_user_email(&exchange.access_token)
        .await
        .ok();

    let calendar_ids = google_calendar::list_calendar_ids(&exchange.access_token)
        .await
        .unwrap_or_default();

    let tokens = db::CalendarTokens {
        label: label.clone(),
        access_token: exchange.access_token,
        refresh_token,
        expires_at: Utc::now() + Duration::seconds(exchange.expires_in),
        scope: exchange.scope,
        account_email: email.clone(),
        calendar_ids,
        updated_at: Utc::now(),
    };

    db::upsert_calendar_tokens(&state.firestore_db, &tokens)
        .await
        .map_err(|e| {
            tracing::error!("Failed to save calendar tokens: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    tracing::info!("Saved calendar tokens for label={label} email={:?}", email);
    Ok((
        StatusCode::OK,
        format!(
            "Calendar '{label}' connected successfully ({}). You can close this window.",
            email.as_deref().unwrap_or("unknown")
        ),
    ))
}
