use std::collections::HashMap;
use std::sync::Arc;

use axum::Form;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};

use crate::services::db;
use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct OutboundCallParams {
    pub assignment_id: String,
    pub contact_index: usize,
}

/// TwiML handler called by Twilio when an outbound call connects.
pub async fn outbound_call(
    State(state): State<Arc<AppState>>,
    Query(params): Query<OutboundCallParams>,
) -> Result<impl IntoResponse, StatusCode> {
    let assignment = db::get_assignment(&state.firestore_db, &params.assignment_id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to load assignment: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            tracing::error!("Assignment {} not found", params.assignment_id);
            StatusCode::NOT_FOUND
        })?;

    let contact = assignment
        .contacts
        .get(params.contact_index)
        .ok_or_else(|| {
            tracing::error!(
                "Contact index {} out of bounds for assignment {}",
                params.contact_index,
                params.assignment_id
            );
            StatusCode::BAD_REQUEST
        })?;

    let contact_name = contact.name.clone().unwrap_or_default();
    let host = &state.server_host;

    let twiml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
    <Connect>
        <Stream url="wss://{host}/media-stream">
            <Parameter name="assignmentId" value="{}" />
            <Parameter name="contactIndex" value="{}" />
            <Parameter name="objective" value="{}" />
            <Parameter name="contactName" value="{}" />
            <Parameter name="contactPhone" value="{}" />
            <Parameter name="profileId" value="{}" />
        </Stream>
    </Connect>
</Response>"#,
        params.assignment_id,
        params.contact_index,
        assignment.objective,
        contact_name,
        contact.phone,
        assignment.profile_id,
    );

    Ok(Html(twiml))
}

#[derive(serde::Deserialize)]
pub struct CallStatusParams {
    pub assignment_id: String,
    pub contact_index: usize,
}

/// Status callback from Twilio for outbound calls.
pub async fn outbound_call_status(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CallStatusParams>,
    Form(form): Form<HashMap<String, String>>,
) -> StatusCode {
    let call_status = form.get("CallStatus").map(|s| s.as_str()).unwrap_or("");
    let call_sid = form.get("CallSid").cloned().unwrap_or_default();

    tracing::info!(
        "Outbound call status: assignment={} contact={} status={call_status} sid={call_sid}",
        params.assignment_id,
        params.contact_index,
    );

    let failed_statuses = ["busy", "no-answer", "failed", "canceled"];

    if failed_statuses.contains(&call_status) || call_status == "completed" {
        let db = state.firestore_db.clone();
        let aid = params.assignment_id.clone();
        let cidx = params.contact_index;
        let status = if failed_statuses.contains(&call_status) {
            "failed"
        } else {
            "completed"
        };
        let error = if failed_statuses.contains(&call_status) {
            Some(call_status.to_string())
        } else {
            None
        };

        tokio::spawn(async move {
            if let Ok(Some(mut assignment)) = db::get_assignment(&db, &aid).await {
                if let Some(contact) = assignment.contacts.get_mut(cidx) {
                    // Only update if not already completed (media_stream may have set it)
                    if contact.status != "completed" {
                        contact.status = status.to_string();
                        contact.call_sid = Some(call_sid);
                        if let Some(err) = error {
                            contact.error = Some(err);
                        }
                    }

                    // Check if all contacts are done
                    let all_done = assignment.contacts.iter().all(|c| {
                        c.status == "completed" || (c.status == "failed" && c.attempts >= 3)
                    });
                    if all_done {
                        assignment.status = "completed".to_string();
                    }

                    if let Err(e) = db::update_assignment(&db, &aid, &assignment).await {
                        tracing::error!("Failed to update assignment after status callback: {e}");
                    }
                }
            }
        });
    }

    StatusCode::OK
}
