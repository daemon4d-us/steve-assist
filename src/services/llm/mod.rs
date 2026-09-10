//! Provider-neutral LLM abstraction.
//!
//! Everything above this module (conversation loop, tool loop, end-of-call
//! tasks) speaks only in terms of [`Message`], [`Tool`], [`ToolCall`] and
//! [`Completion`]. Each provider translates those to and from its own wire
//! format. Add a new cloud by implementing [`LlmProvider`] and registering it
//! in [`from_env`].

pub mod claude;
pub mod openai_compat;

use std::env;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type LlmError = Box<dyn std::error::Error + Send + Sync>;

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// The result of executing a [`ToolCall`], to be fed back to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    /// JSON-encoded tool output.
    pub content: String,
}

/// One turn of conversation history, independent of any provider's wire format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    User(String),
    Assistant(String),
    /// Assistant turn that requested one or more tool calls (optionally with text).
    ToolCalls {
        text: String,
        calls: Vec<ToolCall>,
    },
    /// Results for the tool calls of the preceding [`Message::ToolCalls`] turn.
    ToolResults(Vec<ToolResult>),
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Message::User(text.into())
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Message::Assistant(text.into())
    }

    /// Plain spoken text of this turn, if it has any. Tool traffic yields `None`.
    pub fn text(&self) -> Option<&str> {
        match self {
            Message::User(t) | Message::Assistant(t) => Some(t),
            Message::ToolCalls { .. } | Message::ToolResults(_) => None,
        }
    }

    pub fn role(&self) -> &'static str {
        match self {
            Message::User(_) | Message::ToolResults(_) => "user",
            Message::Assistant(_) | Message::ToolCalls { .. } => "assistant",
        }
    }
}

/// A tool the model may call. `input_schema` is a JSON Schema object.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A single completion request.
pub struct CompletionRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    /// Provider-specific model id. `None` or empty means the provider's default.
    pub model: Option<&'a str>,
    pub max_tokens: u32,
    pub tools: &'a [Tool],
}

/// Token accounting for one completion, when the provider reports it.
#[derive(Debug, Default, Clone, Copy)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// What the model produced: spoken text and/or tool calls to execute.
#[derive(Debug, Default)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Short identifier for logs (e.g. "claude", "nemotron").
    fn name(&self) -> &'static str;

    /// Model used when a request does not name one.
    fn default_model(&self) -> &str;

    async fn complete(&self, req: CompletionRequest<'_>) -> Result<Completion, LlmError>;
}

/// Resolve the model for a request: the explicit one if non-empty, else the default.
pub(crate) fn resolve_model<'a>(requested: Option<&'a str>, default: &'a str) -> &'a str {
    match requested {
        Some(m) if !m.trim().is_empty() => m,
        _ => default,
    }
}

/// Build the provider selected by `LLM_PROVIDER` (default `claude`).
///
/// - `claude`: Anthropic Messages API. Needs `ANTHROPIC_API_KEY`.
/// - `nemotron`: NVIDIA Nemotron via Nebius Token Factory (OpenAI-compatible).
///   Needs `NEBIUS_API_KEY`; `LLM_BASE_URL` overrides the endpoint, which makes
///   the same implementation work for any OpenAI-compatible cloud.
///
/// `LLM_MODEL` overrides the provider's default model. `LLM_THINKING=on` re-enables
/// chain-of-thought on reasoning models (off by default: it eats the small
/// `max_tokens` budgets spoken replies use and adds seconds of latency).
pub fn from_env() -> Arc<dyn LlmProvider> {
    let provider = env::var("LLM_PROVIDER").unwrap_or_else(|_| "claude".to_string());
    let model_override = env::var("LLM_MODEL").ok().filter(|m| !m.trim().is_empty());

    match provider.trim().to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => {
            let api_key = env::var("ANTHROPIC_API_KEY")
                .expect("ANTHROPIC_API_KEY must be set when LLM_PROVIDER=claude");
            let model = model_override.unwrap_or_else(|| claude::DEFAULT_MODEL.to_string());
            Arc::new(claude::ClaudeProvider::new(api_key, model))
        }
        "nemotron" | "nebius" | "token-factory" => {
            let api_key = env::var("NEBIUS_API_KEY")
                .expect("NEBIUS_API_KEY must be set when LLM_PROVIDER=nemotron");
            let base_url = env::var("LLM_BASE_URL")
                .ok()
                .filter(|u| !u.trim().is_empty())
                .unwrap_or_else(|| openai_compat::TOKEN_FACTORY_BASE_URL.to_string());
            let model =
                model_override.unwrap_or_else(|| openai_compat::DEFAULT_NEMOTRON_MODEL.to_string());
            let thinking = env::var("LLM_THINKING")
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "on" | "true" | "1"))
                .unwrap_or(false);
            Arc::new(
                openai_compat::OpenAiCompatProvider::new("nemotron", base_url, api_key, model)
                    .with_thinking(thinking),
            )
        }
        other => panic!("Unknown LLM_PROVIDER '{other}'. Use 'claude' or 'nemotron'."),
    }
}

/// Shared live-test scenario: a plain reply, then a forced tool call whose
/// result the model must read back. Used by each provider's `#[ignore]`d test.
#[cfg(test)]
pub(crate) async fn live_tool_round_trip(provider: &dyn LlmProvider) {
    use serde_json::json;

    let plain = provider
        .complete(CompletionRequest {
            system: "You are a phone assistant. Answer in one short sentence.",
            messages: &[Message::user_text("Say hello to the caller.")],
            model: None,
            max_tokens: 60,
            tools: &[],
        })
        .await
        .expect("plain completion");
    assert!(!plain.text.is_empty(), "empty plain reply");
    assert!(plain.tool_calls.is_empty());
    eprintln!("[{}] plain: {}", provider.name(), plain.text);

    let tools = [Tool {
        name: "lookup_weather".into(),
        description:
            "Get the current weather for a city. Always call this when asked about weather.".into(),
        input_schema: json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"],
            "additionalProperties": false
        }),
    }];
    let mut history = vec![Message::user_text(
        "What's the weather in Amsterdam right now? Use the tool.",
    )];
    let first = provider
        .complete(CompletionRequest {
            system: "You are a helpful assistant with tools.",
            messages: &history,
            model: None,
            max_tokens: 200,
            tools: &tools,
        })
        .await
        .expect("tool completion");
    assert_eq!(
        first.tool_calls.len(),
        1,
        "expected exactly one tool call, got {first:?}"
    );
    let call = &first.tool_calls[0];
    assert_eq!(call.name, "lookup_weather");
    assert_eq!(
        call.input["city"]
            .as_str()
            .map(str::to_lowercase)
            .as_deref(),
        Some("amsterdam")
    );
    eprintln!(
        "[{}] tool call: {} {}",
        provider.name(),
        call.name,
        call.input
    );

    history.push(Message::ToolCalls {
        text: first.text,
        calls: first.tool_calls.clone(),
    });
    history.push(Message::ToolResults(vec![ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: json!({"temp_c": 17, "sky": "overcast"}).to_string(),
    }]));
    let second = provider
        .complete(CompletionRequest {
            system: "You are a helpful assistant with tools.",
            messages: &history,
            model: None,
            max_tokens: 200,
            tools: &tools,
        })
        .await
        .expect("follow-up completion");
    assert!(
        second.tool_calls.is_empty(),
        "unexpected second tool call: {second:?}"
    );
    assert!(
        second.text.contains("17"),
        "reply did not use tool result: {}",
        second.text
    );
    eprintln!("[{}] final: {}", provider.name(), second.text);
}
