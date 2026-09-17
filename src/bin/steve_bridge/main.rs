//! steve-bridge: lets the Linux laptop act as a Bluetooth hands-free unit for
//! the user's phone and bridges live calls into Steve.
//!
//! Call control comes from oFono (HFP hands-free role) over D-Bus; call audio
//! is the pair of PipeWire SCO stream nodes that exist while a call is up.
//! The bridge speaks the Twilio Media Streams protocol to Steve's
//! `/media-stream` endpoint, so the server's conversation loop is reused
//! unchanged. It prefers a local server and falls back to the cloud one.
//!
//! Usage:
//!   steve-bridge            # watch the phone, answer incoming calls, bridge them
//!   steve-bridge loopback   # no phone: laptop mic + speakers, one synthetic call
//!
//! See docs/plans/bluetooth-bridge.md and deploy/laptop/bridge.env.example.

mod audio;
mod ofono;
mod session;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::watch;
use tracing::{error, info, warn};

use audio::AudioTargets;

#[derive(Clone, Debug)]
pub struct Config {
    pub local_ws_url: String,
    pub remote_ws_url: String,
    pub local_connect_timeout: Duration,
    pub answer_delay: Duration,
    pub token: Option<String>,
    /// Keep the far-end voice audible on the laptop speakers.
    pub monitor: bool,
}

impl Config {
    fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Self {
            local_ws_url: env("STEVE_LOCAL_WS_URL")
                .unwrap_or_else(|| "ws://127.0.0.1:8080/media-stream".into()),
            remote_ws_url: env("STEVE_REMOTE_WS_URL")
                .unwrap_or_else(|| "wss://steve-nb.creativecaptains.com/media-stream".into()),
            local_connect_timeout: Duration::from_millis(
                env("BRIDGE_LOCAL_TIMEOUT_MS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1500),
            ),
            answer_delay: Duration::from_millis(
                env("BRIDGE_ANSWER_DELAY_MS")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1500),
            ),
            token: env("BRIDGE_TOKEN"),
            monitor: env("BRIDGE_MONITOR")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false),
        }
    }
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    // tracing-subscriber is built without env-filter here, so RUST_LOG is a
    // plain level (info, debug, ...); default to info.
    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|v| v.parse::<tracing::Level>().ok())
        .unwrap_or(tracing::Level::INFO);
    tracing_subscriber::fmt().with_max_level(level).init();

    let cfg = Config::from_env();
    info!(local = %cfg.local_ws_url, remote = %cfg.remote_ws_url, "steve-bridge starting");

    let mode = std::env::args().nth(1).unwrap_or_default();
    let result = match mode.as_str() {
        "loopback" => run_loopback(cfg, std::env::args().nth(2)).await,
        "" => run_phone(cfg).await,
        other => {
            eprintln!("unknown mode {other:?}; use no argument or `loopback [phone]`");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        error!("bridge exited with error: {e}");
        std::process::exit(1);
    }
}

/// Dev mode: one synthetic call using the laptop's default mic and speakers.
/// Wear headphones, otherwise Steve hears itself.
async fn run_loopback(cfg: Config, phone: Option<String>) -> Result<(), session::BoxError> {
    let phone = phone
        .or_else(|| std::env::var("BRIDGE_LOOPBACK_PHONE").ok())
        .unwrap_or_else(|| "+16263993152".into());
    info!(%phone, "loopback call: default devices, Ctrl-C to end");
    let (end_tx, end_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = end_tx.send(true);
    });
    let targets = AudioTargets {
        source: None,
        sink: None,
    };
    session::bridge_call(&cfg, &phone, "loopback", targets, end_rx).await
}

/// Normal mode: watch oFono for incoming calls on the phone and bridge them.
async fn run_phone(cfg: Config) -> Result<(), session::BoxError> {
    let conn = zbus::Connection::system().await?;
    let modem = loop {
        match ofono::find_hfp_modem(&conn).await? {
            Some(m) => break m,
            None => {
                warn!("no HFP modem in oFono yet (phone not connected?); retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };
    info!(modem = %modem, "watching for incoming calls");

    let vcm = ofono::VoiceCallManagerProxy::builder(&conn)
        .path(modem.clone())?
        .build()
        .await?;
    let mut added = vcm.receive_call_added().await?;
    let mut removed = vcm.receive_call_removed().await?;

    // Calls that are already ringing when we start.
    for (path, props) in vcm.get_calls().await.unwrap_or_default() {
        if ofono::prop_str(&props, "State").as_deref() == Some("incoming") {
            info!(call = %path, "call already ringing at startup");
        }
    }

    let busy = Arc::new(AtomicBool::new(false));
    // Broadcast of "call <path> ended" so a running session can stop.
    let (ended_tx, _) = tokio::sync::broadcast::channel::<String>(8);
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = &mut ctrl_c => { info!("Ctrl-C, exiting"); return Ok(()); }
            Some(sig) = removed.next() => {
                if let Ok(args) = sig.args() {
                    let path = args.path().to_string();
                    info!(call = %path, "call removed");
                    let _ = ended_tx.send(path);
                }
            }
            Some(sig) = added.next() => {
                let Ok(args) = sig.args() else { continue };
                let path = args.path().clone();
                let props = args.properties();
                let state = ofono::prop_str(props, "State").unwrap_or_default();
                let line = ofono::prop_str(props, "LineIdentification").unwrap_or_default();
                info!(call = %path, %state, caller = %line, "call added");
                if state != "incoming" {
                    continue; // outgoing or already-active calls are the human's
                }
                if busy.swap(true, Ordering::SeqCst) {
                    warn!(call = %path, "already bridging a call; ignoring this one");
                    continue;
                }
                let cfg = cfg.clone();
                let conn = conn.clone();
                let busy = busy.clone();
                let mut ended_rx = ended_tx.subscribe();
                tokio::spawn(async move {
                    let call_path = path.to_string();
                    let (end_tx, end_rx) = watch::channel(false);
                    let cp = call_path.clone();
                    let forward = tokio::spawn(async move {
                        while let Ok(p) = ended_rx.recv().await {
                            if p == cp { let _ = end_tx.send(true); break; }
                        }
                    });
                    if let Err(e) = handle_incoming(&cfg, &conn, &call_path, &line, end_rx).await {
                        error!(call = %call_path, "call handling failed: {e}");
                    }
                    forward.abort();
                    busy.store(false, Ordering::SeqCst);
                });
            }
        }
    }
}

async fn handle_incoming(
    cfg: &Config,
    conn: &zbus::Connection,
    call_path: &str,
    caller: &str,
    end_rx: watch::Receiver<bool>,
) -> Result<(), session::BoxError> {
    let call = ofono::VoiceCallProxy::builder(conn)
        .path(call_path.to_string())?
        .build()
        .await?;

    // Ring delay: gives the human a chance to pick up on the phone. If the
    // call goes active or away before we answer, it is not ours.
    tokio::time::sleep(cfg.answer_delay).await;
    if *end_rx.borrow() {
        info!(call = %call_path, "caller hung up before we answered");
        return Ok(());
    }
    let state = call
        .get_properties()
        .await
        .ok()
        .and_then(|p| ofono::prop_str(&p, "State"))
        .unwrap_or_default();
    if state != "incoming" {
        info!(call = %call_path, %state, "call no longer incoming (human took it?); not bridging");
        return Ok(());
    }

    info!(call = %call_path, %caller, "answering");
    call.answer().await?;

    let targets = match audio::wait_for_bt_nodes(Duration::from_secs(20)).await {
        Some(t) => t,
        None => {
            error!("Bluetooth audio nodes never appeared; hanging up");
            let _ = call.hangup().await;
            return Ok(());
        }
    };
    info!(source = ?targets.source, sink = ?targets.sink, "SCO audio nodes ready");
    audio::detach_from_default_devices(&targets, cfg.monitor).await;

    let result = session::bridge_call(cfg, caller, call_path, targets, end_rx.clone()).await;

    // If the server ended the conversation while the call is still up, end it.
    if !*end_rx.borrow() {
        info!(call = %call_path, "session over, hanging up");
        let _ = call.hangup().await;
    }
    result
}
