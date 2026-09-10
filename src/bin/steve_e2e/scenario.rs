//! Scenario definitions and results for the end-to-end harness.

use serde::{Deserialize, Serialize};

/// A test scenario: a scripted phone call plus the assertions it must satisfy.
#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub name: String,
    /// Phone number the simulated caller presents. Reused as the test identity.
    pub caller_phone: String,
    /// Wipe this number's caller profile and memories before the call, so
    /// "new caller" scenarios are deterministic.
    #[serde(default)]
    pub reset_caller: bool,
    /// Lines the caller speaks, in order. Steve's turns happen in between.
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub assertions: Assertions,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Turn {
    /// What the caller says. Synthesized to speech and played to Steve.
    pub say: String,
    /// Optional substring that Steve's reply to this turn must contain
    /// (case-insensitive), e.g. the caller's name on a returning-caller turn.
    #[serde(default)]
    pub reply_contains: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Assertions {
    /// The opening greeting must contain this substring (case-insensitive).
    #[serde(default)]
    pub greeting_contains: Option<String>,
    /// A caller profile must exist in Firestore after the call.
    #[serde(default)]
    pub caller_created: bool,
    /// The session summary must be non-empty after end-of-call processing.
    #[serde(default)]
    pub summary_present: bool,
    /// At least this many memories must be extracted for the call.
    #[serde(default)]
    pub min_memories: u32,
    /// At least this many calendar bookings must be recorded for the call.
    #[serde(default)]
    pub min_bookings: u32,
}

/// One graded check with its outcome.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// Latency and token accounting for a single caller/Steve exchange, measured
/// from the client side.
#[derive(Debug, Clone, Serialize)]
pub struct TurnMetrics {
    pub turn: u32,
    /// Milliseconds from the caller finishing speaking to Steve's first audio.
    pub time_to_first_audio_ms: u128,
    /// Milliseconds of audio Steve spoke back.
    pub response_audio_ms: u128,
}

/// The full result of running one scenario.
#[derive(Debug, Clone, Serialize)]
pub struct ScenarioResult {
    pub name: String,
    pub provider: String,
    pub call_sid: String,
    pub passed: bool,
    pub greeting_latency_ms: u128,
    pub turns: Vec<TurnMetrics>,
    pub checks: Vec<Check>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ScenarioResult {
    /// p50 / p95 of per-turn time-to-first-audio, in milliseconds.
    pub fn latency_percentiles(&self) -> (u128, u128) {
        let mut v: Vec<u128> = self
            .turns
            .iter()
            .map(|t| t.time_to_first_audio_ms)
            .collect();
        if v.is_empty() {
            return (0, 0);
        }
        v.sort_unstable();
        let pick = |p: f64| {
            v[((v.len() as f64 * p).ceil() as usize)
                .saturating_sub(1)
                .min(v.len() - 1)]
        };
        (pick(0.50), pick(0.95))
    }
}
