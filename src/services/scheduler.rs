use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::services::db;
use crate::state::AppState;

const RETRY_DELAYS_SECS: [i64; 3] = [300, 900, 1800]; // 5min, 15min, 30min

pub async fn run(state: Arc<AppState>) {
    tracing::info!("Assignment scheduler started");

    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;

        if let Err(e) = check_and_execute(&state).await {
            tracing::error!("Scheduler error: {e}");
        }
    }
}

async fn check_and_execute(
    state: &AppState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let assignments = db::list_due_assignments(&state.firestore_db).await?;

    for mut assignment in assignments {
        tracing::info!(
            "Processing assignment {}: {}",
            assignment.id,
            assignment.objective
        );

        assignment.status = "in_progress".to_string();
        db::update_assignment(&state.firestore_db, &assignment.id, &assignment).await?;

        let now = Utc::now();

        for (idx, contact) in assignment.contacts.iter_mut().enumerate() {
            if contact.status == "completed" || contact.status == "in_progress" {
                continue;
            }

            if contact.status == "failed" {
                if contact.attempts >= 3 {
                    continue; // Max retries reached
                }
                // Check retry delay
                if let Some(last) = contact.last_attempt_at {
                    let delay = RETRY_DELAYS_SECS
                        .get(contact.attempts as usize - 1)
                        .copied()
                        .unwrap_or(1800);
                    if (now - last).num_seconds() < delay {
                        continue; // Not time to retry yet
                    }
                }
            }

            // Initiate the call
            match initiate_outbound_call(state, &assignment.id, idx, &contact.phone).await {
                Ok(call_sid) => {
                    tracing::info!(
                        "Initiated call to {} (assignment={}, contact={}): sid={}",
                        contact.phone,
                        assignment.id,
                        idx,
                        call_sid
                    );
                    contact.status = "in_progress".to_string();
                    contact.attempts += 1;
                    contact.last_attempt_at = Some(now);
                    contact.call_sid = Some(call_sid);
                    contact.error = None;
                }
                Err(e) => {
                    tracing::error!("Failed to initiate call to {}: {e}", contact.phone);
                    contact.status = "failed".to_string();
                    contact.attempts += 1;
                    contact.last_attempt_at = Some(now);
                    contact.error = Some(e.to_string());
                }
            }
        }

        // Check if all contacts are done
        let all_done = assignment
            .contacts
            .iter()
            .all(|c| c.status == "completed" || (c.status == "failed" && c.attempts >= 3));
        if all_done {
            assignment.status = "completed".to_string();
        }

        db::update_assignment(&state.firestore_db, &assignment.id, &assignment).await?;
    }

    Ok(())
}

async fn initiate_outbound_call(
    state: &AppState,
    assignment_id: &str,
    contact_index: usize,
    to_phone: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = reqwest::Client::new();

    let twiml_url = format!(
        "https://{}/outbound-call?assignment_id={}&contact_index={}",
        state.server_host, assignment_id, contact_index
    );
    let status_url = format!(
        "https://{}/outbound-call-status?assignment_id={}&contact_index={}",
        state.server_host, assignment_id, contact_index
    );

    let params = [
        ("From", state.twilio_phone_number.as_str()),
        ("To", to_phone),
        ("Url", &twiml_url),
        ("Method", "POST"),
        ("StatusCallback", &status_url),
        ("StatusCallbackMethod", "POST"),
    ];

    let response = client
        .post(format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Calls.json",
            state.twilio_account_sid
        ))
        .basic_auth(&state.twilio_account_sid, Some(&state.twilio_auth_token))
        .form(&params)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await?;
        return Err(format!("Twilio API error {status}: {error_text}").into());
    }

    let body: serde_json::Value = response.json().await?;
    let call_sid = body["sid"].as_str().unwrap_or_default().to_string();

    Ok(call_sid)
}
