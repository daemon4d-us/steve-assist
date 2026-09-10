//! Anthropic Messages API provider.

use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};

use super::{Completion, CompletionRequest, LlmError, LlmProvider, Message, Tool, ToolCall};

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub struct ClaudeProvider {
    api_key: String,
    default_model: String,
    client: Client,
}

impl ClaudeProvider {
    pub fn new(api_key: String, default_model: String) -> Self {
        Self {
            api_key,
            default_model,
            client: Client::new(),
        }
    }
}

#[derive(Serialize)]
struct Request<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [Tool]>,
}

/// Translate neutral history into Anthropic content blocks.
fn to_wire(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| match m {
            Message::User(t) => json!({ "role": "user", "content": t }),
            Message::Assistant(t) => json!({ "role": "assistant", "content": t }),
            Message::ToolCalls { text, calls } => {
                let mut blocks = Vec::with_capacity(calls.len() + 1);
                if !text.is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for c in calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": c.input,
                    }));
                }
                json!({ "role": "assistant", "content": blocks })
            }
            Message::ToolResults(results) => {
                let blocks: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": r.call_id,
                            "content": r.content,
                        })
                    })
                    .collect();
                json!({ "role": "user", "content": blocks })
            }
        })
        .collect()
}

#[async_trait]
impl LlmProvider for ClaudeProvider {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn default_model(&self) -> &str {
        &self.default_model
    }

    async fn complete(&self, req: CompletionRequest<'_>) -> Result<Completion, LlmError> {
        let model = super::resolve_model(req.model, &self.default_model);

        let body = Request {
            model,
            max_tokens: req.max_tokens,
            system: req.system,
            messages: to_wire(req.messages),
            tools: if req.tools.is_empty() {
                None
            } else {
                Some(req.tools)
            },
        };

        let response = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(format!("Claude API error {status}: {error_text}").into());
        }

        let resp: Value = response.json().await?;
        let content = resp
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or("Claude response missing content array")?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(t);
                    }
                }
                Some("tool_use") => {
                    tool_calls.push(ToolCall {
                        id: str_field(block, "id"),
                        name: str_field(block, "name"),
                        input: block.get("input").cloned().unwrap_or(Value::Null),
                    });
                }
                _ => {}
            }
        }

        let usage = resp.get("usage").map(|u| super::Usage {
            input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        });

        Ok(Completion {
            text,
            tool_calls,
            usage,
        })
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::llm::ToolResult;

    #[test]
    fn tool_history_round_trips_to_anthropic_blocks() {
        let history = vec![
            Message::user_text("book it"),
            Message::ToolCalls {
                text: "Sure.".into(),
                calls: vec![ToolCall {
                    id: "toolu_1".into(),
                    name: "book_meeting".into(),
                    input: json!({"start": "x"}),
                }],
            },
            Message::ToolResults(vec![ToolResult {
                call_id: "toolu_1".into(),
                name: "book_meeting".into(),
                content: "{\"ok\":true}".into(),
            }]),
        ];
        let wire = to_wire(&history);
        assert_eq!(wire.len(), 3);
        assert_eq!(wire[1]["content"][0]["type"], "text");
        assert_eq!(wire[1]["content"][1]["type"], "tool_use");
        assert_eq!(wire[1]["content"][1]["input"]["start"], "x");
        assert_eq!(wire[2]["role"], "user");
        assert_eq!(wire[2]["content"][0]["tool_use_id"], "toolu_1");
    }

    /// Live round trip: `cargo test claude_live -- --ignored` with ANTHROPIC_API_KEY in .env.
    #[tokio::test]
    #[ignore]
    async fn claude_live_tool_round_trip() {
        dotenvy::dotenv().ok();
        let key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY");
        let provider = ClaudeProvider::new(key, DEFAULT_MODEL.to_string());
        crate::services::llm::live_tool_round_trip(&provider).await;
    }
}
