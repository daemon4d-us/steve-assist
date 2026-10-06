//! Steve's "brain": prompt composition and the LLM-backed tasks the call loop
//! needs. Everything here is provider-neutral and goes through [`LlmProvider`].

use chrono::{DateTime, Days, Utc};
use chrono_tz::Tz;

use crate::services::call_control::END_CALL_PROMPT;
use crate::services::db::{self, AgentProfile};
use crate::services::llm::{Completion, CompletionRequest, LlmError, LlmProvider, Message, Tool};

/// Reply text goes straight to TTS. Nemotron in particular, with reasoning
/// switched off, otherwise plans out loud ("Let me use the check_availability
/// tool. In ISO format: ...") and the caller hears all of it.
const SPOKEN_ONLY_PROMPT: &str = " Everything you write is spoken aloud to the caller, so write only what you would say to them: never narrate your reasoning, your plan, date arithmetic, tool names or timestamps. When you need a tool, call it right away in the same turn instead of announcing or describing it.";

/// Appended to the system prompt when a reply ran into `max_tokens` without
/// reaching a tool call, for the single retry of that turn.
pub const NO_NARRATION_RETRY: &str = "\n\nIMPORTANT: your previous attempt at this reply was discarded because it narrated your reasoning and ran too long. Do not explain your steps. Either call the tool you need now with no accompanying text, or answer the caller in one or two short sentences.";

/// How many days, starting today, the system prompt lists by weekday and date.
const UPCOMING_DAYS: u64 = 14;

/// The model has no clock: without this it guesses the date when asked and
/// cannot reason about "tomorrow" for calendar tools. Rendered in the
/// profile's time zone (UTC if the profile's zone does not parse).
fn current_time_line(profile: &AgentProfile, now: DateTime<Utc>) -> String {
    let tz: Tz = profile.timezone.parse().unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz);
    // Models miscount from today's date to a weekday ("Friday, October 11"
    // for a Sunday), so spell the coming days out instead of leaving the
    // arithmetic to them.
    let upcoming = (0..UPCOMING_DAYS)
        .map(|i| {
            let day = local.date_naive() + Days::new(i);
            let label = match i {
                0 => " (today)",
                1 => " (tomorrow)",
                _ => "",
            };
            format!("- {}{}", day.format("%A, %B %-d"), label)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "\n\nThe current date and time is {} ({}). Use this for anything involving dates, days of the week, or scheduling.\n\
         Never work out the date of a day of the week yourself. Read it from this list of the coming days:\n{}\n",
        local.format("%A, %B %-d, %Y, %-I:%M %p"),
        tz.name(),
        upcoming
    )
}

/// Placeholder for the agent's own name in every profile prompt field.
pub const AGENT_NAME_PLACEHOLDER: &str = "{agent_name}";

/// The shared opening of both system prompts: the base prompt with the agent
/// name filled in, then the clock and the spoken-reply rules. When the profile
/// has a name but the base prompt never mentions the placeholder, an identity
/// line is prepended so the setting still takes effect.
fn prompt_preamble(profile: &AgentProfile) -> String {
    let mut prompt = String::new();
    if !profile.agent_name.is_empty() && !profile.base_prompt.contains(AGENT_NAME_PLACEHOLDER) {
        prompt.push_str(&format!(
            "Your name is {}. Introduce yourself by that name.\n\n",
            profile.agent_name
        ));
    }
    prompt.push_str(&with_agent_name(profile, &profile.base_prompt));
    prompt.push_str(&current_time_line(profile, Utc::now()));
    prompt.push_str(END_CALL_PROMPT);
    prompt.push_str(SPOKEN_ONLY_PROMPT);
    prompt.push('\n');
    prompt
}

/// Fill `{agent_name}` in one prompt field.
pub fn with_agent_name(profile: &AgentProfile, text: &str) -> String {
    text.replace(AGENT_NAME_PLACEHOLDER, &profile.agent_name)
}

/// Build a dynamic system prompt based on caller identity and memories.
pub fn build_system_prompt(
    profile: &AgentProfile,
    caller_phone: &str,
    caller_name: Option<&str>,
    memories: &[db::Memory],
) -> String {
    let mut prompt = prompt_preamble(profile);

    match caller_name {
        Some(name) => {
            prompt.push_str(
                &with_agent_name(profile, &profile.returning_caller_prompt)
                    .replace("{phone}", caller_phone)
                    .replace("{name}", name),
            );

            if !memories.is_empty() {
                let memory_list: String = memories
                    .iter()
                    .map(|m| format!("- {}", m.content))
                    .collect::<Vec<_>>()
                    .join("\n");
                prompt.push_str(
                    &with_agent_name(profile, &profile.memory_prompt)
                        .replace("{name}", name)
                        .replace("{memories}", &memory_list),
                );
            }
        }
        None => {
            prompt.push_str(
                &with_agent_name(profile, &profile.new_caller_prompt)
                    .replace("{phone}", caller_phone),
            );
        }
    }

    prompt
}

/// Build a system prompt for outbound calls with an assignment objective.
/// Uses the profile's `outbound_prompt` field with placeholders: {name}, {phone}, {objective},
/// {agent_name}.
pub fn build_outbound_prompt(
    profile: &AgentProfile,
    objective: &str,
    contact_name: Option<&str>,
    contact_phone: &str,
) -> String {
    let mut prompt = prompt_preamble(profile);
    let name_str = contact_name.unwrap_or("the person");

    prompt.push_str(
        &with_agent_name(profile, &profile.outbound_prompt)
            .replace("{name}", name_str)
            .replace("{phone}", contact_phone)
            .replace("{objective}", objective),
    );

    prompt
}

/// One conversational turn using the agent profile's model and token budget.
/// Returns text only — use [`respond_with_tools`] when tool use is needed.
pub async fn respond(
    llm: &dyn LlmProvider,
    system_prompt: &str,
    conversation: &[Message],
    profile: &AgentProfile,
) -> Result<String, LlmError> {
    let completion = respond_with_tools(llm, system_prompt, conversation, profile, &[]).await?;
    Ok(completion.text)
}

/// One conversational turn with tools available. Returns both text and any
/// tool calls so the caller can execute them and follow up.
pub async fn respond_with_tools(
    llm: &dyn LlmProvider,
    system_prompt: &str,
    conversation: &[Message],
    profile: &AgentProfile,
    tools: &[Tool],
) -> Result<Completion, LlmError> {
    llm.complete(CompletionRequest {
        system: system_prompt,
        messages: conversation,
        model: Some(&profile.model),
        max_tokens: profile.max_tokens,
        tools,
    })
    .await
}

/// Small single-shot task on the provider's default model.
async fn utility(
    llm: &dyn LlmProvider,
    system: &str,
    user_text: String,
    max_tokens: u32,
) -> Result<String, LlmError> {
    let messages = [Message::user_text(user_text)];
    let completion = llm
        .complete(CompletionRequest {
            system,
            messages: &messages,
            model: None,
            max_tokens,
            tools: &[],
        })
        .await?;
    Ok(completion.text)
}

/// Extract the caller's name from a transcript snippet. `agent_name` is the
/// assistant's own name, so a caller opening with "Hi Susan" is not filed as
/// Susan.
pub async fn extract_name(
    llm: &dyn LlmProvider,
    transcript: &str,
    agent_name: &str,
) -> Result<Option<String>, LlmError> {
    let system = extract_name_prompt(agent_name);

    let name = utility(llm, &system, transcript.to_string(), 50).await?;
    let name = name.trim().to_string();

    if name == "UNKNOWN" || name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name))
    }
}

fn extract_name_prompt(agent_name: &str) -> String {
    let mut system = "Extract the caller's name from the following transcript. \
        Return ONLY the name (first name, or first and last name if given). \
        If no name is mentioned, return exactly the word UNKNOWN."
        .to_string();
    if !agent_name.is_empty() {
        system.push_str(&format!(
            " The caller is speaking to an assistant named {agent_name}; that is not the \
             caller's name, so never return it."
        ));
    }
    system
}

/// Generate a summary of a completed call.
pub async fn generate_summary(
    llm: &dyn LlmProvider,
    conversation: &[Message],
) -> Result<String, LlmError> {
    let system = "Summarize the following phone conversation in 1-2 concise sentences. \
        Focus on the key topics discussed and any action items or decisions made.";

    utility(llm, system, format_transcript(conversation), 200).await
}

fn format_transcript(conversation: &[Message]) -> String {
    conversation
        .iter()
        .filter_map(|m| m.text().map(|t| format!("{}: {}", m.role(), t)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Result of bot-call classification.
#[derive(Debug, Clone)]
pub enum BotClassification {
    /// Skip — caller is human, or a bot we can't identify.
    Skip,
    /// Belongs to an existing group (matched by id).
    MatchExisting(String),
    /// New bot group with the given canonical name.
    NewGroup(String),
}

/// Classify a completed call as human or a recognizable bot. If bot, either match
/// against an existing group or propose a new group name.
///
/// `existing_groups` is `(group_id, canonical_name)` — keep small to fit context.
pub async fn classify_bot_call(
    llm: &dyn LlmProvider,
    conversation: &[Message],
    existing_groups: &[(String, String)],
) -> Result<BotClassification, LlmError> {
    let groups_block = if existing_groups.is_empty() {
        "(none)".to_string()
    } else {
        existing_groups
            .iter()
            .map(|(id, name)| format!("- id={id} name={name}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let system = "You classify completed phone conversations recorded by an AI assistant. \
        Decide whether the CALLER (not the assistant) is a human or an automated/robotic caller \
        — IVR, scripted bot, voice agent, robocaller, telemarketing recording, etc. \
        \n\nRules:\n\
        - If the caller is a real human, respond exactly: HUMAN\n\
        - If the caller is automated but you cannot extract a clear self-identification \
          (company or service they represent), respond exactly: UNKNOWN_BOT\n\
        - If the caller is automated AND you can identify them: first check whether they \
          match any existing group from the list below. If they do, respond: MATCH <group_id>\n\
        - If they are automated, identifiable, but match no existing group, respond: \
          NEW <Canonical Short Name>  (e.g. \"Verizon\", \"Google Verification\", \"Comcast\"). \
          Use a short, generic, brand-level label — not a person's name and not a campaign \
          description.\n\
        \nRespond with ONLY one of those four forms. No prose, no quotes.";

    let prompt = format!(
        "Existing bot groups:\n{groups_block}\n\nConversation transcript:\n{}",
        format_transcript(conversation)
    );

    let response = utility(llm, system, prompt, 60).await?;

    let raw = response.trim();
    if raw.eq_ignore_ascii_case("HUMAN") || raw.eq_ignore_ascii_case("UNKNOWN_BOT") {
        return Ok(BotClassification::Skip);
    }
    if let Some(rest) = raw.strip_prefix("MATCH ") {
        return Ok(BotClassification::MatchExisting(rest.trim().to_string()));
    }
    if let Some(rest) = raw.strip_prefix("NEW ") {
        let name = rest.trim().trim_matches('"').to_string();
        if name.is_empty() {
            return Ok(BotClassification::Skip);
        }
        return Ok(BotClassification::NewGroup(name));
    }

    tracing::warn!("Unparseable bot classification response: {raw:?}");
    Ok(BotClassification::Skip)
}

/// Extract memorable facts from a conversation.
pub async fn extract_memories(
    llm: &dyn LlmProvider,
    conversation: &[Message],
) -> Result<Vec<String>, LlmError> {
    let system = "Extract 3-5 key facts worth remembering from this phone conversation. \
        Focus on personal details, preferences, appointments, commitments, or anything \
        that would be useful to recall in future conversations. \
        Return each fact on its own line, with no bullet points or numbering. \
        If there are no memorable facts, return NONE.";

    let response = utility(llm, system, format_transcript(conversation), 300).await?;

    if response.trim() == "NONE" {
        return Ok(Vec::new());
    }

    let facts: Vec<String> = response
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(facts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn profile(timezone: &str) -> AgentProfile {
        serde_json::from_value(serde_json::json!({
            "base_prompt": "You are Steve.",
            "new_caller_prompt": " New caller {phone}.",
            "returning_caller_prompt": " Returning {name} {phone}.",
            "memory_prompt": " Memories for {name}: {memories}",
            "model": "test-model",
            "max_tokens": 100,
            "timezone": timezone
        }))
        .expect("valid profile")
    }

    #[test]
    fn current_time_line_uses_profile_timezone() {
        let now = Utc.with_ymd_and_hms(2026, 9, 17, 1, 5, 0).unwrap();
        let line = current_time_line(&profile("America/Los_Angeles"), now);
        assert!(
            line.contains("Wednesday, September 16, 2026, 6:05 PM (America/Los_Angeles)"),
            "{line}"
        );
    }

    #[test]
    fn current_time_line_falls_back_to_utc() {
        let now = Utc.with_ymd_and_hms(2026, 9, 17, 1, 5, 0).unwrap();
        let line = current_time_line(&profile("Not/AZone"), now);
        assert!(
            line.contains("Thursday, September 17, 2026, 1:05 AM (UTC)"),
            "{line}"
        );
    }

    #[test]
    fn current_time_line_lists_the_coming_days() {
        let now = Utc.with_ymd_and_hms(2026, 10, 5, 20, 50, 0).unwrap();
        let line = current_time_line(&profile("America/Los_Angeles"), now);
        assert!(line.contains("- Monday, October 5 (today)\n"), "{line}");
        assert!(line.contains("- Tuesday, October 6 (tomorrow)\n"), "{line}");
        assert!(line.contains("- Friday, October 9\n"), "{line}");
        assert!(line.contains("- Sunday, October 11\n"), "{line}");
        assert!(line.contains("- Sunday, October 18\n"), "{line}");
        assert!(!line.contains("October 19"), "{line}");
    }

    #[test]
    fn prompts_include_the_date_line() {
        let p = profile("UTC");
        let inbound = build_system_prompt(&p, "+15551234567", None, &[]);
        let outbound = build_outbound_prompt(&p, "say hi", Some("Ann"), "+15551234567");
        assert!(inbound.starts_with("You are Steve.\n\nThe current date and time is "));
        assert!(outbound.starts_with("You are Steve.\n\nThe current date and time is "));
        assert!(inbound.ends_with(" New caller +15551234567."));
        assert!(inbound.contains(SPOKEN_ONLY_PROMPT));
        assert!(outbound.contains(SPOKEN_ONLY_PROMPT));
    }

    fn named_profile(agent_name: &str, base_prompt: &str) -> AgentProfile {
        let mut p = profile("UTC");
        p.agent_name = agent_name.to_string();
        p.base_prompt = base_prompt.to_string();
        p.new_caller_prompt = " Say {agent_name} is speaking to {phone}.".to_string();
        p.returning_caller_prompt = " {agent_name} greets {name}.".to_string();
        p.memory_prompt = " {agent_name} recalls: {memories}".to_string();
        p.outbound_prompt = " {agent_name} calls {name} about {objective}.".to_string();
        p
    }

    #[test]
    fn agent_name_fills_the_placeholder_in_every_prompt_field() {
        let p = named_profile("Susan", "You are {agent_name}, an assistant.");
        let memories = vec![db::Memory {
            caller_phone: "+1".into(),
            call_sid: "CA1".into(),
            content: "likes tea".into(),
            created_at: Utc::now(),
            active: true,
        }];

        let new_caller = build_system_prompt(&p, "+15551234567", None, &[]);
        assert!(
            new_caller.starts_with("You are Susan, an assistant.\n\n"),
            "{new_caller}"
        );
        assert!(new_caller.ends_with(" Say Susan is speaking to +15551234567."));
        assert!(!new_caller.contains("Your name is"), "{new_caller}");

        let returning = build_system_prompt(&p, "+15551234567", Some("Ann"), &memories);
        assert!(
            returning.ends_with(" Susan greets Ann. Susan recalls: - likes tea"),
            "{returning}"
        );

        let outbound = build_outbound_prompt(&p, "the invoice", Some("Ann"), "+15551234567");
        assert!(
            outbound.starts_with("You are Susan, an assistant.\n\n"),
            "{outbound}"
        );
        assert!(
            outbound.ends_with(" Susan calls Ann about the invoice."),
            "{outbound}"
        );
        assert!(!new_caller.contains("{agent_name}") && !outbound.contains("{agent_name}"));
    }

    #[test]
    fn a_base_prompt_without_the_placeholder_gets_an_identity_line() {
        let p = named_profile("Susan", "You are a helpful assistant.");
        let inbound = build_system_prompt(&p, "+15551234567", None, &[]);
        let outbound = build_outbound_prompt(&p, "say hi", None, "+15551234567");
        let expected = "Your name is Susan. Introduce yourself by that name.\n\nYou are a helpful assistant.\n\n";
        assert!(inbound.starts_with(expected), "{inbound}");
        assert!(outbound.starts_with(expected), "{outbound}");
    }

    #[test]
    fn an_empty_agent_name_changes_nothing() {
        let p = named_profile("", "You are Steve.");
        let inbound = build_system_prompt(&p, "+15551234567", None, &[]);
        assert!(inbound.starts_with("You are Steve.\n\n"), "{inbound}");
        assert!(!inbound.contains("Your name is"), "{inbound}");
        // The placeholder simply renders empty when no name is set.
        assert!(
            inbound.ends_with(" Say  is speaking to +15551234567."),
            "{inbound}"
        );
    }

    #[test]
    fn name_extraction_prompt_rules_out_the_agent_name() {
        assert!(!extract_name_prompt("").contains("assistant named"));
        let with_name = extract_name_prompt("Susan");
        assert!(with_name.contains("assistant named Susan"), "{with_name}");
        assert!(with_name.contains("never return it"), "{with_name}");
    }
}
