//! Simulated callers. A `Caller` places a call and speaks scripted lines,
//! measuring how long Steve takes to answer each one.
//!
//! `DirectWsCaller` talks Twilio's Media Streams protocol straight to the
//! server's `/media-stream` WebSocket: no phone network, free, CI-friendly.
//! A PSTN caller over a second Twilio number will implement the same trait.

use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::scenario::TurnMetrics;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// How Steve answered one spoken turn.
pub struct TurnOutcome {
    pub metrics: TurnMetrics,
}

/// A live call the harness can speak into and then finish.
#[allow(async_fn_in_trait)]
pub trait Caller {
    /// The Twilio call SID this call is running under.
    fn call_sid(&self) -> &str;
    /// Milliseconds from answer to Steve's first greeting audio.
    fn greeting_latency_ms(&self) -> u128;
    /// Speak one line and wait for Steve to finish replying.
    async fn say(&mut self, utterance_ulaw: &[u8], turn: u32) -> Result<TurnOutcome, BoxError>;
    /// Hang up.
    async fn hangup(self) -> Result<(), BoxError>;
}

/// Frame size for 20 ms of mu-law audio at 8 kHz.
const FRAME_BYTES: usize = 160;
/// mu-law byte for zero amplitude (G.711 silence).
const ULAW_SILENCE: u8 = 0xFF;
/// Trailing silence sent after each utterance so the server's speech
/// recognizer detects end-of-speech and finalizes the transcript. Real Twilio
/// streams silence continuously; without it Deepgram never endpoints.
const TRAILING_SILENCE_MS: usize = 500;
/// Silence gap that marks the end of Steve's turn.
const TURN_END_SILENCE: Duration = Duration::from_millis(800);
/// Hard cap on how long we wait for a single reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

pub struct DirectWsCaller {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    stream_sid: String,
    call_sid: String,
    greeting_latency_ms: u128,
}

impl DirectWsCaller {
    /// Connect, send the Twilio `connected` + `start` handshake, and wait for
    /// Steve's opening greeting audio.
    pub async fn connect(server_ws_url: &str, caller_phone: &str) -> Result<Self, BoxError> {
        let (mut ws, _) = connect_async(server_ws_url).await?;

        let stream_sid = format!("MZ{}", short_id());
        let call_sid = format!("CAe2e{}", short_id());

        ws.send(Message::Text(
            json!({"event": "connected", "protocol": "Call", "version": "1.0.0"})
                .to_string()
                .into(),
        ))
        .await?;
        ws.send(Message::Text(
            json!({
                "event": "start",
                "streamSid": stream_sid,
                "start": {
                    "callSid": call_sid,
                    "tracks": ["inbound"],
                    "customParameters": { "callerPhone": caller_phone }
                }
            })
            .to_string()
            .into(),
        ))
        .await?;

        // The greeting is generated as soon as the start event is processed;
        // measure from now to its first audio frame.
        let started = Instant::now();
        let audio = drain_response(&mut ws).await?;
        let greeting_latency_ms = started.elapsed().as_millis();
        let _ = audio;

        Ok(Self {
            ws,
            stream_sid,
            call_sid,
            greeting_latency_ms,
        })
    }
}

impl DirectWsCaller {
    async fn send_frame(&mut self, chunk: &[u8]) -> Result<(), BoxError> {
        self.ws
            .send(Message::Text(
                json!({
                    "event": "media",
                    "streamSid": self.stream_sid,
                    "media": { "payload": STANDARD.encode(chunk) }
                })
                .to_string()
                .into(),
            ))
            .await?;
        Ok(())
    }
}

impl Caller for DirectWsCaller {
    fn call_sid(&self) -> &str {
        &self.call_sid
    }

    fn greeting_latency_ms(&self) -> u128 {
        self.greeting_latency_ms
    }

    async fn say(&mut self, utterance_ulaw: &[u8], turn: u32) -> Result<TurnOutcome, BoxError> {
        // Stream the utterance at real time so the server's speech recognizer
        // sees natural pacing and its end-of-speech detector fires.
        for chunk in utterance_ulaw.chunks(FRAME_BYTES) {
            self.send_frame(chunk).await?;
            sleep(Duration::from_millis(20)).await;
        }

        // The caller has stopped talking. Latency the caller perceives is
        // measured from here: it includes end-of-speech detection, transcription,
        // the model, and speech synthesis.
        let spoke_at = Instant::now();

        // Keep the audio stream flowing with silence so the recognizer endpoints.
        let silence = [ULAW_SILENCE; FRAME_BYTES];
        for _ in 0..(TRAILING_SILENCE_MS / 20) {
            self.send_frame(&silence).await?;
            sleep(Duration::from_millis(20)).await;
        }

        let bytes = drain_response(&mut self.ws).await?;

        Ok(TurnOutcome {
            metrics: TurnMetrics {
                turn,
                time_to_first_audio_ms: bytes.first_audio_after(spoke_at),
                response_audio_ms: (bytes.total_audio_bytes / 8) as u128,
            },
        })
    }

    async fn hangup(mut self) -> Result<(), BoxError> {
        self.ws
            .send(Message::Text(
                json!({"event": "stop", "streamSid": self.stream_sid})
                    .to_string()
                    .into(),
            ))
            .await?;
        self.ws.close(None).await.ok();
        Ok(())
    }
}

/// Audio collected while Steve was speaking, plus when the first frame landed.
struct Response {
    first_audio_at: Option<Instant>,
    total_audio_bytes: usize,
}

impl Response {
    fn first_audio_after(&self, spoke_at: Instant) -> u128 {
        self.first_audio_at
            .map(|t| t.duration_since(spoke_at).as_millis())
            .unwrap_or(0)
    }
}

/// Read server messages until Steve stops sending audio for `TURN_END_SILENCE`
/// or the reply times out. `clear` events are acknowledged but carry no audio.
async fn drain_response(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<Response, BoxError> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let mut first_audio_at = None;
    let mut total_audio_bytes = 0usize;
    let mut got_any = false;

    loop {
        // After the first audio frame, stop once a silence gap opens.
        let gap = if got_any {
            TURN_END_SILENCE
        } else {
            REPLY_TIMEOUT
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait = gap.min(remaining);
        if wait.is_zero() {
            break;
        }

        match tokio::time::timeout(wait, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let v: Value = serde_json::from_str(&text)?;
                match v.get("event").and_then(|e| e.as_str()) {
                    Some("media") => {
                        if let Some(p) = v.pointer("/media/payload").and_then(|p| p.as_str()) {
                            if first_audio_at.is_none() {
                                first_audio_at = Some(Instant::now());
                            }
                            total_audio_bytes += STANDARD.decode(p).map(|b| b.len()).unwrap_or(0);
                            got_any = true;
                        }
                    }
                    // "clear" precedes each response; not audio.
                    _ => {}
                }
            }
            Ok(Some(Ok(_))) => {} // non-text frame, ignore
            Ok(Some(Err(e))) => return Err(e.into()),
            Ok(None) => break, // socket closed
            Err(_) => break,   // silence gap or timeout: turn over
        }
    }

    Ok(Response {
        first_audio_at,
        total_audio_bytes,
    })
}

fn short_id() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", n)
}
