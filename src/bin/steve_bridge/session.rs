//! One bridged call: a Twilio-Media-Streams-compatible WebSocket session with
//! Steve, fed by the capture pipe and draining into the playback pipe.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, info, warn};

use steve_assist::twilio::audio::{decode_payload, encode_payload};

use crate::Config;
use crate::audio::{AudioTargets, Playback, start_capture};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// What kind of call Steve is joining; decides the server-side prompt.
#[derive(Clone, Debug)]
pub enum CallKind {
    /// Someone called the phone: Steve answers as the assistant.
    Incoming,
    /// The phone placed the call (dialed by the user or by `steve-bridge dial`):
    /// Steve pursues `objective` with whoever answers, like an assignment.
    Outgoing { objective: String },
}

#[derive(Clone, Debug)]
pub struct CallParams {
    /// The other party's number (caller id, or the number dialed).
    pub phone: String,
    /// oFono call path or "loopback"; only for logs.
    pub label: String,
    pub kind: CallKind,
}

/// Local server first; the cloud deployment if it does not answer in time.
async fn connect_with_fallback(cfg: &Config) -> Result<(Ws, &'static str), BoxError> {
    match tokio::time::timeout(
        cfg.local_connect_timeout,
        tokio_tungstenite::connect_async(&cfg.local_ws_url),
    )
    .await
    {
        Ok(Ok((ws, _))) => return Ok((ws, "local")),
        Ok(Err(e)) => {
            info!(url = %cfg.local_ws_url, "local server unavailable ({e}); using remote")
        }
        Err(_) => info!(url = %cfg.local_ws_url, "local server timed out; using remote"),
    }
    let (ws, _) = tokio_tungstenite::connect_async(&cfg.remote_ws_url).await?;
    Ok((ws, "remote"))
}

fn short_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// Runs until the call ends (`end_rx` becomes true), the server closes the
/// stream, or the capture pipe ends.
pub async fn bridge_call(
    cfg: &Config,
    params: &CallParams,
    targets: AudioTargets,
    mut end_rx: watch::Receiver<bool>,
) -> Result<(), BoxError> {
    let (mut ws, which) = connect_with_fallback(cfg).await?;
    let stream_sid = format!("MZbt{}", short_id());
    let call_sid = format!("BT{}", short_id());
    info!(server = which, %call_sid, phone = %params.phone, kind = ?params.kind, "stream starting");

    ws.send(Message::Text(
        json!({"event": "connected", "protocol": "Call", "version": "1.0.0"}).to_string(),
    ))
    .await?;
    let mut custom = json!({
        "callerPhone": params.phone,
        "source": "bluetooth",
        "bridgeCall": params.label,
    });
    if let CallKind::Outgoing { objective } = &params.kind {
        custom["direction"] = Value::String("outgoing".into());
        custom["objective"] = Value::String(objective.clone());
        custom["contactPhone"] = Value::String(params.phone.clone());
    }
    if let Some(t) = &cfg.token {
        custom["token"] = Value::String(t.clone());
    }
    ws.send(Message::Text(
        json!({
            "event": "start",
            "streamSid": stream_sid,
            "start": {
                "callSid": call_sid,
                "tracks": ["inbound"],
                "customParameters": custom
            }
        })
        .to_string(),
    ))
    .await?;

    let (mut capture_child, mut frames) = start_capture(&targets.source)?;
    let mut playback = Playback::new(targets.sink.clone());
    let mut sent_frames: u64 = 0;
    let mut recv_frames: u64 = 0;
    let started = std::time::Instant::now();

    loop {
        tokio::select! {
            changed = end_rx.changed() => {
                if changed.is_err() || *end_rx.borrow() {
                    info!("call ended; stopping stream");
                    break;
                }
            }
            frame = frames.recv() => {
                match frame {
                    Some(f) => {
                        sent_frames += 1;
                        let msg = json!({
                            "event": "media",
                            "streamSid": stream_sid,
                            "media": { "payload": encode_payload(&f) }
                        });
                        if let Err(e) = ws.send(Message::Text(msg.to_string())).await {
                            warn!("send failed: {e}");
                            break;
                        }
                    }
                    None => {
                        info!("capture ended (SCO link gone); stopping stream");
                        break;
                    }
                }
            }
            msg = ws.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                        match v.get("event").and_then(Value::as_str) {
                            Some("media") => {
                                if let Some(p) = v.pointer("/media/payload").and_then(Value::as_str) {
                                    recv_frames += 1;
                                    playback.write(&decode_payload(p)).await;
                                }
                            }
                            Some("clear") => {
                                debug!("clear: dropping queued playback");
                                playback.clear().await;
                            }
                            Some("mark") => {}
                            other => debug!(?other, "ignoring event"),
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("server closed the stream");
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => { warn!("websocket error: {e}"); break; }
                }
            }
        }
    }

    let _ = ws
        .send(Message::Text(
            json!({"event": "stop", "streamSid": stream_sid}).to_string(),
        ))
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(2), ws.close(None)).await;
    let _ = capture_child.kill().await;
    playback.stop().await;
    info!(
        server = which, %call_sid, sent_frames, recv_frames,
        secs = started.elapsed().as_secs(), "stream finished"
    );
    Ok(())
}
