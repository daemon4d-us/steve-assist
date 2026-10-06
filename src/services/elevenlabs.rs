use reqwest::Client;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct TtsRequest<'a> {
    text: &'a str,
    model_id: &'a str,
}

pub async fn text_to_speech(
    api_key: &str,
    voice_id: &str,
    text: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::new();

    let url =
        format!("https://api.elevenlabs.io/v1/text-to-speech/{voice_id}?output_format=ulaw_8000");

    let body = TtsRequest {
        text,
        model_id: "eleven_turbo_v2",
    };

    let response = client
        .post(&url)
        .header("xi-api-key", api_key)
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await?;
        return Err(format!("ElevenLabs API error {status}: {error_text}").into());
    }

    let bytes = response.bytes().await?;
    Ok(bytes.to_vec())
}

/// One entry of the account's voice library, as the dashboard's voice picker
/// shows it. Unknown fields from the API are ignored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Voice {
    pub voice_id: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    /// Free-form tags from ElevenLabs such as accent, gender, age, use case.
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub preview_url: Option<String>,
}

#[derive(Deserialize)]
struct VoicesResponse {
    voices: Vec<Voice>,
}

/// The voices available to the account, premade and cloned alike.
pub async fn list_voices(
    api_key: &str,
) -> Result<Vec<Voice>, Box<dyn std::error::Error + Send + Sync>> {
    let response = Client::new()
        .get("https://api.elevenlabs.io/v1/voices")
        .header("xi-api-key", api_key)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await?;
        return Err(format!("ElevenLabs API error {status}: {error_text}").into());
    }

    let parsed: VoicesResponse = response.json().await?;
    Ok(parsed.voices)
}

/// A one-line, user-facing reason for a failed ElevenLabs call. The API's
/// error body is JSON with a `detail.message` (or a plain `detail` string);
/// anything else is passed through as is.
pub fn describe_error(err: &(dyn std::error::Error + Send + Sync)) -> String {
    let text = err.to_string();
    let Some(json_start) = text.find('{') else {
        return text;
    };
    let message = serde_json::from_str::<serde_json::Value>(&text[json_start..])
        .ok()
        .and_then(|v| {
            let detail = v.get("detail")?;
            detail
                .get("message")
                .and_then(|m| m.as_str())
                .or_else(|| detail.as_str())
                .map(str::to_string)
        });
    message.unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_error_extracts_the_api_message() {
        let err: Box<dyn std::error::Error + Send + Sync> = format!(
            "ElevenLabs API error 401 Unauthorized: {}",
            r#"{"detail":{"type":"authentication_error","code":"unauthorized","message":"The API key you used is missing the permission voices_read to execute this operation.","status":"missing_permissions"}}"#
        )
        .into();
        assert_eq!(
            describe_error(err.as_ref()),
            "The API key you used is missing the permission voices_read to execute this operation."
        );
        let plain: Box<dyn std::error::Error + Send + Sync> = "connection refused".into();
        assert_eq!(describe_error(plain.as_ref()), "connection refused");
    }

    #[test]
    fn voice_list_parses_the_fields_the_picker_needs_and_ignores_the_rest() {
        let body = r#"{"voices":[
            {"voice_id":"21m00Tcm4TlvDq8ikWAM","name":"Rachel","samples":null,"category":"premade",
             "fine_tuning":{"is_allowed_to_fine_tune":false},
             "labels":{"accent":"american","gender":"female","age":"young","use_case":"narration"},
             "preview_url":"https://storage.googleapis.com/eleven-public-prod/premade/voices/rachel.mp3",
             "settings":null,"sharing":null},
            {"voice_id":"abc","name":"My Clone"}
        ]}"#;
        let parsed: VoicesResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.voices.len(), 2);
        let rachel = &parsed.voices[0];
        assert_eq!(rachel.name, "Rachel");
        assert_eq!(rachel.category, "premade");
        assert_eq!(rachel.labels["accent"], "american");
        assert!(
            rachel
                .preview_url
                .as_deref()
                .unwrap()
                .ends_with("rachel.mp3")
        );
        let clone = &parsed.voices[1];
        assert_eq!(clone.category, "");
        assert!(clone.labels.is_empty());
        assert_eq!(clone.preview_url, None);
    }
}
