//! steve-e2e: end-to-end test harness for the Steve phone assistant.
//!
//! Places scripted calls against a running server, measures answer latency,
//! and asserts the functional outcomes (greeting, caller memory, summaries,
//! bookings) in Firestore. The direct-WebSocket transport needs no phone
//! network; a PSTN transport over a second Twilio number implements the same
//! `Caller` trait for pre-demo runs.
//!
//! Usage:
//!   steve-e2e --scenarios <file-or-dir> [--server ws://127.0.0.1:8080/media-stream]
//!             [--out-dir e2e-report] [--cache-dir <dir>]
//!
//! Reads GCP_PROJECT_ID, ELEVENLABS_API_KEY, ELEVENLABS_VOICE_ID, and
//! LLM_PROVIDER (for the report label) from the environment / .env.

mod caller;
mod report;
mod scenario;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::Duration;

use firestore::FirestoreDb;
use steve_assist::services::{db, elevenlabs};

use caller::{Caller, DirectWsCaller};
use scenario::{Check, Scenario, ScenarioResult};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

struct Args {
    scenarios: PathBuf,
    server: String,
    out_dir: PathBuf,
    cache_dir: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut scenarios = None;
    let mut server = "ws://127.0.0.1:8080/media-stream".to_string();
    let mut out_dir = PathBuf::from("e2e-report");
    let mut cache_dir = std::env::temp_dir().join("steve-e2e-cache");

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut next = || it.next().ok_or_else(|| format!("missing value for {a}"));
        match a.as_str() {
            "--scenarios" => scenarios = Some(PathBuf::from(next()?)),
            "--server" => server = next()?,
            "--out-dir" => out_dir = PathBuf::from(next()?),
            "--cache-dir" => cache_dir = PathBuf::from(next()?),
            "-h" | "--help" => return Err("help".to_string()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        scenarios: scenarios.ok_or("--scenarios is required")?,
        server,
        out_dir,
        cache_dir,
    })
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "steve-e2e: {e}\n\nUsage: steve-e2e --scenarios <file-or-dir> \
                [--server <ws-url>] [--out-dir <dir>] [--cache-dir <dir>]"
            );
            std::process::exit(2);
        }
    };

    if let Err(e) = run(args).await {
        eprintln!("steve-e2e: fatal: {e}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), BoxError> {
    let project_id = std::env::var("GCP_PROJECT_ID").map_err(|_| "GCP_PROJECT_ID must be set")?;
    let el_key =
        std::env::var("ELEVENLABS_API_KEY").map_err(|_| "ELEVENLABS_API_KEY must be set")?;
    let el_voice =
        std::env::var("ELEVENLABS_VOICE_ID").map_err(|_| "ELEVENLABS_VOICE_ID must be set")?;
    let provider = std::env::var("LLM_PROVIDER").unwrap_or_else(|_| "claude".to_string());

    let firestore = db::init(&project_id).await?;
    std::fs::create_dir_all(&args.cache_dir)?;

    let scenarios = load_scenarios(&args.scenarios)?;
    if scenarios.is_empty() {
        return Err(format!("no scenarios found at {}", args.scenarios.display()).into());
    }
    eprintln!(
        "Running {} scenario(s) against {} [provider: {provider}]",
        scenarios.len(),
        args.server
    );

    let ctx = Ctx {
        firestore,
        el_key,
        el_voice,
        cache_dir: args.cache_dir,
        server: args.server.clone(),
        provider,
    };

    let mut results = Vec::new();
    for sc in &scenarios {
        eprintln!("→ {}", sc.name);
        let result = run_scenario(&ctx, sc).await;
        eprintln!("   {}", if result.passed { "pass" } else { "FAIL" });
        results.push(result);
    }

    std::fs::create_dir_all(&args.out_dir)?;
    std::fs::write(
        args.out_dir.join("results.json"),
        report::write_json(&results),
    )?;
    let md = report::write_markdown(&results, &args.server);
    std::fs::write(args.out_dir.join("report.md"), &md)?;
    eprintln!("\n{md}");
    eprintln!("Reports written to {}", args.out_dir.display());

    if results.iter().all(|r| r.passed) {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

struct Ctx {
    firestore: FirestoreDb,
    el_key: String,
    el_voice: String,
    cache_dir: PathBuf,
    server: String,
    provider: String,
}

async fn run_scenario(ctx: &Ctx, sc: &Scenario) -> ScenarioResult {
    match run_scenario_inner(ctx, sc).await {
        Ok(r) => r,
        Err(e) => ScenarioResult {
            name: sc.name.clone(),
            provider: ctx.provider.clone(),
            call_sid: String::new(),
            passed: false,
            greeting_latency_ms: 0,
            turns: Vec::new(),
            checks: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

async fn run_scenario_inner(ctx: &Ctx, sc: &Scenario) -> Result<ScenarioResult, BoxError> {
    if sc.reset_caller {
        let removed = db::forget_caller(&ctx.firestore, &sc.caller_phone).await?;
        eprintln!(
            "   reset caller {} ({removed} memories cleared)",
            sc.caller_phone
        );
    }

    let mut call = DirectWsCaller::connect(&ctx.server, &sc.caller_phone).await?;
    let call_sid = call.call_sid().to_string();
    let greeting_latency_ms = call.greeting_latency_ms();

    let mut turn_metrics = Vec::new();
    for (i, turn) in sc.turns.iter().enumerate() {
        let ulaw = ctx.synthesize(&turn.say).await?;
        let outcome = call.say(&ulaw, (i + 1) as u32).await?;
        turn_metrics.push(outcome.metrics);
    }
    call.hangup().await?;

    // End-of-call processing (summary, memories) runs as background tasks on
    // the server; give it time to land before reading Firestore.
    let messages = wait_for_messages(&ctx.firestore, &call_sid).await?;
    let checks = grade(ctx, sc, &call_sid, &messages).await?;

    let passed = checks.iter().all(|c| c.passed);
    Ok(ScenarioResult {
        name: sc.name.clone(),
        provider: ctx.provider.clone(),
        call_sid,
        passed,
        greeting_latency_ms,
        turns: turn_metrics,
        checks,
        error: None,
    })
}

impl Ctx {
    /// Synthesize an utterance to mu-law 8 kHz, caching by text so repeat runs
    /// don't re-hit ElevenLabs.
    async fn synthesize(&self, text: &str) -> Result<Vec<u8>, BoxError> {
        let mut h = DefaultHasher::new();
        text.hash(&mut h);
        self.el_voice.hash(&mut h);
        let path = self.cache_dir.join(format!("{:x}.ulaw", h.finish()));
        if let Ok(bytes) = std::fs::read(&path) {
            return Ok(bytes);
        }
        let bytes = elevenlabs::text_to_speech(&self.el_key, &self.el_voice, text).await?;
        std::fs::write(&path, &bytes)?;
        Ok(bytes)
    }
}

/// Poll for the assistant's turns to appear, up to ~30 s, so summary/memory
/// background tasks have time to complete.
async fn wait_for_messages(
    db_conn: &FirestoreDb,
    call_sid: &str,
) -> Result<Vec<db::CallMessage>, BoxError> {
    let mut messages = Vec::new();
    for _ in 0..30 {
        messages = db::list_messages(db_conn, call_sid).await?;
        // The summary lands after messages; wait until the session is ended.
        if let Some(s) = db::get_session(db_conn, call_sid).await? {
            if s.ended_at.is_some() {
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Ok(messages)
}

async fn grade(
    ctx: &Ctx,
    sc: &Scenario,
    call_sid: &str,
    messages: &[db::CallMessage],
) -> Result<Vec<Check>, BoxError> {
    let mut checks = Vec::new();
    let assistant: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.content.as_str())
        .collect();

    checks.push(Check {
        name: "assistant replied".into(),
        passed: !assistant.is_empty(),
        detail: format!("{} assistant turn(s) recorded", assistant.len()),
    });

    if let Some(want) = &sc.assertions.greeting_contains {
        let greeting = assistant.first().copied().unwrap_or("");
        checks.push(contains_check("greeting contains", greeting, want));
    }

    for turn in &sc.turns {
        if let Some(want) = &turn.reply_contains {
            let found = assistant
                .iter()
                .any(|a| a.to_lowercase().contains(&want.to_lowercase()));
            checks.push(Check {
                name: format!("some reply contains {want:?}"),
                passed: found,
                detail: if found {
                    "found".into()
                } else {
                    "not found in any assistant turn".into()
                },
            });
        }
    }

    if sc.assertions.caller_created {
        let caller = db::get_caller(&ctx.firestore, &sc.caller_phone).await?;
        checks.push(Check {
            name: "caller profile created".into(),
            passed: caller.is_some(),
            detail: caller
                .map(|c| format!("name={}", c.name))
                .unwrap_or_else(|| "no caller record".into()),
        });
    }

    if sc.assertions.summary_present {
        let summary = db::get_session(&ctx.firestore, call_sid)
            .await?
            .and_then(|s| s.summary);
        let ok = summary
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        checks.push(Check {
            name: "session summary present".into(),
            passed: ok,
            detail: summary
                .map(|s| truncate(&s, 80))
                .unwrap_or_else(|| "empty".into()),
        });
    }

    if sc.assertions.min_memories > 0 {
        let n = db::list_memories_for_call(&ctx.firestore, call_sid)
            .await?
            .len() as u32;
        checks.push(Check {
            name: format!("at least {} memories", sc.assertions.min_memories),
            passed: n >= sc.assertions.min_memories,
            detail: format!("{n} extracted"),
        });
    }

    if sc.assertions.min_bookings > 0 {
        let n = db::list_bookings_for_call(&ctx.firestore, call_sid)
            .await?
            .len() as u32;
        checks.push(Check {
            name: format!("at least {} bookings", sc.assertions.min_bookings),
            passed: n >= sc.assertions.min_bookings,
            detail: format!("{n} recorded"),
        });
    }

    Ok(checks)
}

fn contains_check(name: &str, haystack: &str, want: &str) -> Check {
    let passed = haystack.to_lowercase().contains(&want.to_lowercase());
    Check {
        name: name.to_string(),
        passed,
        detail: if passed {
            format!("{want:?} present")
        } else {
            format!("{want:?} not in {:?}", truncate(haystack, 80))
        },
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn load_scenarios(path: &Path) -> Result<Vec<Scenario>, BoxError> {
    let mut files = Vec::new();
    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let p = entry?.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                files.push(p);
            }
        }
        files.sort();
    } else {
        files.push(path.to_path_buf());
    }
    let mut scenarios = Vec::new();
    for f in files {
        let text = std::fs::read_to_string(&f)?;
        let sc: Scenario =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", f.display()))?;
        scenarios.push(sc);
    }
    Ok(scenarios)
}
