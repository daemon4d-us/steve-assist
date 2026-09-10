//! OpenAI-compatible chat-completions provider.
//!
//! Used for NVIDIA Nemotron served through Nebius Token Factory, but the wire
//! format is the de-facto standard, so pointing `base_url` at any other
//! OpenAI-compatible endpoint works unchanged.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{Value, json};

use super::{Completion, CompletionRequest, LlmError, LlmProvider, Message, Tool, ToolCall};

pub const TOKEN_FACTORY_BASE_URL: &str = "https://api.tokenfactory.nebius.com/v1";
pub const DEFAULT_NEMOTRON_MODEL: &str = "nvidia/nemotron-3-super-120b-a12b";

pub struct OpenAiCompatProvider {
    name: &'static str,
    base_url: String,
    api_key: String,
    default_model: String,
    /// When false, ask the server to skip chain-of-thought generation.
    /// Reasoning models (Nemotron 3) otherwise spend the whole `max_tokens`
    /// budget on `reasoning_content` and return an empty answer.
    thinking: bool,
    client: Client,
}

impl OpenAiCompatProvider {
    pub fn new(
        name: &'static str,
        base_url: String,
        api_key: String,
        default_model: String,
    ) -> Self {
        Self {
            name,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            default_model,
            thinking: true,
            client: Client::new(),
        }
    }

    /// Enable or disable model reasoning. Sent as vLLM-style
    /// `chat_template_kwargs.enable_thinking`, which Nebius Token Factory honors.
    pub fn with_thinking(mut self, enabled: bool) -> Self {
        self.thinking = enabled;
        self
    }

    fn build_body(&self, req: &CompletionRequest<'_>) -> Value {
        let model = super::resolve_model(req.model, &self.default_model);
        let mut body = json!({
            "model": model,
            "max_tokens": req.max_tokens,
            "messages": to_wire(req.system, req.messages),
        });
        if !req.tools.is_empty() {
            body["tools"] = Value::Array(tools_to_wire(req.tools));
            body["tool_choice"] = json!("auto");
        }
        if !self.thinking {
            body["chat_template_kwargs"] = json!({ "enable_thinking": false });
        }
        body
    }
}

/// Translate neutral history into OpenAI chat messages, with the system prompt first.
fn to_wire(system: &str, messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::with_capacity(messages.len() + 1);
    if !system.is_empty() {
        out.push(json!({ "role": "system", "content": system }));
    }
    for m in messages {
        match m {
            Message::User(t) => out.push(json!({ "role": "user", "content": t })),
            Message::Assistant(t) => out.push(json!({ "role": "assistant", "content": t })),
            Message::ToolCalls { text, calls } => {
                let tool_calls: Vec<Value> = calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": {
                                "name": c.name,
                                "arguments": c.input.to_string(),
                            },
                        })
                    })
                    .collect();
                let content = if text.is_empty() {
                    Value::Null
                } else {
                    json!(text)
                };
                out.push(json!({
                    "role": "assistant",
                    "content": content,
                    "tool_calls": tool_calls,
                }));
            }
            Message::ToolResults(results) => {
                for r in results {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": r.call_id,
                        "name": r.name,
                        "content": r.content,
                    }));
                }
            }
        }
    }
    out
}

fn tools_to_wire(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                },
            })
        })
        .collect()
}

/// Nemotron and other reasoning models may inline their chain of thought as
/// `<think>...</think>`. That must never reach TTS, so strip it.
fn strip_thinking(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    async fn complete(&self, req: CompletionRequest<'_>) -> Result<Completion, LlmError> {
        let body = self.build_body(&req);

        let response = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(format!("{} API error {status}: {error_text}", self.name).into());
        }

        let resp: Value = response.json().await?;
        let message = resp
            .pointer("/choices/0/message")
            .ok_or_else(|| format!("{} response missing choices[0].message", self.name))?;

        let text = message
            .get("content")
            .and_then(|c| c.as_str())
            .map(strip_thinking)
            .unwrap_or_default();

        let mut tool_calls = Vec::new();
        if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
            for c in calls {
                let id = c
                    .get("id")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                let function = c.get("function").cloned().unwrap_or(Value::Null);
                let name = function
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                // `arguments` is a JSON-encoded string per the OpenAI spec, but some
                // servers return an object directly. Accept both.
                let input = match function.get("arguments") {
                    Some(Value::String(s)) if s.trim().is_empty() => json!({}),
                    Some(Value::String(s)) => serde_json::from_str(s)
                        .map_err(|e| format!("tool '{name}' arguments are not valid JSON: {e}"))?,
                    Some(v) => v.clone(),
                    None => json!({}),
                };
                tool_calls.push(ToolCall { id, name, input });
            }
        }

        let usage = resp.get("usage").map(|u| super::Usage {
            input_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            output_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
        });

        Ok(Completion {
            text,
            tool_calls,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_think_blocks() {
        assert_eq!(
            strip_thinking("<think>hmm</think>Hello there"),
            "Hello there"
        );
        assert_eq!(
            strip_thinking("Hello <think>a</think>world<think>b</think>!"),
            "Hello world!"
        );
        assert_eq!(strip_thinking("plain"), "plain");
        assert_eq!(strip_thinking("<think>unterminated"), "");
    }

    #[test]
    fn thinking_toggle_controls_chat_template_kwargs() {
        let req = |p: &OpenAiCompatProvider| {
            p.build_body(&CompletionRequest {
                system: "s",
                messages: &[Message::user_text("hi")],
                model: None,
                max_tokens: 10,
                tools: &[],
            })
        };
        let on = OpenAiCompatProvider::new("x", "http://h".into(), "k".into(), "m".into());
        assert!(req(&on).get("chat_template_kwargs").is_none());
        let off = OpenAiCompatProvider::new("x", "http://h".into(), "k".into(), "m".into())
            .with_thinking(false);
        assert_eq!(req(&off)["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(req(&off)["model"], "m");
    }

    #[test]
    fn tool_history_round_trips_to_openai_shape() {
        let history = vec![
            Message::user_text("book it"),
            Message::ToolCalls {
                text: String::new(),
                calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "book_meeting".into(),
                    input: json!({"start": "x"}),
                }],
            },
            Message::ToolResults(vec![super::super::ToolResult {
                call_id: "c1".into(),
                name: "book_meeting".into(),
                content: "{\"ok\":true}".into(),
            }]),
        ];
        let wire = to_wire("sys", &history);
        assert_eq!(wire.len(), 4);
        assert_eq!(wire[0]["role"], "system");
        assert_eq!(
            wire[2]["tool_calls"][0]["function"]["arguments"],
            "{\"start\":\"x\"}"
        );
        assert!(wire[2]["content"].is_null());
        assert_eq!(wire[3]["role"], "tool");
        assert_eq!(wire[3]["tool_call_id"], "c1");
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Live round trip: `cargo test nemotron_live -- --ignored` with NEBIUS_API_KEY in .env.
    #[tokio::test]
    #[ignore]
    async fn nemotron_live_tool_round_trip() {
        dotenvy::dotenv().ok();
        let key = std::env::var("NEBIUS_API_KEY").expect("NEBIUS_API_KEY");
        let base =
            std::env::var("LLM_BASE_URL").unwrap_or_else(|_| TOKEN_FACTORY_BASE_URL.to_string());
        let model =
            std::env::var("LLM_MODEL").unwrap_or_else(|_| DEFAULT_NEMOTRON_MODEL.to_string());
        let provider = OpenAiCompatProvider::new("nemotron", base, key, model).with_thinking(false);
        crate::services::llm::live_tool_round_trip(&provider).await;
    }
}
