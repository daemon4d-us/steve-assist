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
pub const END_CALL_PROMPT: &str = "To end a call you must call the end_call tool; saying goodbye by itself does not hang up, and never write the tool name in your spoken text. Call it as soon as the conversation is over (the caller says goodbye, asks you to hang up, or the purpose of the call is complete), then say one brief goodbye. Never keep a call open once it is finished.";

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

/// Signs that the model ended the call in words instead of calling the tool
/// (seen with Nemotron: "Now ending the call.", "(end_call) {}"). Treated as
/// a hang-up request so the call cannot linger.
pub fn text_signals_hangup(text: &str) -> bool {
    let t = text.to_lowercase();
    [
        "end_call",
        "end-call",
        "ending the call",
        "ending call",
        "call ending",
        "hanging up now",
        "hangin' up now",
        "hang up now",
        "i'll hang up",
        "call ended",
        "call closed",
        "call completed",
    ]
    .iter()
    .any(|p| t.contains(p))
}

/// Remove pseudo tool-call markup the model may have put in its spoken text,
/// e.g. "(end_call) {}", "[I'll trigger end-call.]", "{CALL END}", so TTS does
/// not read it out.
pub fn strip_hangup_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if matches!(c, '(' | '[' | '{') {
            // Drop a short bracketed span if it mentions the tool or a hang-up.
            let close = match c {
                '(' => ')',
                '[' => ']',
                _ => '}',
            };
            let span: String = chars.clone().take_while(|&x| x != close).collect();
            if span.len() < 60 && text_signals_hangup(&span) {
                for _ in 0..span.chars().count() {
                    chars.next();
                }
                chars.next(); // the closing bracket
                continue;
            }
        }
        out.push(c);
    }
    // Leftover empty brace pairs, bare tool names, and the whitespace they leave.
    let cleaned = out.replace("{}", "").replace("()", "");
    cleaned
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty() && !matches!(l.as_str(), "{" | "}" | "end_call" | "(end_call)"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A turn that is (mostly) a goodbye. Two of these in a row without the
/// call ending means both parties are done; hang up.
pub fn is_farewell(text: &str) -> bool {
    let t = text.to_lowercase();
    let short = t.split_whitespace().count() <= 25;
    let words: Vec<&str> = t
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .collect();
    let has_bye = words
        .iter()
        .any(|w| matches!(*w, "bye" | "goodbye" | "goodnight"))
        || t.contains("bye bye")
        || t.contains("talk soon")
        || t.contains("talk later")
        || t.contains("take care");
    short && has_bye
}

/// Upper bound on a single call, from `MAX_CALL_SECONDS` (default 15 min):
/// a safety net against runaway conversations (e.g. two assistants trading
/// goodbyes) that would otherwise run until Twilio's four-hour limit.
pub fn max_call_duration() -> Duration {
    std::env::var("MAX_CALL_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(15 * 60))
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
    fn text_fallbacks_detect_verbal_hangups() {
        assert!(text_signals_hangup("Alright—hanging up now. Take care!"));
        assert!(text_signals_hangup(
            "Okay, thanks! Bye. Now ending the call."
        ));
        assert!(text_signals_hangup(
            "TILL NEXT TIME—GOODBYE. CALL ENDING NOW. (END_CALL) {}"
        ));
        assert!(!text_signals_hangup(
            "Sure! Did you know octopuses have three hearts?"
        ));
        assert!(!text_signals_hangup("Talk soon! Bye."));
    }

    #[test]
    fn markers_are_stripped_from_spoken_text() {
        assert_eq!(strip_hangup_markers("Goodbye! (end_call) {}"), "Goodbye!");
        assert_eq!(
            strip_hangup_markers("Bye for now! [I'll trigger end-call.]"),
            "Bye for now!"
        );
        assert_eq!(
            strip_hangup_markers("Nothing else needed.\n{}\n(end_call){}"),
            "Nothing else needed."
        );
        assert_eq!(
            strip_hangup_markers("See you (at 5pm) tomorrow"),
            "See you (at 5pm) tomorrow"
        );
    }

    #[test]
    fn farewell_detection() {
        assert!(is_farewell("Talk soon! Bye."));
        assert!(is_farewell(
            "No problem! Have a great rest of your day. Talk later."
        ));
        assert!(is_farewell("Goodbye and good night!"));
        assert!(!is_farewell(
            "Sure! Did you know octopuses have three hearts?"
        ));
        assert!(!is_farewell(
            "Before you go, can you confirm the meeting is Tuesday at 3? I want to be sure I book the right slot for you and send the invite to the right address, bye for now"
        ));
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
