use chrono::{DateTime, Duration, Utc};
use firestore::FirestoreDb;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::services::db::{self, CalendarTokens};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CALENDAR_API: &str = "https://www.googleapis.com/calendar/v3";

pub const SCOPES: &str = "https://www.googleapis.com/auth/calendar.events \
                          https://www.googleapis.com/auth/calendar.readonly \
                          https://www.googleapis.com/auth/userinfo.email";

#[derive(Debug, Clone)]
pub struct BusyInterval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct BookedEvent {
    pub id: String,
    pub html_link: Option<String>,
}

#[derive(Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
pub struct TokenExchangeResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    pub scope: String,
}

/// Exchange an authorization code for tokens during the OAuth callback.
pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
) -> Result<TokenExchangeResponse, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();
    let params = [
        ("code", code),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("redirect_uri", redirect_uri),
        ("grant_type", "authorization_code"),
    ];
    let resp = client.post(TOKEN_URL).form(&params).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Token exchange failed {status}: {text}").into());
    }
    Ok(resp.json::<TokenExchangeResponse>().await?)
}

/// Verify that the stored refresh token still works by attempting a refresh.
/// Returns Ok(()) if Google issues a new access token, Err with the response
/// body otherwise (e.g. `invalid_grant` when the user revoked access).
pub async fn validate_refresh_token(
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<(), String> {
    let client = Client::new();
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    let resp = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    Err(format!("{status}: {text}"))
}

/// Get a valid access token for the given label, refreshing if expired.
pub async fn get_valid_access_token(
    db: &FirestoreDb,
    client_id: &str,
    client_secret: &str,
    label: &str,
) -> Result<CalendarTokens, Box<dyn std::error::Error + Send + Sync>> {
    let mut tokens = db::get_calendar_tokens(db, label)
        .await?
        .ok_or_else(|| format!("No calendar tokens for label '{label}'"))?;

    // Refresh if within 60 seconds of expiry
    if tokens.expires_at - Utc::now() < Duration::seconds(60) {
        let client = Client::new();
        let params = [
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", tokens.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];
        let resp = client.post(TOKEN_URL).form(&params).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Token refresh failed {status}: {text}").into());
        }
        let refreshed: TokenRefreshResponse = resp.json().await?;
        tokens.access_token = refreshed.access_token;
        tokens.expires_at = Utc::now() + Duration::seconds(refreshed.expires_in);
        if let Some(s) = refreshed.scope {
            tokens.scope = s;
        }
        tokens.updated_at = Utc::now();
        db::upsert_calendar_tokens(db, &tokens).await?;
    }

    Ok(tokens)
}

#[derive(Deserialize)]
struct UserInfo {
    email: String,
}

pub async fn fetch_user_email(
    access_token: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();
    let resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(access_token)
        .send()
        .await?;
    let info: UserInfo = resp.json().await?;
    Ok(info.email)
}

#[derive(Deserialize)]
struct CalendarListResponse {
    items: Vec<CalendarListEntry>,
}

#[derive(Deserialize)]
struct CalendarListEntry {
    id: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    selected: bool,
}

/// List calendar IDs the user has access to, preferring primary + selected calendars.
pub async fn list_calendar_ids(
    access_token: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();
    let resp = client
        .get(format!("{CALENDAR_API}/users/me/calendarList"))
        .bearer_auth(access_token)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("calendarList failed {status}: {text}").into());
    }
    let list: CalendarListResponse = resp.json().await?;
    let ids: Vec<String> = list
        .items
        .into_iter()
        .filter(|c| c.primary || c.selected)
        .map(|c| c.id)
        .collect();
    Ok(ids)
}

#[derive(Deserialize)]
struct FreeBusyResponse {
    calendars: std::collections::HashMap<String, FreeBusyCalendar>,
}

#[derive(Deserialize)]
struct FreeBusyCalendar {
    #[serde(default)]
    busy: Vec<FreeBusySlot>,
}

#[derive(Deserialize)]
struct FreeBusySlot {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

/// Query free/busy for a set of calendars within a time window.
pub async fn free_busy(
    access_token: &str,
    calendar_ids: &[String],
    time_min: DateTime<Utc>,
    time_max: DateTime<Utc>,
) -> Result<Vec<BusyInterval>, Box<dyn std::error::Error + Send + Sync>> {
    if calendar_ids.is_empty() {
        return Ok(Vec::new());
    }
    let client = Client::new();
    let body = json!({
        "timeMin": time_min.to_rfc3339(),
        "timeMax": time_max.to_rfc3339(),
        "items": calendar_ids.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
    });
    let resp = client
        .post(format!("{CALENDAR_API}/freeBusy"))
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("freeBusy failed {status}: {text}").into());
    }
    let parsed: FreeBusyResponse = resp.json().await?;
    let mut intervals: Vec<BusyInterval> = parsed
        .calendars
        .into_values()
        .flat_map(|c| {
            c.busy.into_iter().map(|s| BusyInterval {
                start: s.start,
                end: s.end,
            })
        })
        .collect();
    intervals.sort_by_key(|i| i.start);
    Ok(intervals)
}

/// Pull busy intervals from every stored calendar account and merge overlapping ones.
pub async fn combined_busy(
    db: &FirestoreDb,
    client_id: &str,
    client_secret: &str,
    time_min: DateTime<Utc>,
    time_max: DateTime<Utc>,
) -> Result<Vec<BusyInterval>, Box<dyn std::error::Error + Send + Sync>> {
    let labels: Vec<String> = db::list_calendar_tokens(db)
        .await?
        .into_iter()
        .map(|t| t.label)
        .collect();

    let mut all = Vec::new();
    for label in labels {
        let tokens = get_valid_access_token(db, client_id, client_secret, &label).await?;
        let ids = if tokens.calendar_ids.is_empty() {
            list_calendar_ids(&tokens.access_token)
                .await
                .unwrap_or_default()
        } else {
            tokens.calendar_ids.clone()
        };
        let busy = free_busy(&tokens.access_token, &ids, time_min, time_max).await?;
        all.extend(busy);
    }

    all.sort_by_key(|i| i.start);
    Ok(merge_intervals(all))
}

fn merge_intervals(intervals: Vec<BusyInterval>) -> Vec<BusyInterval> {
    let mut merged: Vec<BusyInterval> = Vec::new();
    for iv in intervals {
        if let Some(last) = merged.last_mut()
            && iv.start <= last.end
        {
            if iv.end > last.end {
                last.end = iv.end;
            }
            continue;
        }
        merged.push(iv);
    }
    merged
}

#[derive(Serialize)]
struct EventDateTime {
    #[serde(rename = "dateTime")]
    date_time: String,
    #[serde(rename = "timeZone")]
    time_zone: String,
}

#[derive(Serialize)]
struct EventAttendee {
    email: String,
}

#[derive(Serialize)]
struct CreateEventBody {
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    start: EventDateTime,
    end: EventDateTime,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attendees: Vec<EventAttendee>,
}

#[derive(Deserialize)]
struct CreatedEvent {
    id: String,
    #[serde(rename = "htmlLink", default)]
    html_link: Option<String>,
}

/// Create a calendar event on the primary calendar of the given account.
pub async fn create_event(
    access_token: &str,
    calendar_id: &str,
    summary: &str,
    description: Option<&str>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    time_zone: &str,
    attendee_email: Option<&str>,
) -> Result<BookedEvent, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();

    let body = CreateEventBody {
        summary: summary.to_string(),
        description: description.map(|s| s.to_string()),
        start: EventDateTime {
            date_time: start.to_rfc3339(),
            time_zone: time_zone.to_string(),
        },
        end: EventDateTime {
            date_time: end.to_rfc3339(),
            time_zone: time_zone.to_string(),
        },
        attendees: attendee_email
            .map(|e| {
                vec![EventAttendee {
                    email: e.to_string(),
                }]
            })
            .unwrap_or_default(),
    };

    let url = format!(
        "{CALENDAR_API}/calendars/{}/events?sendUpdates=all",
        urlencoding_encode(calendar_id)
    );

    let resp = client
        .post(&url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Event create failed {status}: {text}").into());
    }

    let created: CreatedEvent = resp.json().await?;
    Ok(BookedEvent {
        id: created.id,
        html_link: created.html_link,
    })
}

fn urlencoding_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
