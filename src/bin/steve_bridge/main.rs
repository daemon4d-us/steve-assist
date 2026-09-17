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
//!   steve-bridge                         # watch the phone; answer incoming calls,
//!                                        # join outgoing ones when an objective is set
//!   steve-bridge dial <number> <objective...>   # have the phone call someone and
//!                                        # let Steve run the call with that objective
//!   steve-bridge loopback [phone]        # no phone: laptop mic + speakers
//!
//! Outgoing calls you dial yourself stay yours unless BRIDGE_OBJECTIVE_FILE
//! (default ~/.config/steve-bridge/objective.txt) holds an objective; the file
//! is renamed to .used after the call so Steve only takes that one call.
//!
//! See docs/plans/bluetooth-bridge.md and deploy/laptop/bridge.env.example.

mod audio;
mod ofono;
mod session;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::watch;
use tracing::{error, info, warn};

use audio::AudioTargets;
use session::{BoxError, CallKind, CallParams};

const OUTGOING_STATES: [&str; 3] = ["dialing", "alerting", "active"];

#[derive(Clone, Debug)]
pub struct Config {
    pub local_ws_url: String,
    pub remote_ws_url: String,
    pub local_connect_timeout: Duration,
    pub answer_delay: Duration,
    /// How long to wait for an outgoing call to be answered.
    pub dial_timeout: Duration,
    pub token: Option<String>,
    /// Keep the far-end voice audible on the laptop speakers.
    pub monitor: bool,
    pub objective_file: PathBuf,
}

impl Config {
    fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
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
            dial_timeout: Duration::from_secs(
                env("BRIDGE_DIAL_TIMEOUT_S")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(90),
            ),
            token: env("BRIDGE_TOKEN"),
            monitor: env("BRIDGE_MONITOR")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(false),
            objective_file: env("BRIDGE_OBJECTIVE_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(home).join(".config/steve-bridge/objective.txt")),
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

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => run_phone(cfg).await,
        Some("loopback") => run_loopback(cfg, args.get(1).cloned()).await,
        Some("dial") => match (args.get(1), args.get(2..)) {
            (Some(number), Some(rest)) if !rest.is_empty() => {
                run_dial(cfg, number.clone(), rest.join(" ")).await
            }
            _ => {
                eprintln!("usage: steve-bridge dial <number> <objective...>");
                std::process::exit(2);
            }
        },
        Some(other) => {
            eprintln!(
                "unknown mode {other:?}; use no argument, `dial <number> <objective>` or `loopback [phone]`"
            );
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
async fn run_loopback(cfg: Config, phone: Option<String>) -> Result<(), BoxError> {
    let phone = phone
        .or_else(|| std::env::var("BRIDGE_LOOPBACK_PHONE").ok())
        .unwrap_or_else(|| "+16263993152".into());
    info!(%phone, "loopback call: default devices, Ctrl-C to end");
    let (end_tx, end_rx) = watch::channel(false);
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = end_tx.send(true);
    });
    let params = CallParams {
        phone,
        label: "loopback".into(),
        kind: CallKind::Incoming,
    };
    session::bridge_call(
        &cfg,
        &params,
        AudioTargets {
            source: None,
            sink: None,
        },
        end_rx,
    )
    .await
}

async fn connect_modem(
    conn: &zbus::Connection,
) -> Result<ofono::VoiceCallManagerProxy<'static>, BoxError> {
    let modem = loop {
        match ofono::find_hfp_modem(conn).await? {
            Some(m) => break m,
            None => {
                warn!("no HFP modem in oFono yet (phone not connected?); retrying in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };
    info!(modem = %modem, "HFP modem found");
    Ok(ofono::VoiceCallManagerProxy::builder(conn)
        .path(modem)?
        .build()
        .await?)
}

/// Turn oFono's CallRemoved signal for `call_path` into a watch flag.
fn watch_call_end(
    vcm: &ofono::VoiceCallManagerProxy<'static>,
    call_path: String,
) -> watch::Receiver<bool> {
    let (end_tx, end_rx) = watch::channel(false);
    let vcm = vcm.clone();
    tokio::spawn(async move {
        let Ok(mut removed) = vcm.receive_call_removed().await else {
            return;
        };
        while let Some(sig) = removed.next().await {
            if let Ok(args) = sig.args()
                && args.path().as_str() == call_path
            {
                let _ = end_tx.send(true);
                break;
            }
        }
    });
    end_rx
}

/// `steve-bridge dial <number> <objective>`: the phone places the call, Steve
/// talks to whoever answers.
async fn run_dial(cfg: Config, number: String, objective: String) -> Result<(), BoxError> {
    let conn = zbus::Connection::system().await?;
    let vcm = connect_modem(&conn).await?;
    info!(%number, %objective, "dialing from the phone");
    let call_path = vcm.dial(&number, "").await?;
    info!(call = %call_path, "call placed");
    let end_rx = watch_call_end(&vcm, call_path.to_string());
    handle_outgoing(&cfg, &conn, call_path.as_str(), &number, objective, end_rx).await
}

/// Normal mode: watch oFono; answer incoming calls, join outgoing ones that
/// have an objective.
async fn run_phone(cfg: Config) -> Result<(), BoxError> {
    let conn = zbus::Connection::system().await?;
    let vcm = connect_modem(&conn).await?;
    info!(objective_file = %cfg.objective_file.display(), "watching for calls");
    let mut added = vcm.receive_call_added().await?;

    let busy = Arc::new(AtomicBool::new(false));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
            _ = &mut ctrl_c => { info!("Ctrl-C, exiting"); return Ok(()); }
            Some(sig) = added.next() => {
                let Ok(args) = sig.args() else { continue };
                let path = args.path().to_string();
                let props = args.properties();
                let state = ofono::prop_str(props, "State").unwrap_or_default();
                let line = ofono::prop_str(props, "LineIdentification").unwrap_or_default();
                info!(call = %path, %state, phone = %line, "call added");

                let outgoing_objective = if OUTGOING_STATES.contains(&state.as_str()) {
                    match read_objective(&cfg.objective_file) {
                        Some(o) => Some(o),
                        None => {
                            info!(call = %path, "outgoing call with no objective set; leaving it to the human");
                            continue;
                        }
                    }
                } else if state == "incoming" {
                    None
                } else {
                    continue;
                };
                if busy.swap(true, Ordering::SeqCst) {
                    warn!(call = %path, "already bridging a call; ignoring this one");
                    continue;
                }

                let cfg = cfg.clone();
                let conn = conn.clone();
                let busy = busy.clone();
                let end_rx = watch_call_end(&vcm, path.clone());
                tokio::spawn(async move {
                    let result = match outgoing_objective {
                        Some(objective) => {
                            let r = handle_outgoing(&cfg, &conn, &path, &line, objective, end_rx).await;
                            consume_objective(&cfg.objective_file);
                            r
                        }
                        None => handle_incoming(&cfg, &conn, &path, &line, end_rx).await,
                    };
                    if let Err(e) = result {
                        error!(call = %path, "call handling failed: {e}");
                    }
                    busy.store(false, Ordering::SeqCst);
                });
            }
        }
    }
}

/// The objective for the next outgoing call, if the file holds one. Lines
/// starting with `#` are comments; blank content means "no objective".
fn read_objective(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let objective = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!objective.is_empty()).then_some(objective)
}

/// One-shot: after Steve has used the objective, park it as `.used` so the
/// next call you dial is yours again.
fn consume_objective(path: &std::path::Path) {
    let used = path.with_extension("used");
    match std::fs::rename(path, &used) {
        Ok(()) => info!(file = %used.display(), "objective consumed"),
        Err(e) => warn!("could not rename objective file: {e}"),
    }
}

async fn handle_incoming(
    cfg: &Config,
    conn: &zbus::Connection,
    call_path: &str,
    caller: &str,
    end_rx: watch::Receiver<bool>,
) -> Result<(), BoxError> {
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
    let state = ofono::call_state(&call).await.unwrap_or_default();
    if state != "incoming" {
        info!(call = %call_path, %state, "call no longer incoming (human took it?); not bridging");
        return Ok(());
    }

    info!(call = %call_path, %caller, "answering");
    call.answer().await?;

    let params = CallParams {
        phone: caller.to_string(),
        label: call_path.to_string(),
        kind: CallKind::Incoming,
    };
    run_bridged(cfg, &call, params, end_rx).await
}

async fn handle_outgoing(
    cfg: &Config,
    conn: &zbus::Connection,
    call_path: &str,
    number: &str,
    objective: String,
    end_rx: watch::Receiver<bool>,
) -> Result<(), BoxError> {
    let call = ofono::VoiceCallProxy::builder(conn)
        .path(call_path.to_string())?
        .build()
        .await?;
    info!(call = %call_path, %number, "outgoing call: waiting for the other party to answer");
    match ofono::wait_for_state(&call, &["active"], cfg.dial_timeout).await {
        Some(s) if s == "active" => {}
        Some(s) => {
            info!(call = %call_path, state = %s, "outgoing call ended before it was answered");
            return Ok(());
        }
        None => {
            warn!(call = %call_path, "nobody answered within the dial timeout; hanging up");
            let _ = call.hangup().await;
            return Ok(());
        }
    }
    info!(call = %call_path, "answered; Steve is joining with objective: {objective}");
    let params = CallParams {
        phone: number.to_string(),
        label: call_path.to_string(),
        kind: CallKind::Outgoing { objective },
    };
    run_bridged(cfg, &call, params, end_rx).await
}

/// Shared tail for both directions: wait for the SCO audio nodes, detach the
/// laptop's own devices, run the stream, hang up if the server ended it.
async fn run_bridged(
    cfg: &Config,
    call: &ofono::VoiceCallProxy<'_>,
    params: CallParams,
    end_rx: watch::Receiver<bool>,
) -> Result<(), BoxError> {
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

    let result = session::bridge_call(cfg, &params, targets, end_rx.clone()).await;

    // If the server ended the conversation while the call is still up, end it.
    if !*end_rx.borrow() {
        info!(call = %params.label, "session over, hanging up");
        let _ = call.hangup().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objective_file_parsing() {
        let dir = std::env::temp_dir().join(format!("steve-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("objective.txt");
        assert_eq!(read_objective(&f), None);
        std::fs::write(&f, "# next call\n\nBook a table for two\n  at seven.\n").unwrap();
        assert_eq!(
            read_objective(&f).as_deref(),
            Some("Book a table for two at seven.")
        );
        std::fs::write(&f, "# only a comment\n\n").unwrap();
        assert_eq!(read_objective(&f), None);
        std::fs::write(&f, "x").unwrap();
        consume_objective(&f);
        assert!(!f.exists());
        assert!(dir.join("objective.used").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
