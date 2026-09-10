use tokio::sync::mpsc;

use crate::services::{deepgram, google_stt};
use crate::state::AppState;

pub async fn start_session(
    state: &AppState,
    audio_rx: mpsc::Receiver<Vec<u8>>,
    transcript_tx: mpsc::Sender<String>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref api_key) = state.deepgram_api_key {
        tracing::info!("Using Deepgram STT");
        deepgram::start_session(api_key, audio_rx, transcript_tx).await
    } else {
        tracing::info!("Using Google STT");
        google_stt::start_session(audio_rx, transcript_tx).await
    }
}
