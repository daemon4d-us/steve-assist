# Steve end-to-end tests

`steve-e2e` places scripted calls against a running Steve server, measures how
fast Steve answers each turn, and asserts the functional outcomes in Firestore.

## Running (direct WebSocket — no phone network)

1. Start the server locally (`.env` with `GCP_PROJECT_ID`, `ELEVENLABS_*`,
   the LLM provider key, and Deepgram or Google STT).
2. In another shell:

   ```bash
   cargo run --bin steve-e2e -- --scenarios e2e/scenarios
   ```

Reports are written to `e2e-report/results.json` and `e2e-report/report.md`.
The process exits non-zero if any assertion fails, so it works as a CI gate.

To compare providers, run the server once per provider (set `LLM_PROVIDER`) and
keep each run's `report.md`; the provider is labeled in every row.

## What it measures

- **Non-functional:** greeting latency, and per-turn time-to-first-audio
  (p50/p95) — the delay a caller actually hears. The server also emits a
  `steve_assist::metrics` log line per turn breaking the number into
  LLM / TTS / send time and token counts.
- **Functional:** greeting content, caller profile creation, returning-caller
  recognition, end-of-call summary, memory extraction, calendar bookings.

## Scenario format

One JSON file per scenario (see `scenarios/`). `reset_caller` wipes the number's
caller profile and memories first, for deterministic new-caller runs.

## PSTN transport (second Twilio number)

The `Caller` trait in `src/bin/steve_e2e/caller.rs` abstracts the transport.
The direct-WebSocket caller is implemented; a PSTN caller that dials Steve from
a dedicated Twilio test number over a `<Connect><Stream>` leg implements the
same trait, so scenarios and grading are unchanged. Fill it in once the test
number is provisioned.
