//! Call control the model can exercise itself: the `end_call` tool.
//!
//! Without it Steve says goodbye and then sits on the line until the other
//! party hangs up (or Twilio's four-hour limit). With it, the model calls the
//! tool once the conversation is over, says one closing sentence, and the
//! stream handler ends the call after that sentence has played out.

use std::time::Duration;

use serde_json::{Value, json};

use crate::services::llm::Tool;

pub const END_CALL_TOOL: &str = "end_call";

/// Appended to the system prompt so the model knows the tool is the way to
/// finish a call.
pub const END_CALL_PROMPT: &str = "When the conversation is over (the caller says goodbye, asks you to hang up, or the purpose of the call is complete), call the end_call tool and then say a brief goodbye. Never keep a call open once it is finished.";

pub fn end_call_tool() -> Tool {
    Tool {
        name: END_CALL_TOOL.to_string(),
        description: "End the phone call. Call this when the conversation is finished: the other party said goodbye, asked you to hang up, or the call's purpose is complete. The call ends right after your next (final) message is spoken.".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "A few words on why the call is ending"
                }
            },
            "required": []
        }),
    }
}

/// Result handed back to the model after it requested the hang-up.
pub fn end_call_result() -> Value {
    json!({
        "status": "ending",
        "instruction": "The call will end right after your next message is spoken. Reply with one short closing sentence only."
    })
}

pub fn end_call_reason(input: &Value) -> String {
    input
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("not given")
        .to_string()
}

/// How long to let the farewell play before dropping the stream. The audio
/// is sent in a burst but plays in real time on the far end: μ-law at 8 kHz
/// is 8 bytes per millisecond, plus a margin for transport buffering.
pub fn playout_delay(audio_bytes: usize) -> Duration {
    Duration::from_millis((audio_bytes / 8) as u64 + 750)
}

/// True for real Twilio call sids. The Bluetooth bridge uses `BT…` ids and
/// ends its call itself when the stream closes.
pub fn is_twilio_call(call_sid: &str) -> bool {
    call_sid.starts_with("CA")
}

/// Ask Twilio to complete the call. Closing the media stream already ends a
/// `<Connect><Stream>` call, so this is belt and braces; errors are logged by
/// the caller.
pub async fn twilio_hangup(
    account_sid: &str,
    auth_token: &str,
    call_sid: &str,
) -> Result<(), String> {
    let url =
        format!("https://api.twilio.com/2010-04-01/Accounts/{account_sid}/Calls/{call_sid}.json");
    let resp = reqwest::Client::new()
        .post(&url)
        .basic_auth(account_sid, Some(auth_token))
        .form(&[("Status", "completed")])
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("Twilio returned {}", resp.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_is_well_formed() {
        let t = end_call_tool();
        assert_eq!(t.name, "end_call");
        assert_eq!(t.input_schema["type"], "object");
        assert!(t.input_schema["properties"]["reason"].is_object());
    }

    #[test]
    fn playout_delay_covers_the_audio_plus_margin() {
        // 3 seconds of μ-law = 24000 bytes
        assert_eq!(playout_delay(24000), Duration::from_millis(3750));
        assert_eq!(playout_delay(0), Duration::from_millis(750));
    }

    #[test]
    fn reason_and_call_kind() {
        assert_eq!(
            end_call_reason(&json!({"reason": "caller said bye"})),
            "caller said bye"
        );
        assert_eq!(end_call_reason(&json!({})), "not given");
        assert!(is_twilio_call("CA123"));
        assert!(!is_twilio_call("BT123"));
    }
}
