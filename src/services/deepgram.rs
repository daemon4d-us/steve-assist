use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

/// Start a Deepgram streaming STT session.
///
/// `audio_rx` receives raw mu-law audio bytes from Twilio.
/// Final transcripts are sent to `transcript_tx`.
pub async fn start_session(
    api_key: &str,
    mut audio_rx: mpsc::Receiver<Vec<u8>>,
    transcript_tx: mpsc::Sender<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let url = "wss://api.deepgram.com/v1/listen?encoding=mulaw&sample_rate=8000&channels=1&endpointing=300&interim_results=false";

    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Token {api_key}"))?,
    );

    let (ws, _response) = tokio_tungstenite::connect_async(request).await?;
    let (mut ws_sink, mut ws_stream) = ws.split();

    tracing::info!("Connected to Deepgram STT");

    // Spawn task to forward audio to Deepgram
    tokio::spawn(async move {
        while let Some(audio_data) = audio_rx.recv().await {
            if ws_sink
                .send(tokio_tungstenite::tungstenite::Message::Binary(
                    audio_data.into(),
                ))
                .await
                .is_err()
            {
                tracing::warn!("Deepgram WebSocket send failed");
                break;
            }
        }
        // Send close frame to signal end of audio
        let _ = ws_sink.close().await;
        tracing::info!("Deepgram audio forwarding ended");
    });

    // Process Deepgram responses
    tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_stream.next().await {
            let tokio_tungstenite::tungstenite::Message::Text(text) = msg else {
                continue;
            };

            let Ok(resp) = serde_json::from_str::<DeepgramResponse>(&text) else {
                continue;
            };

            if !resp.is_final {
                continue;
            }

            let transcript = resp.transcript().trim().to_string();
            if transcript.is_empty() {
                continue;
            }

            tracing::info!("Transcript: {transcript}");
            if transcript_tx.send(transcript).await.is_err() {
                break;
            }
        }
        tracing::info!("Deepgram response stream ended");
    });

    Ok(())
}

#[derive(Debug, Deserialize)]
struct DeepgramResponse {
    channel: Channel,
    is_final: bool,
}

#[derive(Debug, Deserialize)]
struct Channel {
    alternatives: Vec<Alternative>,
}

#[derive(Debug, Deserialize)]
struct Alternative {
    transcript: String,
}

impl DeepgramResponse {
    fn transcript(&self) -> &str {
        self.channel
            .alternatives
            .first()
            .map(|a| a.transcript.as_str())
            .unwrap_or("")
    }
}
