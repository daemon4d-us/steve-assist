use std::sync::Arc;

use chrono::{DateTime, Datelike, Days, Duration, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use serde_json::{Value, json};

use crate::services::db::{self, AgentProfile, BookingRecord};
use crate::services::google_calendar::{self, BusyInterval};
use crate::services::llm::Message;
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
                    },
                    "start_weekday": {
                        "type": "string",
                        "description": "Day of the week the caller asked for, e.g. Friday. If start falls on another day the window is moved to that weekday and the result says so"
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
                    },
                    "start_weekday": {
                        "type": "string",
                        "description": "Day of the week the caller asked for, e.g. Friday. If earliest falls on another day the window is moved to that weekday and the result says so"
                    }
                },
                "required": ["duration_minutes", "earliest", "latest"]
            }),
        },
        Tool {
            name: "book_meeting".to_string(),
            description: "Create a calendar event on the owner's personal calendar. Booking \
                takes two calls: the first call for a slot never books, it returns \
                needs_confirmation with the date and time to repeat to the caller. Repeat them \
                with the title, ask whether that is right, and only after the caller has said \
                yes call it again with the same start and end and caller_confirmed set to \
                true. If the caller provided an email address, pass it as attendee_email so \
                they receive an invite."
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
                    "start_weekday": {
                        "type": "string",
                        "description": "Day of the week the caller asked for, e.g. Friday. The call is rejected if start does not fall on that day"
                    },
                    "caller_confirmed": {
                        "type": "boolean",
                        "description": "true only when the caller has said yes, in a later turn, to the exact date, time and title you repeated to them after the first call returned needs_confirmation"
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional event description / context"
                    },
                    "attendee_email": {
                        "type": "string",
                        "description": "Optional: the caller's email address, only if they volunteered one. Never ask for an email and never wait for one; the meeting is booked without it"
                    }
                },
                "required": ["title", "start", "end", "start_weekday"]
            }),
        },
    ]
}

pub struct ToolContext<'a> {
    pub state: &'a Arc<AppState>,
    pub profile: &'a AgentProfile,
    pub call_sid: &'a str,
    pub caller_phone: &'a str,
    /// The conversation so far, up to and including the current turn's
    /// earlier tool rounds. `book_meeting` reads it to tell whether the caller
    /// has had a chance to confirm.
    pub history: &'a [Message],
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
    let tz: Tz = ctx.profile.timezone.parse().unwrap_or(chrono_tz::UTC);
    let (start, end, date_corrected) = snap_to_weekday(
        input,
        parse_dt(input, "start")?,
        parse_dt(input, "end")?,
        tz,
        Utc::now(),
    );
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

    let busy_json: Vec<Value> = busy
        .into_iter()
        .map(|b| {
            json!({
                "start": b.start.with_timezone(&tz).to_rfc3339(),
                "end": b.end.with_timezone(&tz).to_rfc3339(),
            })
        })
        .collect();

    let mut result = json!({
        "timezone": ctx.profile.timezone,
        "window_start": spoken_dt(start, tz),
        "busy": busy_json,
    });
    if let Some(note) = date_corrected {
        result["date_corrected"] = json!(note);
    }
    Ok(result)
}

async fn suggest_slots(ctx: &ToolContext<'_>, input: &Value) -> Result<Value, String> {
    let duration_minutes = input
        .get("duration_minutes")
        .and_then(|v| v.as_i64())
        .ok_or("Missing duration_minutes")?;
    let tz: Tz = ctx
        .profile
        .timezone
        .parse()
        .map_err(|_| format!("Invalid profile timezone: {}", ctx.profile.timezone))?;
    let (earliest, latest, date_corrected) = snap_to_weekday(
        input,
        parse_dt(input, "earliest")?,
        parse_dt(input, "latest")?,
        tz,
        Utc::now(),
    );
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

    let mut result = json!({
        "timezone": ctx.profile.timezone,
        "slots": slots_json,
    });
    if let Some(note) = date_corrected {
        result["date_corrected"] = json!(note);
    }
    Ok(result)
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
    let tz: Tz = ctx.profile.timezone.parse().unwrap_or(chrono_tz::UTC);
    check_weekday(input, start, tz, Utc::now())?;
    if let Some(mut earlier) = already_booked(ctx.history, start, end) {
        earlier["status"] = json!("already_booked");
        earlier["next_step"] = json!(
            "This slot was already booked earlier in this call; no second event was \
             created. Tell the caller it is booked."
        );
        return Ok(earlier);
    }
    if !caller_confirmed(ctx.history, input, start, end) {
        return Ok(json!({
            "status": "needs_confirmation",
            "title": title,
            "when": spoken_dt(start, tz),
            "end_time": end.with_timezone(&tz).format("%-I:%M %p").to_string(),
            "next_step": "Nothing was booked. Repeat this date, time and title to the caller \
                and ask whether that is right. Once they say yes, call book_meeting again \
                with the same start and end and caller_confirmed set to true.",
        }));
    }
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
        "when": spoken_dt(start, tz),
        "start": start.to_rfc3339(),
        "end": end.to_rfc3339(),
        "event_id": booked.id,
        "html_link": booked.html_link,
        "invited": attendee_email.is_some(),
        "next_step": "The event exists now. Tell the caller it is booked, with the day and \
            time. Do not ask them to confirm again.",
    }))
}

/// The `booked` result of an earlier `book_meeting` call in this call for the
/// same slot, if any. A model that misreads its own booking as still pending
/// asks the caller again and then calls the tool again; answering from the
/// history keeps that from creating a second event.
fn already_booked(history: &[Message], start: DateTime<Utc>, end: DateTime<Utc>) -> Option<Value> {
    history.iter().rev().find_map(|msg| match msg {
        Message::ToolResults(results) => results
            .iter()
            .filter(|r| r.name == "book_meeting")
            .filter_map(|r| serde_json::from_str::<Value>(&r.content).ok())
            .find(|v| {
                v.get("status").and_then(|s| s.as_str()) == Some("booked")
                    && parse_dt(v, "start").ok() == Some(start)
                    && parse_dt(v, "end").ok() == Some(end)
            }),
        _ => None,
    })
}

/// Models book the moment a caller picks a slot, skipping the read-back the
/// scheduling rules ask for, and `caller_confirmed: true` alone is only a
/// claim. The history settles it: the booking goes ahead only if an earlier
/// `book_meeting` call for the same start and end (the proposal) was followed
/// by something the caller said, i.e. the model had to speak the details and
/// wait for an answer before calling again.
fn caller_confirmed(
    history: &[Message],
    input: &Value,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> bool {
    if input.get("caller_confirmed").and_then(|v| v.as_bool()) != Some(true) {
        return false;
    }
    let mut proposed = false;
    for msg in history {
        match msg {
            Message::ToolCalls { calls, .. } => {
                proposed |= calls.iter().any(|c| {
                    c.name == "book_meeting"
                        && parse_dt(&c.input, "start").ok() == Some(start)
                        && parse_dt(&c.input, "end").ok() == Some(end)
                });
            }
            Message::User(_) if proposed => return true,
            _ => {}
        }
    }
    false
}

/// A tool-result date the model can read back to the caller as is, weekday
/// included, so it never has to derive one from an ISO timestamp.
fn spoken_dt(dt: DateTime<Utc>, tz: Tz) -> String {
    dt.with_timezone(&tz)
        .format("%A, %B %-d, %Y, %-I:%M %p")
        .to_string()
}

/// Models miscount days of the week (a meeting asked for "Friday" was booked
/// on Sunday the 11th and announced as "Friday, October 11th"). When the tool
/// call names the weekday the caller asked for, refuse a `start` that falls on
/// another day and hand back the date that weekday really is.
fn check_weekday(
    input: &Value,
    start: DateTime<Utc>,
    tz: Tz,
    now: DateTime<Utc>,
) -> Result<(), String> {
    match weekday_mismatch(input, start, tz, now) {
        None => Ok(()),
        Some((given, next)) => Err(format!(
            "{} is a {}, not a {}. The next {} is {}. Nothing was checked or booked. \
             Use that date instead, and tell the caller the correct date.",
            given.format("%B %-d, %Y"),
            given.format("%A"),
            next.format("%A"),
            next.format("%A"),
            next.format("%B %-d, %Y (%Y-%m-%d)"),
        )),
    }
}

/// The read-only tools' counterpart of [`check_weekday`]: instead of refusing
/// a window on the wrong day, move it (same times) to the weekday the caller
/// asked for and say so, because a refusal was read back to callers as
/// "nothing is free that day".
fn snap_to_weekday(
    input: &Value,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    tz: Tz,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>, Option<String>) {
    let Some((given, next)) = weekday_mismatch(input, start, tz, now) else {
        return (start, end, None);
    };
    let shift = Duration::days((next - given).num_days());
    let note = format!(
        "You asked about {} but gave {}, which is a {}. The window was moved to {}; \
         tell the caller that date, not the one you gave.",
        next.format("%A"),
        given.format("%B %-d, %Y"),
        given.format("%A"),
        next.format("%A, %B %-d, %Y"),
    );
    (start + shift, end + shift, Some(note))
}

/// When `start_weekday` names a day and `start` falls on another, the date
/// that was given and the next date that really is that weekday.
fn weekday_mismatch(
    input: &Value,
    start: DateTime<Utc>,
    tz: Tz,
    now: DateTime<Utc>,
) -> Option<(chrono::NaiveDate, chrono::NaiveDate)> {
    let wanted = input
        .get("start_weekday")
        .and_then(|v| v.as_str())
        .and_then(|s| s.trim().parse::<Weekday>().ok())?;
    let given = start.with_timezone(&tz).date_naive();
    if given.weekday() == wanted {
        return None;
    }
    let today = now.with_timezone(&tz).date_naive();
    let ahead = (7 + wanted.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
    Some((given, today + Days::new(u64::from(ahead))))
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
           and title with the caller. Repeat the details back and wait for their yes. \
           book_meeting enforces this: its first call for a slot only returns the details \
           to confirm, and it books on a second call with caller_confirmed true.\n\
         - An email address is optional. If the caller volunteers one, pass it as \
           attendee_email so they get an invite; otherwise book without it. Never ask for \
           an email or make the booking wait for one. The owner is always on their own \
           calendar and needs no invite.\n\
         - Only book on the personal calendar.\n\
         - Your responses are spoken aloud by a voice model. Respond in plain \
           conversational text ONLY. Never use markdown: no asterisks, no bullet \
           lists, no headings, no bold. When offering multiple time slots, speak \
           them naturally like \"nine to ten, ten to eleven, or eleven to noon\".\n"
            .to_string()
    } else {
        format!(
            "\n{}\n",
            crate::services::assistant::with_agent_name(profile, &profile.calendar_prompt)
        )
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

#[cfg(test)]
mod weekday_tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn rejects_a_start_on_the_wrong_weekday() {
        let tz = chrono_tz::America::Los_Angeles;
        // Monday Oct 5; the model asks for "Friday" but passes Sunday Oct 11, 2 PM PT.
        let err = check_weekday(
            &json!({ "start_weekday": "Friday" }),
            at(2026, 10, 11, 21),
            tz,
            at(2026, 10, 5, 20),
        )
        .unwrap_err();
        assert!(
            err.contains("October 11, 2026 is a Sunday, not a Friday"),
            "{err}"
        );
        assert!(
            err.contains("The next Friday is October 9, 2026 (2026-10-09)"),
            "{err}"
        );
    }

    #[test]
    fn read_only_tools_move_the_window_to_the_asked_weekday() {
        let tz = chrono_tz::America::Los_Angeles;
        let now = at(2026, 10, 5, 20);
        // Saturday Oct 10, 9:00-18:00 PT asked as "Friday" -> Friday Oct 9, same times.
        let (start, end, note) = snap_to_weekday(
            &json!({ "start_weekday": "Friday" }),
            at(2026, 10, 10, 16),
            at(2026, 10, 11, 1),
            tz,
            now,
        );
        assert_eq!(start, at(2026, 10, 9, 16));
        assert_eq!(end, at(2026, 10, 10, 1));
        let note = note.expect("a correction note");
        assert!(
            note.contains("gave October 10, 2026, which is a Saturday"),
            "{note}"
        );
        assert!(note.contains("moved to Friday, October 9, 2026"), "{note}");

        // Matching or absent weekday: untouched, no note.
        let (s, e, n) = snap_to_weekday(
            &json!({ "start_weekday": "Friday" }),
            at(2026, 10, 9, 16),
            at(2026, 10, 9, 17),
            tz,
            now,
        );
        assert_eq!((s, e, n), (at(2026, 10, 9, 16), at(2026, 10, 9, 17), None));
        let (s, e, n) = snap_to_weekday(
            &json!({}),
            at(2026, 10, 10, 16),
            at(2026, 10, 10, 17),
            tz,
            now,
        );
        assert_eq!(
            (s, e, n),
            (at(2026, 10, 10, 16), at(2026, 10, 10, 17), None)
        );
    }

    #[test]
    fn accepts_a_matching_or_missing_weekday() {
        let tz = chrono_tz::America::Los_Angeles;
        let start = at(2026, 10, 9, 21);
        let now = at(2026, 10, 5, 20);
        assert!(check_weekday(&json!({ "start_weekday": " friday " }), start, tz, now).is_ok());
        assert!(check_weekday(&json!({}), start, tz, now).is_ok());
        assert!(check_weekday(&json!({ "start_weekday": "someday" }), start, tz, now).is_ok());
    }

    #[test]
    fn weekday_is_judged_in_the_profile_timezone() {
        // 2026-10-10 02:00 UTC is still Friday evening in Los Angeles.
        let tz = chrono_tz::America::Los_Angeles;
        let now = at(2026, 10, 5, 20);
        assert!(
            check_weekday(
                &json!({ "start_weekday": "Friday" }),
                at(2026, 10, 10, 2),
                tz,
                now
            )
            .is_ok()
        );
        assert_eq!(
            spoken_dt(at(2026, 10, 10, 2), tz),
            "Friday, October 9, 2026, 7:00 PM"
        );
    }
}

#[cfg(test)]
mod confirmation_tests {
    use super::*;
    use crate::services::llm::ToolCall;

    const START: &str = "2026-10-09T09:30:00-07:00";
    const END: &str = "2026-10-09T10:00:00-07:00";

    fn book_call(start: &str, end: &str, confirmed: bool) -> Message {
        Message::ToolCalls {
            text: String::new(),
            calls: vec![ToolCall {
                id: "t1".into(),
                name: "book_meeting".into(),
                input: json!({ "title": "Test", "start": start, "end": end, "caller_confirmed": confirmed }),
            }],
        }
    }

    fn confirmed_now(history: &[Message]) -> bool {
        let input = json!({ "start": START, "end": END, "caller_confirmed": true });
        let start = parse_dt(&input, "start").unwrap();
        let end = parse_dt(&input, "end").unwrap();
        caller_confirmed(history, &input, start, end)
    }

    #[test]
    fn first_call_never_books_even_if_the_model_claims_confirmation() {
        let history = [Message::User("the first one works".into())];
        assert!(!confirmed_now(&history));
        assert!(!caller_confirmed(
            &history,
            &json!({ "caller_confirmed": false }),
            parse_dt(&json!({ "s": START }), "s").unwrap(),
            parse_dt(&json!({ "e": END }), "e").unwrap(),
        ));
    }

    #[test]
    fn a_second_call_in_the_same_turn_does_not_count() {
        // Proposal and "confirmed" call in consecutive tool rounds, no caller in between.
        let history = [
            Message::User("the first one works".into()),
            book_call(START, END, false),
            Message::ToolResults(vec![]),
        ];
        assert!(!confirmed_now(&history));
    }

    #[test]
    fn books_once_the_caller_has_spoken_after_the_proposal() {
        let history = [
            Message::User("the first one works".into()),
            book_call(START, END, false),
            Message::ToolResults(vec![]),
            Message::Assistant("That is Friday, October 9 at 9:30. Shall I book it?".into()),
            Message::User("yes, go ahead".into()),
        ];
        assert!(confirmed_now(&history));
    }

    #[test]
    fn a_proposal_for_a_different_slot_does_not_count() {
        let history = [
            book_call(
                "2026-10-09T10:00:00-07:00",
                "2026-10-09T10:30:00-07:00",
                false,
            ),
            Message::ToolResults(vec![]),
            Message::Assistant("Friday at 10?".into()),
            Message::User("no, 9:30 please".into()),
        ];
        assert!(!confirmed_now(&history));
    }

    #[test]
    fn a_slot_booked_earlier_in_the_call_is_not_booked_twice() {
        let booked = crate::services::llm::ToolResult {
            call_id: "t2".into(),
            name: "book_meeting".into(),
            content: json!({
                "status": "booked", "start": "2026-10-09T16:30:00+00:00",
                "end": "2026-10-09T17:00:00+00:00", "event_id": "ev1"
            })
            .to_string(),
        };
        let pending = crate::services::llm::ToolResult {
            call_id: "t1".into(),
            name: "book_meeting".into(),
            content: json!({ "status": "needs_confirmation", "start": START, "end": END })
                .to_string(),
        };
        let input = json!({ "start": START, "end": END });
        let start = parse_dt(&input, "start").unwrap();
        let end = parse_dt(&input, "end").unwrap();

        let history = [Message::ToolResults(vec![pending.clone()])];
        assert!(already_booked(&history, start, end).is_none());

        let history = [
            Message::ToolResults(vec![pending]),
            Message::User("yes".into()),
            Message::ToolResults(vec![booked]),
            Message::User("yes, book it".into()),
        ];
        let hit = already_booked(&history, start, end).expect("found the earlier booking");
        assert_eq!(hit["event_id"], "ev1");
        // A different slot is not covered by it.
        assert!(already_booked(&history, start, start + Duration::minutes(45)).is_none());
    }

    #[test]
    fn the_same_instant_in_another_offset_still_matches() {
        let history = [
            book_call("2026-10-09T16:30:00Z", "2026-10-09T17:00:00Z", false),
            Message::ToolResults(vec![]),
            Message::User("yes".into()),
        ];
        assert!(confirmed_now(&history));
    }
}
