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

#[cfg(test)]
mod tests {
    use super::*;

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
