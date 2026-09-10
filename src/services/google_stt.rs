use googleapis_tonic_google_cloud_speech_v1::google::cloud::speech::v1::{
    RecognitionConfig, StreamingRecognitionConfig, StreamingRecognizeRequest,
    StreamingRecognizeResponse, recognition_config::AudioEncoding, speech_client::SpeechClient,
};
use tokio::sync::mpsc;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig};

/// Start a Speech-to-Text streaming session.
///
/// `audio_rx` receives raw mu-law audio bytes from Twilio.
/// Final transcripts are sent to `transcript_tx`.
/// This function spawns background tasks and returns once the gRPC stream is established.
pub async fn start_session(
    mut audio_rx: mpsc::Receiver<Vec<u8>>,
    transcript_tx: mpsc::Sender<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Authenticate — auto-detects credentials file or Workload Identity metadata
    let scopes = &["https://www.googleapis.com/auth/cloud-platform"];
    let provider = gcp_auth::provider().await?;
    let token_str = provider.token(scopes).await?.as_str().to_string();

    tracing::info!("GCP auth token obtained");

    // Connect to Speech API with TLS
    let tls_config = ClientTlsConfig::new()
        .domain_name("speech.googleapis.com")
        .with_native_roots();
    let channel = Channel::from_static("https://speech.googleapis.com")
        .tls_config(tls_config)?
        .connect()
        .await?;

    tracing::info!("Connected to Speech API");

    let mut client = SpeechClient::with_interceptor(channel, move |mut req: tonic::Request<()>| {
        let val = MetadataValue::try_from(&format!("Bearer {token_str}"))
            .map_err(|_| tonic::Status::unauthenticated("invalid token"))?;
        req.metadata_mut().insert("authorization", val);
        Ok(req)
    });

    // Build request stream: config first, then audio chunks directly from the receiver
    let config_msg = StreamingRecognizeRequest {
        streaming_request: Some(
            googleapis_tonic_google_cloud_speech_v1::google::cloud::speech::v1::streaming_recognize_request::StreamingRequest::StreamingConfig(
                StreamingRecognitionConfig {
                    config: Some(RecognitionConfig {
                        encoding: AudioEncoding::Mulaw.into(),
                        sample_rate_hertz: 8000,
                        language_code: "en-US".to_string(),
                        audio_channel_count: 1,
                        ..Default::default()
                    }),
                    interim_results: false,
                    ..Default::default()
                },
            ),
        ),
    };

    let request_stream = async_stream::stream! {
        yield config_msg;
        while let Some(audio_data) = audio_rx.recv().await {
            yield StreamingRecognizeRequest {
                streaming_request: Some(
                    googleapis_tonic_google_cloud_speech_v1::google::cloud::speech::v1::streaming_recognize_request::StreamingRequest::AudioContent(
                        audio_data,
                    ),
                ),
            };
        }
        tracing::info!("Audio stream ended");
    };

    // Start the streaming recognize call
    let response = client.streaming_recognize(request_stream).await?;
    let mut response_stream = response.into_inner();

    tracing::info!("Speech streaming session started");

    // Spawn task to process responses
    tokio::spawn(async move {
        loop {
            match response_stream.message().await {
                Ok(Some(resp)) => {
                    process_response(resp, &transcript_tx).await;
                }
                Ok(None) => {
                    tracing::info!("Speech response stream ended");
                    break;
                }
                Err(e) => {
                    tracing::error!("Speech response stream error: {e:?}");
                    break;
                }
            }
        }
    });

    Ok(())
}

async fn process_response(resp: StreamingRecognizeResponse, result_tx: &mpsc::Sender<String>) {
    for result in resp.results {
        if !result.is_final {
            continue;
        }

        if let Some(alt) = result.alternatives.first() {
            let transcript = alt.transcript.trim().to_string();
            if !transcript.is_empty() {
                tracing::info!("Transcript: {transcript}");
                if result_tx.send(transcript).await.is_err() {
                    return;
                }
            }
        }
    }
}
