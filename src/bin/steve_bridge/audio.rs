//! PipeWire side of the bridge: find the phone's SCO stream nodes, keep the
//! laptop's own mic and speakers out of the call, and run pw-record / pw-play
//! as μ-law 8 kHz pipes.
//!
//! PipeWire exposes a phone call as two dynamic *stream* nodes that exist only
//! while the SCO link is up: `bluez_input.<addr>.N` carries the far-end voice
//! (a playback stream WirePlumber links to the default sink) and
//! `bluez_output.<addr>.N` feeds audio into the call (a capture stream linked
//! from the default source). pw-cat can target them by name and speaks μ-law
//! natively, so no resampling or G.711 code is needed here.

use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// 20 ms of μ-law at 8 kHz: the frame size Twilio media events carry.
pub const FRAME_BYTES: usize = 160;

#[derive(Clone, Debug)]
pub struct AudioTargets {
    /// Node to record far-end audio from; `None` = default source (loopback).
    pub source: Option<String>,
    /// Node to play Steve's voice into; `None` = default sink (loopback).
    pub sink: Option<String>,
}

#[derive(Debug, Default)]
struct Graph {
    nodes: Vec<(u64, String)>,   // (id, node.name)
    links: Vec<(u64, u64, u64)>, // (link id, output node id, input node id)
}

/// `pw-dump` can emit several concatenated JSON documents when the graph
/// changes mid-dump; decode all of them.
async fn dump_graph() -> Graph {
    let out = match Command::new("pw-dump").output().await {
        Ok(o) => o.stdout,
        Err(e) => {
            warn!("pw-dump failed: {e}");
            return Graph::default();
        }
    };
    let mut g = Graph::default();
    for doc in serde_json::Deserializer::from_slice(&out).into_iter::<Value>() {
        let Ok(doc) = doc else { break };
        let Some(objs) = doc.as_array() else { continue };
        for o in objs {
            let id = o.get("id").and_then(Value::as_u64).unwrap_or(0);
            let info = o.get("info").cloned().unwrap_or(Value::Null);
            match o.get("type").and_then(Value::as_str) {
                Some("PipeWire:Interface:Node") => {
                    if let Some(name) = info
                        .get("props")
                        .and_then(|p| p.get("node.name"))
                        .and_then(Value::as_str)
                    {
                        g.nodes.push((id, name.to_string()));
                    }
                }
                Some("PipeWire:Interface:Link") => {
                    let on = info.get("output-node-id").and_then(Value::as_u64);
                    let inn = info.get("input-node-id").and_then(Value::as_u64);
                    if let (Some(on), Some(inn)) = (on, inn) {
                        g.links.push((id, on, inn));
                    }
                }
                _ => {}
            }
        }
    }
    g
}

pub async fn find_bt_nodes() -> Option<AudioTargets> {
    let g = dump_graph().await;
    let source = g
        .nodes
        .iter()
        .find(|(_, n)| n.starts_with("bluez_input."))?
        .1
        .clone();
    let sink = g
        .nodes
        .iter()
        .find(|(_, n)| n.starts_with("bluez_output."))?
        .1
        .clone();
    Some(AudioTargets {
        source: Some(source),
        sink: Some(sink),
    })
}

pub async fn wait_for_bt_nodes(timeout: Duration) -> Option<AudioTargets> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(t) = find_bt_nodes().await {
            return Some(t);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    None
}

/// WirePlumber auto-links the call streams to the laptop's default sink and
/// source. Break those links so the laptop mic does not leak into the call
/// and (unless monitoring) the far end does not play on the speakers. Runs a
/// few passes because the links can appear slightly after the nodes.
pub async fn detach_from_default_devices(t: &AudioTargets, keep_monitor: bool) {
    let (Some(src), Some(snk)) = (&t.source, &t.sink) else {
        return;
    };
    for pass in 0..3 {
        let g = dump_graph().await;
        let src_id = g.nodes.iter().find(|(_, n)| n == src).map(|(i, _)| *i);
        let snk_id = g.nodes.iter().find(|(_, n)| n == snk).map(|(i, _)| *i);
        let mut removed = 0;
        for (link, out_node, in_node) in &g.links {
            let far_end_to_speakers = Some(*out_node) == src_id;
            let mic_into_call = Some(*in_node) == snk_id;
            if (far_end_to_speakers && !keep_monitor) || mic_into_call {
                debug!(link, "removing auto link");
                let _ = Command::new("pw-link")
                    .arg("-d")
                    .arg(link.to_string())
                    .output()
                    .await;
                removed += 1;
            }
        }
        if removed > 0 {
            info!(pass, removed, "detached call streams from default devices");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn pw_cat(bin: &str, target: &Option<String>) -> Command {
    let mut cmd = Command::new(bin);
    if let Some(t) = target {
        cmd.arg("--target").arg(t);
    }
    cmd.args([
        "--format",
        "ulaw",
        "--rate",
        "8000",
        "--channels",
        "1",
        "--latency",
        "40ms",
        "-",
    ]);
    cmd.kill_on_drop(true);
    cmd
}

/// Far-end audio → 160-byte μ-law frames on the channel. Ends when pw-record
/// exits (the SCO node went away) or the receiver is dropped.
pub fn start_capture(target: &Option<String>) -> std::io::Result<(Child, mpsc::Receiver<Vec<u8>>)> {
    let mut child = pw_cat("pw-record", target)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
    tokio::spawn(async move {
        let mut frame = vec![0u8; FRAME_BYTES];
        loop {
            match stdout.read_exact(&mut frame).await {
                Ok(_) => {
                    if tx.send(frame.clone()).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!("capture ended: {e}");
                    break;
                }
            }
        }
    });
    Ok((child, rx))
}

/// Steve's voice → the call. `clear()` drops whatever is queued (barge-in) by
/// restarting pw-play.
pub struct Playback {
    target: Option<String>,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl Playback {
    pub fn new(target: Option<String>) -> Self {
        Self {
            target,
            child: None,
            stdin: None,
        }
    }

    fn ensure_running(&mut self) -> std::io::Result<()> {
        if self.stdin.is_some() {
            return Ok(());
        }
        let mut child = pw_cat("pw-play", &self.target)
            .stdin(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        self.stdin = child.stdin.take();
        self.child = Some(child);
        Ok(())
    }

    pub async fn write(&mut self, ulaw: &[u8]) {
        if let Err(e) = self.ensure_running() {
            warn!("pw-play failed to start: {e}");
            return;
        }
        if let Some(stdin) = self.stdin.as_mut()
            && let Err(e) = stdin.write_all(ulaw).await
        {
            debug!("pw-play write failed, restarting: {e}");
            self.stop().await;
        }
    }

    pub async fn clear(&mut self) {
        self.stop().await;
    }

    pub async fn stop(&mut self) {
        self.stdin.take();
        if let Some(mut c) = self.child.take() {
            let _ = c.kill().await;
        }
    }
}
