use reqwest::Client;
use serde::Serialize;

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
