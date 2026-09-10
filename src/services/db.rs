use chrono::{DateTime, Utc};
use firestore::*;
use serde::{Deserialize, Serialize};

const AGENT_PROFILES_COLLECTION: &str = "agent_profiles";
const ASSIGNMENTS_COLLECTION: &str = "assignments";
const CALLERS_COLLECTION: &str = "callers";
const SESSIONS_COLLECTION: &str = "call_sessions";
const MESSAGES_COLLECTION: &str = "messages";
const MEMORIES_COLLECTION: &str = "memories";
const CALENDAR_TOKENS_COLLECTION: &str = "calendar_tokens";
const BOOKINGS_COLLECTION: &str = "bookings";
const BOT_CALL_GROUPS_COLLECTION: &str = "bot_call_groups";

fn default_timezone() -> String {
    "America/Los_Angeles".to_string()
}

fn default_working_hours_start() -> u32 {
    9
}

fn default_working_hours_end() -> u32 {
    18
}

fn default_working_days() -> Vec<u32> {
    vec![1, 2, 3, 4, 5]
}

// --- Agent profile ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub base_prompt: String,
    pub new_caller_prompt: String,
    pub returning_caller_prompt: String,
    pub memory_prompt: String,
    #[serde(default)]
    pub outbound_prompt: String,
    pub model: String,
    pub max_tokens: u32,
    #[serde(default = "default_timezone")]
    pub timezone: String,
    #[serde(default = "default_working_hours_start")]
    pub working_hours_start: u32,
    #[serde(default = "default_working_hours_end")]
    pub working_hours_end: u32,
    #[serde(default = "default_working_days")]
    pub working_days: Vec<u32>,
    #[serde(default)]
    pub calendar_prompt: String,
    #[serde(default)]
    pub voice_id: Option<String>,
}

// --- Bot call groups ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotCallGroup {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub phone_numbers: Vec<String>,
    pub latest_summary: Option<String>,
    pub latest_call_sid: Option<String>,
    pub latest_call_at: DateTime<Utc>,
    pub call_count: u32,
    pub created_at: DateTime<Utc>,
}

// --- Assignments ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentContact {
    pub phone: String,
    pub name: Option<String>,
    pub status: String,
    pub attempts: u32,
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub call_sid: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    #[serde(default)]
    pub id: String,
    pub objective: String,
    pub profile_id: String,
    pub scheduled_at: DateTime<Utc>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub contacts: Vec<AssignmentContact>,
}

// --- Data types ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerProfile {
    pub name: String,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    pub call_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallSession {
    #[serde(default)]
    pub call_sid: String,
    pub caller_phone: String,
    pub caller_name: Option<String>,
    #[serde(default = "default_direction")]
    pub direction: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallMessage {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub caller_phone: String,
    pub call_sid: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub active: bool,
}

fn default_direction() -> String {
    "inbound".to_string()
}

// --- Initialization ---

pub async fn init(
    project_id: &str,
) -> Result<FirestoreDb, Box<dyn std::error::Error + Send + Sync>> {
    let db = FirestoreDb::new(project_id).await?;
    Ok(db)
}

// --- Agent profile operations ---

pub async fn get_agent_profile(
    db: &FirestoreDb,
    profile_id: &str,
) -> Result<Option<AgentProfile>, Box<dyn std::error::Error + Send + Sync>> {
    let result: Option<AgentProfile> = db
        .fluent()
        .select()
        .by_id_in(AGENT_PROFILES_COLLECTION)
        .obj()
        .one(profile_id)
        .await?;
    Ok(result)
}

pub async fn list_profiles(
    db: &FirestoreDb,
) -> Result<Vec<(String, AgentProfile)>, Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    let mut stream = db
        .fluent()
        .list()
        .from(AGENT_PROFILES_COLLECTION)
        .page_size(100)
        .stream_all()
        .await?;

    let mut entries = Vec::new();
    while let Some(doc) = stream.next().await {
        let id = doc.name.rsplit('/').next().unwrap_or_default().to_string();
        if let Ok(profile) = FirestoreDb::deserialize_doc_to::<AgentProfile>(&doc) {
            entries.push((id, profile));
        }
    }

    Ok(entries)
}

pub async fn update_profile(
    db: &FirestoreDb,
    profile_id: &str,
    profile: &AgentProfile,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _: AgentProfile = db
        .fluent()
        .update()
        .in_col(AGENT_PROFILES_COLLECTION)
        .document_id(profile_id)
        .object(profile)
        .execute()
        .await?;
    Ok(())
}

// --- Caller operations ---

pub async fn get_caller(
    db: &FirestoreDb,
    phone: &str,
) -> Result<Option<CallerProfile>, Box<dyn std::error::Error + Send + Sync>> {
    let result: Option<CallerProfile> = db
        .fluent()
        .select()
        .by_id_in(CALLERS_COLLECTION)
        .obj()
        .one(phone)
        .await?;
    Ok(result)
}

pub async fn upsert_caller(
    db: &FirestoreDb,
    phone: &str,
    name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now();

    match get_caller(db, phone).await? {
        Some(existing) => {
            let updated = CallerProfile {
                name: name.to_string(),
                last_seen: now,
                call_count: existing.call_count + 1,
                ..existing
            };
            let _: CallerProfile = db
                .fluent()
                .update()
                .in_col(CALLERS_COLLECTION)
                .document_id(phone)
                .object(&updated)
                .execute()
                .await?;
        }
        None => {
            let profile = CallerProfile {
                name: name.to_string(),
                first_seen: now,
                last_seen: now,
                call_count: 1,
            };
            let _: CallerProfile = db
                .fluent()
                .insert()
                .into(CALLERS_COLLECTION)
                .document_id(phone)
                .object(&profile)
                .execute()
                .await?;
        }
    }
    Ok(())
}

pub async fn update_caller_last_seen(
    db: &FirestoreDb,
    phone: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(mut caller) = get_caller(db, phone).await? {
        caller.last_seen = Utc::now();
        caller.call_count += 1;
        let _: CallerProfile = db
            .fluent()
            .update()
            .in_col(CALLERS_COLLECTION)
            .document_id(phone)
            .object(&caller)
            .execute()
            .await?;
    }
    Ok(())
}

// --- Session operations ---

pub async fn create_session(
    db: &FirestoreDb,
    call_sid: &str,
    caller_phone: &str,
    direction: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session = CallSession {
        call_sid: call_sid.to_string(),
        caller_phone: caller_phone.to_string(),
        caller_name: None,
        direction: direction.to_string(),
        started_at: Utc::now(),
        ended_at: None,
        summary: None,
    };
    let _: CallSession = db
        .fluent()
        .insert()
        .into(SESSIONS_COLLECTION)
        .document_id(call_sid)
        .object(&session)
        .execute()
        .await?;
    Ok(())
}

pub async fn end_session(
    db: &FirestoreDb,
    call_sid: &str,
    caller_name: Option<&str>,
    summary: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(mut session) = db
        .fluent()
        .select()
        .by_id_in(SESSIONS_COLLECTION)
        .obj::<CallSession>()
        .one(call_sid)
        .await?
    {
        session.ended_at = Some(Utc::now());
        if let Some(name) = caller_name {
            session.caller_name = Some(name.to_string());
        }
        if let Some(s) = summary {
            session.summary = Some(s.to_string());
        }
        let _: CallSession = db
            .fluent()
            .update()
            .in_col(SESSIONS_COLLECTION)
            .document_id(call_sid)
            .object(&session)
            .execute()
            .await?;
    }
    Ok(())
}

// --- Message operations ---

pub async fn save_message(
    db: &FirestoreDb,
    call_sid: &str,
    role: &str,
    content: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let msg = CallMessage {
        role: role.to_string(),
        content: content.to_string(),
        timestamp: Utc::now(),
    };

    let parent_path = db.parent_path(SESSIONS_COLLECTION, call_sid)?;
    let _: CallMessage = db
        .fluent()
        .insert()
        .into(MESSAGES_COLLECTION)
        .document_id(format!("{}_{}", role, Utc::now().timestamp_millis()))
        .parent(&parent_path)
        .object(&msg)
        .execute()
        .await?;
    Ok(())
}

// --- Session list operations ---

pub async fn list_sessions(
    db: &FirestoreDb,
    limit: u32,
) -> Result<Vec<CallSession>, Box<dyn std::error::Error + Send + Sync>> {
    let sessions: Vec<CallSession> = db
        .fluent()
        .select()
        .from(SESSIONS_COLLECTION)
        .order_by([(
            path!(CallSession::started_at),
            FirestoreQueryDirection::Descending,
        )])
        .limit(limit)
        .obj()
        .query()
        .await?;
    Ok(sessions)
}

// --- Memory operations ---

pub async fn get_memories(
    db: &FirestoreDb,
    phone: &str,
    limit: u32,
) -> Result<Vec<Memory>, Box<dyn std::error::Error + Send + Sync>> {
    let memories: Vec<Memory> = db
        .fluent()
        .select()
        .from(MEMORIES_COLLECTION)
        .filter(|q| {
            q.for_all([
                q.field(path!(Memory::caller_phone)).eq(phone),
                q.field(path!(Memory::active)).eq(true),
            ])
        })
        .order_by([(
            path!(Memory::created_at),
            FirestoreQueryDirection::Descending,
        )])
        .limit(limit)
        .obj()
        .query()
        .await?;
    Ok(memories)
}

pub async fn save_memory(
    db: &FirestoreDb,
    phone: &str,
    call_sid: &str,
    content: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let memory = Memory {
        caller_phone: phone.to_string(),
        call_sid: call_sid.to_string(),
        content: content.to_string(),
        created_at: Utc::now(),
        active: true,
    };
    let _: Memory = db
        .fluent()
        .insert()
        .into(MEMORIES_COLLECTION)
        .generate_document_id()
        .object(&memory)
        .execute()
        .await?;
    Ok(())
}

// --- Assignment operations ---

pub async fn create_assignment(
    db: &FirestoreDb,
    assignment: &Assignment,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let result: Assignment = db
        .fluent()
        .insert()
        .into(ASSIGNMENTS_COLLECTION)
        .generate_document_id()
        .object(assignment)
        .execute()
        .await?;
    // The document ID is returned in the result; extract from Firestore response
    // Since generate_document_id doesn't give us the ID directly, we use a workaround
    Ok(result.id)
}

pub async fn get_assignment(
    db: &FirestoreDb,
    id: &str,
) -> Result<Option<Assignment>, Box<dyn std::error::Error + Send + Sync>> {
    let result: Option<Assignment> = db
        .fluent()
        .select()
        .by_id_in(ASSIGNMENTS_COLLECTION)
        .obj()
        .one(id)
        .await?;
    Ok(result)
}

pub async fn list_assignments(
    db: &FirestoreDb,
    limit: u32,
) -> Result<Vec<Assignment>, Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    let mut stream = db
        .fluent()
        .list()
        .from(ASSIGNMENTS_COLLECTION)
        .page_size(limit as usize)
        .stream_all()
        .await?;

    let mut entries = Vec::new();
    while let Some(doc) = stream.next().await {
        let id = doc.name.rsplit('/').next().unwrap_or_default().to_string();
        if let Ok(mut assignment) = FirestoreDb::deserialize_doc_to::<Assignment>(&doc) {
            assignment.id = id;
            entries.push(assignment);
        }
    }

    // Sort by scheduled_at descending
    entries.sort_by(|a, b| b.scheduled_at.cmp(&a.scheduled_at));
    Ok(entries)
}

pub async fn update_assignment(
    db: &FirestoreDb,
    id: &str,
    assignment: &Assignment,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _: Assignment = db
        .fluent()
        .update()
        .in_col(ASSIGNMENTS_COLLECTION)
        .document_id(id)
        .object(assignment)
        .execute()
        .await?;
    Ok(())
}

pub async fn delete_assignment(
    db: &FirestoreDb,
    id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    db.fluent()
        .delete()
        .from(ASSIGNMENTS_COLLECTION)
        .document_id(id)
        .execute()
        .await?;
    Ok(())
}

// --- Calendar token operations ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarTokens {
    pub label: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub scope: String,
    pub account_email: Option<String>,
    pub calendar_ids: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

pub async fn get_calendar_tokens(
    db: &FirestoreDb,
    label: &str,
) -> Result<Option<CalendarTokens>, Box<dyn std::error::Error + Send + Sync>> {
    let result: Option<CalendarTokens> = db
        .fluent()
        .select()
        .by_id_in(CALENDAR_TOKENS_COLLECTION)
        .obj()
        .one(label)
        .await?;
    Ok(result)
}

pub async fn list_calendar_tokens(
    db: &FirestoreDb,
) -> Result<Vec<CalendarTokens>, Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    let mut stream = db
        .fluent()
        .list()
        .from(CALENDAR_TOKENS_COLLECTION)
        .page_size(20)
        .stream_all()
        .await?;

    let mut entries = Vec::new();
    while let Some(doc) = stream.next().await {
        if let Ok(tokens) = FirestoreDb::deserialize_doc_to::<CalendarTokens>(&doc) {
            entries.push(tokens);
        }
    }
    Ok(entries)
}

pub async fn upsert_calendar_tokens(
    db: &FirestoreDb,
    tokens: &CalendarTokens,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let existing = get_calendar_tokens(db, &tokens.label).await?;
    if existing.is_some() {
        let _: CalendarTokens = db
            .fluent()
            .update()
            .in_col(CALENDAR_TOKENS_COLLECTION)
            .document_id(&tokens.label)
            .object(tokens)
            .execute()
            .await?;
    } else {
        let _: CalendarTokens = db
            .fluent()
            .insert()
            .into(CALENDAR_TOKENS_COLLECTION)
            .document_id(&tokens.label)
            .object(tokens)
            .execute()
            .await?;
    }
    Ok(())
}

// --- Booking audit log ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookingRecord {
    pub call_sid: String,
    pub caller_phone: String,
    pub calendar_label: String,
    pub event_id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub attendee_email: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn save_booking(
    db: &FirestoreDb,
    booking: &BookingRecord,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _: BookingRecord = db
        .fluent()
        .insert()
        .into(BOOKINGS_COLLECTION)
        .generate_document_id()
        .object(booking)
        .execute()
        .await?;
    Ok(())
}

pub async fn list_due_assignments(
    db: &FirestoreDb,
) -> Result<Vec<Assignment>, Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    let mut stream = db
        .fluent()
        .list()
        .from(ASSIGNMENTS_COLLECTION)
        .page_size(100)
        .stream_all()
        .await?;

    let now = Utc::now();
    let mut entries = Vec::new();
    while let Some(doc) = stream.next().await {
        let id = doc.name.rsplit('/').next().unwrap_or_default().to_string();
        if let Ok(mut assignment) = FirestoreDb::deserialize_doc_to::<Assignment>(&doc) {
            assignment.id = id;
            if assignment.status == "pending" && assignment.scheduled_at <= now {
                entries.push(assignment);
            }
        }
    }

    Ok(entries)
}

// --- Bot call group operations ---

pub async fn list_bot_groups(
    db: &FirestoreDb,
) -> Result<Vec<BotCallGroup>, Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::StreamExt;

    let mut stream = db
        .fluent()
        .list()
        .from(BOT_CALL_GROUPS_COLLECTION)
        .page_size(200)
        .stream_all()
        .await?;

    let mut entries = Vec::new();
    while let Some(doc) = stream.next().await {
        let id = doc.name.rsplit('/').next().unwrap_or_default().to_string();
        if let Ok(mut group) = FirestoreDb::deserialize_doc_to::<BotCallGroup>(&doc) {
            group.id = id;
            entries.push(group);
        }
    }

    entries.sort_by(|a, b| b.latest_call_at.cmp(&a.latest_call_at));
    Ok(entries)
}

pub async fn create_bot_group(
    db: &FirestoreDb,
    group: &BotCallGroup,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let result: BotCallGroup = db
        .fluent()
        .insert()
        .into(BOT_CALL_GROUPS_COLLECTION)
        .generate_document_id()
        .object(group)
        .execute()
        .await?;
    Ok(result.id)
}

pub async fn update_bot_group(
    db: &FirestoreDb,
    id: &str,
    group: &BotCallGroup,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _: BotCallGroup = db
        .fluent()
        .update()
        .in_col(BOT_CALL_GROUPS_COLLECTION)
        .document_id(id)
        .object(group)
        .execute()
        .await?;
    Ok(())
}

// --- Test-harness helpers (used by the steve-e2e binary) ---

pub async fn list_messages(
    db: &FirestoreDb,
    call_sid: &str,
) -> Result<Vec<CallMessage>, Box<dyn std::error::Error + Send + Sync>> {
    let parent_path = db.parent_path(SESSIONS_COLLECTION, call_sid)?;
    let mut messages: Vec<CallMessage> = db
        .fluent()
        .select()
        .from(MESSAGES_COLLECTION)
        .parent(&parent_path)
        .obj()
        .query()
        .await?;
    messages.sort_by_key(|m| m.timestamp);
    Ok(messages)
}

pub async fn get_session(
    db: &FirestoreDb,
    call_sid: &str,
) -> Result<Option<CallSession>, Box<dyn std::error::Error + Send + Sync>> {
    let session = db
        .fluent()
        .select()
        .by_id_in(SESSIONS_COLLECTION)
        .obj::<CallSession>()
        .one(call_sid)
        .await?;
    Ok(session)
}

pub async fn list_memories_for_call(
    db: &FirestoreDb,
    call_sid: &str,
) -> Result<Vec<Memory>, Box<dyn std::error::Error + Send + Sync>> {
    let memories: Vec<Memory> = db
        .fluent()
        .select()
        .from(MEMORIES_COLLECTION)
        .filter(|q| q.field(path!(Memory::call_sid)).eq(call_sid))
        .obj()
        .query()
        .await?;
    Ok(memories)
}

pub async fn list_bookings_for_call(
    db: &FirestoreDb,
    call_sid: &str,
) -> Result<Vec<BookingRecord>, Box<dyn std::error::Error + Send + Sync>> {
    let bookings: Vec<BookingRecord> = db
        .fluent()
        .select()
        .from(BOOKINGS_COLLECTION)
        .filter(|q| q.field(path!(BookingRecord::call_sid)).eq(call_sid))
        .obj()
        .query()
        .await?;
    Ok(bookings)
}

/// Remove everything Steve knows about a phone number: the caller profile and
/// all memories. Used to put a test number back into the "new caller" state.
pub async fn forget_caller(
    db: &FirestoreDb,
    phone: &str,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    db.fluent()
        .delete()
        .from(CALLERS_COLLECTION)
        .document_id(phone)
        .execute()
        .await?;

    let docs = db
        .fluent()
        .select()
        .from(MEMORIES_COLLECTION)
        .filter(|q| q.field(path!(Memory::caller_phone)).eq(phone))
        .query()
        .await?;
    let mut removed = 0;
    for doc in docs {
        let id = doc.name.rsplit('/').next().unwrap_or_default().to_string();
        db.fluent()
            .delete()
            .from(MEMORIES_COLLECTION)
            .document_id(&id)
            .execute()
            .await?;
        removed += 1;
    }
    Ok(removed)
}
