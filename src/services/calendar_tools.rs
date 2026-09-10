use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde_json::{Value, json};

use crate::services::db::{self, AgentProfile, BookingRecord};
use crate::services::google_calendar::{self, BusyInterval};
use crate::state::AppState;

/// Returns the list of LLM tool definitions if calendar access is configured
/// and at least one account has tokens stored. Returns empty vec otherwise.
pub async fn available_tools(state: &Arc<AppState>) -> Vec<crate::services::llm::Tool> {
    if state.google_oauth.is_none() {
        return Vec::new();
    }
    match db::list_calendar_tokens(&state.firestore_db).await {
        Ok(tokens) if !tokens.is_empty() => build_tool_definitions(),
        _ => Vec::new(),
    }
}

fn build_tool_definitions() -> Vec<crate::services::llm::Tool> {
    use crate::services::llm::Tool;
    vec![
        Tool {
            name: "check_availability".to_string(),
            description: "Check which time slots are busy on the owner's calendars within a \
                given window. Returns a list of busy intervals merged across all connected \
                calendars (personal and work). Times must be ISO-8601 with offset."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "start": {
                        "type": "string",
                        "description": "Window start as ISO-8601 datetime with timezone offset"
                    },
                    "end": {
                        "type": "string",
                        "description": "Window end as ISO-8601 datetime with timezone offset"
                    }
                },
                "required": ["start", "end"]
            }),
        },
        Tool {
            name: "suggest_meeting_slots".to_string(),
            description: "Find open meeting slots of the given duration within the owner's \
                working hours across the next several days. Returns up to 5 suggested slots."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "duration_minutes": {
                        "type": "integer",
                        "description": "Meeting duration in minutes",
                        "minimum": 15
                    },
                    "earliest": {
                        "type": "string",
                        "description": "Earliest acceptable start as ISO-8601 datetime with offset"
                    },
                    "latest": {
                        "type": "string",
                        "description": "Latest acceptable end as ISO-8601 datetime with offset"
                    }
                },
                "required": ["duration_minutes", "earliest", "latest"]
            }),
        },
        Tool {
            name: "book_meeting".to_string(),
            description: "Create a calendar event on the owner's personal calendar. Only call \
                this AFTER verbally confirming the exact date, time, duration, and title with \
                the caller. If the caller provided an email address, pass it as attendee_email \
                so they receive an invite."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short event title"
                    },
                    "start": {
                        "type": "string",
                        "description": "Event start as ISO-8601 datetime with offset"
                    },
                    "end": {
                        "type": "string",
                        "description": "Event end as ISO-8601 datetime with offset"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional event description / context"
                    },
                    "attendee_email": {
                        "type": "string",
                        "description": "Optional caller email address to invite"
                    }
                },
                "required": ["title", "start", "end"]
            }),
        },
    ]
}

pub struct ToolContext<'a> {
    pub state: &'a Arc<AppState>,
    pub profile: &'a AgentProfile,
    pub call_sid: &'a str,
    pub caller_phone: &'a str,
}

/// Execute a tool call. Always returns a JSON value to send back as tool_result.
/// Errors are returned as `{"error": "..."}` rather than failing the whole turn.
pub async fn dispatch(ctx: &ToolContext<'_>, name: &str, input: &Value) -> Value {
    let result = match name {
        "check_availability" => check_availability(ctx, input).await,
        "suggest_meeting_slots" => suggest_slots(ctx, input).await,
        "book_meeting" => book_meeting(ctx, input).await,
        _ => Err(format!("Unknown tool: {name}")),
    };
    match result {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Tool '{name}' failed: {e}");
            json!({ "error": e })
        }
    }
}

async fn check_availability(ctx: &ToolContext<'_>, input: &Value) -> Result<Value, String> {
    let start = parse_dt(input, "start")?;
    let end = parse_dt(input, "end")?;
    let cfg = ctx
        .state
        .google_oauth
        .as_ref()
        .ok_or("Calendar not configured")?;

    let busy = google_calendar::combined_busy(
        &ctx.state.firestore_db,
        &cfg.client_id,
        &cfg.client_secret,
        start,
        end,
    )
    .await
    .map_err(|e| e.to_string())?;

    let tz: Tz = ctx.profile.timezone.parse().unwrap_or(chrono_tz::UTC);
    let busy_json: Vec<Value> = busy
        .into_iter()
        .map(|b| {
            json!({
                "start": b.start.with_timezone(&tz).to_rfc3339(),
                "end": b.end.with_timezone(&tz).to_rfc3339(),
            })
        })
        .collect();

    Ok(json!({
        "timezone": ctx.profile.timezone,
        "busy": busy_json,
    }))
}

async fn suggest_slots(ctx: &ToolContext<'_>, input: &Value) -> Result<Value, String> {
    let duration_minutes = input
        .get("duration_minutes")
        .and_then(|v| v.as_i64())
        .ok_or("Missing duration_minutes")?;
    let earliest = parse_dt(input, "earliest")?;
    let latest = parse_dt(input, "latest")?;
    let cfg = ctx
        .state
        .google_oauth
        .as_ref()
        .ok_or("Calendar not configured")?;

    let busy = google_calendar::combined_busy(
        &ctx.state.firestore_db,
        &cfg.client_id,
        &cfg.client_secret,
        earliest,
        latest,
    )
    .await
    .map_err(|e| e.to_string())?;

    let tz: Tz = ctx
        .profile
        .timezone
        .parse()
        .map_err(|_| format!("Invalid profile timezone: {}", ctx.profile.timezone))?;

    let slots = find_open_slots(
        earliest,
        latest,
        Duration::minutes(duration_minutes),
        &busy,
        tz,
        ctx.profile.working_hours_start,
        ctx.profile.working_hours_end,
        &ctx.profile.working_days,
        5,
    );

    let slots_json: Vec<Value> = slots
        .into_iter()
        .map(|(s, e)| {
            json!({
                "start": s.with_timezone(&tz).to_rfc3339(),
                "end": e.with_timezone(&tz).to_rfc3339(),
            })
        })
        .collect();

    Ok(json!({
        "timezone": ctx.profile.timezone,
        "slots": slots_json,
    }))
}

#[allow(clippy::too_many_arguments)]
fn find_open_slots(
    earliest: DateTime<Utc>,
    latest: DateTime<Utc>,
    duration: Duration,
    busy: &[BusyInterval],
    tz: Tz,
    wh_start: u32,
    wh_end: u32,
    working_days: &[u32],
    max_slots: usize,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut results = Vec::new();
    // Round cursor up to next 15-minute boundary in the target timezone
    let mut cursor = round_up_15min(earliest, tz);

    while cursor + duration <= latest && results.len() < max_slots {
        let local = cursor.with_timezone(&tz);
        let weekday_num = local.weekday().num_days_from_monday() + 1; // 1=Mon..7=Sun

        if !working_days.contains(&weekday_num) {
            cursor = next_day_start(local, tz, wh_start);
            continue;
        }

        let hour = local.hour();
        if hour < wh_start {
            // Jump to working hours start today
            if let Some(jump) = local_time_to_utc(local, tz, wh_start) {
                cursor = jump;
                continue;
            }
        }
        if hour >= wh_end {
            cursor = next_day_start(local, tz, wh_start);
            continue;
        }

        let slot_end = cursor + duration;
        // Does slot end exceed working hours?
        let slot_end_local = slot_end.with_timezone(&tz);
        if slot_end_local.hour() > wh_end
            || (slot_end_local.hour() == wh_end && slot_end_local.minute() > 0)
            || slot_end_local.day() != local.day()
        {
            cursor = next_day_start(local, tz, wh_start);
            continue;
        }

        // Check collision with busy
        let conflict = busy.iter().find(|b| b.start < slot_end && b.end > cursor);
        if let Some(c) = conflict {
            cursor = round_up_15min(c.end, tz);
            continue;
        }

        results.push((cursor, slot_end));
        cursor = slot_end;
    }

    results
}

fn round_up_15min(dt: DateTime<Utc>, tz: Tz) -> DateTime<Utc> {
    let local = dt.with_timezone(&tz);
    let minute = local.minute();
    let remainder = minute % 15;
    let add = if remainder == 0 && local.second() == 0 && local.nanosecond() == 0 {
        0
    } else {
        15 - remainder
    };
    let rounded = local
        .with_second(0)
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(local)
        + Duration::minutes(add as i64);
    rounded.with_timezone(&Utc)
}

fn next_day_start(local: DateTime<Tz>, tz: Tz, wh_start: u32) -> DateTime<Utc> {
    let next = local.date_naive() + Duration::days(1);
    tz.with_ymd_and_hms(next.year(), next.month(), next.day(), wh_start, 0, 0)
        .single()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

fn local_time_to_utc(local: DateTime<Tz>, tz: Tz, hour: u32) -> Option<DateTime<Utc>> {
    tz.with_ymd_and_hms(local.year(), local.month(), local.day(), hour, 0, 0)
        .single()
        .map(|d| d.with_timezone(&Utc))
}

async fn book_meeting(ctx: &ToolContext<'_>, input: &Value) -> Result<Value, String> {
    let title = input
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or("Missing title")?
        .to_string();
    let start = parse_dt(input, "start")?;
    let end = parse_dt(input, "end")?;
    let description = input
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let attendee_email = input
        .get("attendee_email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let cfg = ctx
        .state
        .google_oauth
        .as_ref()
        .ok_or("Calendar not configured")?;

    // Always book to "personal"
    let label = "personal";
    let tokens = google_calendar::get_valid_access_token(
        &ctx.state.firestore_db,
        &cfg.client_id,
        &cfg.client_secret,
        label,
    )
    .await
    .map_err(|e| format!("No personal calendar connected: {e}"))?;

    // Always target the authenticated user's primary calendar. `primary` is
    // Google's alias for that. The stored calendar_ids list may include
    // read-only shared calendars, and first() isn't necessarily writable.
    let calendar_id = "primary";

    let booked = google_calendar::create_event(
        &tokens.access_token,
        calendar_id,
        &title,
        description.as_deref(),
        start,
        end,
        &ctx.profile.timezone,
        attendee_email.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Audit log — fire and forget
    let record = BookingRecord {
        call_sid: ctx.call_sid.to_string(),
        caller_phone: ctx.caller_phone.to_string(),
        calendar_label: label.to_string(),
        event_id: booked.id.clone(),
        title: title.clone(),
        start,
        end,
        attendee_email: attendee_email.clone(),
        created_at: Utc::now(),
    };
    let db = ctx.state.firestore_db.clone();
    tokio::spawn(async move {
        if let Err(e) = db::save_booking(&db, &record).await {
            tracing::error!("Failed to save booking audit: {e}");
        }
    });

    tracing::info!(
        "Booked meeting '{title}' {start} → {end} for {} (event {})",
        ctx.caller_phone,
        booked.id
    );

    Ok(json!({
        "status": "booked",
        "event_id": booked.id,
        "html_link": booked.html_link,
        "invited": attendee_email.is_some(),
    }))
}

fn parse_dt(input: &Value, key: &str) -> Result<DateTime<Utc>, String> {
    let raw = input
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("Missing {key}"))?;
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| format!("Invalid {key}: {e}"))
}

/// Build a context block for the system prompt describing current time in the
/// owner's timezone plus booking rules.
pub fn build_calendar_context(profile: &AgentProfile, tools_available: bool) -> String {
    if !tools_available {
        return String::new();
    }
    let tz: Tz = profile.timezone.parse().unwrap_or(chrono_tz::UTC);
    let now_local = Utc::now().with_timezone(&tz);
    let working_days_str = profile
        .working_days
        .iter()
        .map(|d| match d {
            1 => "Mon",
            2 => "Tue",
            3 => "Wed",
            4 => "Thu",
            5 => "Fri",
            6 => "Sat",
            7 => "Sun",
            _ => "?",
        })
        .collect::<Vec<_>>()
        .join(", ");

    let rules = if profile.calendar_prompt.is_empty() {
        "\nScheduling rules:\n\
         - Use check_availability or suggest_meeting_slots before proposing times.\n\
         - Never book a meeting without first verbally confirming the exact date, time, \
           and title with the caller. Repeat the details back and wait for their yes.\n\
         - If the caller gives an email, pass it as attendee_email so they get an invite.\n\
         - Only book on the personal calendar.\n\
         - Your responses are spoken aloud by a voice model. Respond in plain \
           conversational text ONLY. Never use markdown: no asterisks, no bullet \
           lists, no headings, no bold. When offering multiple time slots, speak \
           them naturally like \"nine to ten, ten to eleven, or eleven to noon\".\n"
            .to_string()
    } else {
        format!("\n{}\n", profile.calendar_prompt)
    };

    format!(
        "\n\n--- Calendar context ---\n\
         Current time: {} ({})\n\
         Owner timezone: {}\n\
         Working hours: {:02}:00–{:02}:00 {}\n\
         You have calendar tools available: check_availability, suggest_meeting_slots, book_meeting.{}",
        now_local.format("%A %Y-%m-%d %H:%M"),
        profile.timezone,
        profile.timezone,
        profile.working_hours_start,
        profile.working_hours_end,
        working_days_str,
        rules
    )
}
