//! Report writers: machine-readable JSON and a human Markdown summary.

use std::fmt::Write as _;

use crate::scenario::ScenarioResult;

pub fn write_json(results: &[ScenarioResult]) -> String {
    serde_json::to_string_pretty(results).unwrap_or_else(|_| "[]".to_string())
}

pub fn write_markdown(results: &[ScenarioResult], server_url: &str) -> String {
    let mut m = String::new();
    let passed = results.iter().filter(|r| r.passed).count();
    let _ = writeln!(m, "# Steve end-to-end results\n");
    let _ = writeln!(m, "- Target: `{server_url}`");
    let _ = writeln!(
        m,
        "- Scenarios: {} ({} passed, {} failed)\n",
        results.len(),
        passed,
        results.len() - passed
    );

    let _ = writeln!(m, "## Latency and functional summary\n");
    let _ = writeln!(
        m,
        "| Scenario | Provider | Result | Greeting ms | Turn p50 ms | Turn p95 ms | Turns |"
    );
    let _ = writeln!(m, "|---|---|---|---|---|---|---|");
    for r in results {
        let (p50, p95) = r.latency_percentiles();
        let _ = writeln!(
            m,
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.name,
            r.provider,
            if r.passed { "pass" } else { "FAIL" },
            r.greeting_latency_ms,
            p50,
            p95,
            r.turns.len(),
        );
    }

    for r in results {
        let _ = writeln!(m, "\n## {} ({})\n", r.name, r.provider);
        if let Some(e) = &r.error {
            let _ = writeln!(m, "Errored before completion: {e}\n");
        }
        let _ = writeln!(m, "Call SID: `{}`\n", r.call_sid);
        let _ = writeln!(m, "Checks:\n");
        for c in &r.checks {
            let _ = writeln!(
                m,
                "- {} **{}** — {}",
                if c.passed { "✓" } else { "✗" },
                c.name,
                c.detail
            );
        }
    }
    m
}
