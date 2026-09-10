use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::services::{db, google_calendar};
use crate::state::AppState;

// --- Sessions API ---

#[derive(Serialize)]
pub struct SessionSummary {
    pub call_sid: String,
    pub caller_phone: String,
    pub caller_name: Option<String>,
    pub direction: String,
    pub started_at: DateTime<Utc>,
    pub duration_seconds: Option<i64>,
    pub summary: Option<String>,
}

pub async fn list_sessions(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SessionSummary>>, StatusCode> {
    let sessions = db::list_sessions(&state.firestore_db, 50)
        .await
        .map_err(|e| {
            tracing::error!("Failed to list sessions: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let summaries = sessions
        .into_iter()
        .map(|s| SessionSummary {
            call_sid: s.call_sid,
            caller_phone: s.caller_phone,
            caller_name: s.caller_name,
            direction: s.direction,
            started_at: s.started_at,
            duration_seconds: s.ended_at.map(|end| (end - s.started_at).num_seconds()),
            summary: s.summary,
        })
        .collect();

    Ok(Json(summaries))
}

// --- Profiles API ---

#[derive(Serialize)]
pub struct ProfileListItem {
    pub id: String,
    pub model: String,
    pub max_tokens: u32,
}

pub async fn list_profiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ProfileListItem>>, StatusCode> {
    let profiles = db::list_profiles(&state.firestore_db).await.map_err(|e| {
        tracing::error!("Failed to list profiles: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let items = profiles
        .into_iter()
        .map(|(id, p)| ProfileListItem {
            id,
            model: p.model,
            max_tokens: p.max_tokens,
        })
        .collect();

    Ok(Json(items))
}

pub async fn get_profile(
    State(state): State<Arc<AppState>>,
    Path(profile_id): Path<String>,
) -> Result<Json<db::AgentProfile>, StatusCode> {
    let profile = db::get_agent_profile(&state.firestore_db, &profile_id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get profile: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(profile))
}

pub async fn update_profile(
    State(state): State<Arc<AppState>>,
    Path(profile_id): Path<String>,
    Json(profile): Json<db::AgentProfile>,
) -> Result<StatusCode, StatusCode> {
    db::update_profile(&state.firestore_db, &profile_id, &profile)
        .await
        .map_err(|e| {
            tracing::error!("Failed to update profile: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

// --- Assignments API ---

#[derive(Deserialize)]
pub struct CreateAssignmentRequest {
    pub objective: String,
    pub profile_id: String,
    pub scheduled_at: DateTime<Utc>,
    pub contacts: Vec<CreateAssignmentContact>,
}

#[derive(Deserialize)]
pub struct CreateAssignmentContact {
    pub phone: String,
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct CreateAssignmentResponse {
    pub id: String,
}

pub async fn list_assignments(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<db::Assignment>>, StatusCode> {
    let assignments = db::list_assignments(&state.firestore_db, 50)
        .await
        .map_err(|e| {
            tracing::error!("Failed to list assignments: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(assignments))
}

pub async fn create_assignment(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateAssignmentRequest>,
) -> Result<(StatusCode, Json<CreateAssignmentResponse>), StatusCode> {
    let contacts = req
        .contacts
        .into_iter()
        .map(|c| db::AssignmentContact {
            phone: c.phone,
            name: c.name,
            status: "pending".to_string(),
            attempts: 0,
            last_attempt_at: None,
            call_sid: None,
            error: None,
        })
        .collect();

    let assignment = db::Assignment {
        id: String::new(),
        objective: req.objective,
        profile_id: req.profile_id,
        scheduled_at: req.scheduled_at,
        status: "pending".to_string(),
        created_at: Utc::now(),
        contacts,
    };

    let id = db::create_assignment(&state.firestore_db, &assignment)
        .await
        .map_err(|e| {
            tracing::error!("Failed to create assignment: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok((StatusCode::CREATED, Json(CreateAssignmentResponse { id })))
}

pub async fn get_assignment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<db::Assignment>, StatusCode> {
    let assignment = db::get_assignment(&state.firestore_db, &id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get assignment: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(assignment))
}

pub async fn update_assignment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(assignment): Json<db::Assignment>,
) -> Result<StatusCode, StatusCode> {
    db::update_assignment(&state.firestore_db, &id, &assignment)
        .await
        .map_err(|e| {
            tracing::error!("Failed to update assignment: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

pub async fn delete_assignment(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    db::delete_assignment(&state.firestore_db, &id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to delete assignment: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

// --- Bot call groups API ---

pub async fn list_bot_groups(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<db::BotCallGroup>>, StatusCode> {
    let groups = db::list_bot_groups(&state.firestore_db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to list bot groups: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(groups))
}

// --- Calendar API ---

#[derive(Serialize)]
pub struct CalendarStatus {
    pub label: String,
    pub account_email: Option<String>,
    pub valid: bool,
    pub error: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub calendar_count: usize,
}

#[derive(Serialize)]
pub struct CalendarConfigStatus {
    pub configured: bool,
    pub accounts: Vec<CalendarStatus>,
}

pub async fn list_calendar_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CalendarConfigStatus>, StatusCode> {
    let Some(cfg) = state.google_oauth.as_ref() else {
        return Ok(Json(CalendarConfigStatus {
            configured: false,
            accounts: Vec::new(),
        }));
    };

    let tokens = db::list_calendar_tokens(&state.firestore_db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to list calendar tokens: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let mut accounts = Vec::with_capacity(tokens.len());
    for t in tokens {
        let result = google_calendar::validate_refresh_token(
            &cfg.client_id,
            &cfg.client_secret,
            &t.refresh_token,
        )
        .await;
        let (valid, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(e)),
        };
        accounts.push(CalendarStatus {
            label: t.label,
            account_email: t.account_email,
            valid,
            error,
            expires_at: t.expires_at,
            calendar_count: t.calendar_ids.len(),
        });
    }

    Ok(Json(CalendarConfigStatus {
        configured: true,
        accounts,
    }))
}

#[derive(Deserialize)]
pub struct CalendarReauthRequest {
    pub label: String,
}

#[derive(Serialize)]
pub struct CalendarReauthResponse {
    pub url: String,
}

pub async fn calendar_reauth_url(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CalendarReauthRequest>,
) -> Result<Json<CalendarReauthResponse>, StatusCode> {
    let Some(cfg) = state.google_oauth.as_ref() else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    if req.label.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let url = url::Url::parse_with_params(
        "https://accounts.google.com/o/oauth2/v2/auth",
        &[
            ("client_id", cfg.client_id.as_str()),
            ("redirect_uri", cfg.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", google_calendar::SCOPES),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", req.label.as_str()),
        ],
    )
    .map_err(|e| {
        tracing::error!("Failed to build auth URL: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(CalendarReauthResponse { url: url.into() }))
}
