//! Steve's "brain": prompt composition and the LLM-backed tasks the call loop
//! needs. Everything here is provider-neutral and goes through [`LlmProvider`].

use crate::services::db::{self, AgentProfile};
use crate::services::llm::{Completion, CompletionRequest, LlmError, LlmProvider, Message, Tool};

/// Build a dynamic system prompt based on caller identity and memories.
pub fn build_system_prompt(
    profile: &AgentProfile,
    caller_phone: &str,
    caller_name: Option<&str>,
    memories: &[db::Memory],
) -> String {
    let mut prompt = profile.base_prompt.clone();

    match caller_name {
        Some(name) => {
            prompt.push_str(
                &profile
                    .returning_caller_prompt
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
                    &profile
                        .memory_prompt
                        .replace("{name}", name)
                        .replace("{memories}", &memory_list),
                );
            }
        }
        None => {
            prompt.push_str(&profile.new_caller_prompt.replace("{phone}", caller_phone));
        }
    }

    prompt
}

/// Build a system prompt for outbound calls with an assignment objective.
/// Uses the profile's `outbound_prompt` field with placeholders: {name}, {phone}, {objective}.
pub fn build_outbound_prompt(
    profile: &AgentProfile,
    objective: &str,
    contact_name: Option<&str>,
    contact_phone: &str,
) -> String {
    let mut prompt = profile.base_prompt.clone();
    let name_str = contact_name.unwrap_or("the person");

    prompt.push_str(
        &profile
            .outbound_prompt
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

/// Extract the caller's name from a transcript snippet.
pub async fn extract_name(
    llm: &dyn LlmProvider,
    transcript: &str,
) -> Result<Option<String>, LlmError> {
    let system = "Extract the person's name from the following transcript. \
        Return ONLY the name (first name, or first and last name if given). \
        If no name is mentioned, return exactly the word UNKNOWN.";

    let name = utility(llm, system, transcript.to_string(), 50).await?;
    let name = name.trim().to_string();

    if name == "UNKNOWN" || name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name))
    }
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
