# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Rust AI voice bot ("Steve") that answers inbound phone calls via Twilio, transcribes speech with Google Cloud Speech-to-Text, generates responses with Claude, and speaks back via ElevenLabs TTS. Persists call sessions and cross-call memory in Firestore.

## Build & Run Commands

```bash
cargo build          # Build the project
cargo run            # Run the server (requires .env with API keys)
cargo test           # Run all tests
cargo test <name>    # Run a single test by name
cargo clippy         # Lint
cargo fmt            # Format code
cargo fmt -- --check # Check formatting without modifying
```

Requires a `.env` file with: `TWILIO_ACCOUNT_SID`, `TWILIO_AUTH_TOKEN`, `GOOGLE_APPLICATION_CREDENTIALS` (path to service account JSON), `GCP_PROJECT_ID`, `LLM_PROVIDER` (`claude` or `nemotron`) with the matching `ANTHROPIC_API_KEY` or `NEBIUS_API_KEY`, `ELEVENLABS_API_KEY`, `ELEVENLABS_VOICE_ID`, `SERVER_PORT` (default 8080), `MEMORY_DEPTH` (default 5).

For local dev, use `ngrok http 8080` and set the Twilio webhook to `https://<ngrok-id>.ngrok.io/incoming-call`.

## Architecture

```
Caller → Twilio (PSTN) → Axum Server (Rust/Tokio)
                              ├── POST /incoming-call → TwiML <Connect><Stream> + caller phone via Parameter
                              └── WS /media-stream → bidirectional audio
                                    ├── GCP Speech-to-Text (streaming gRPC, μ-law 8kHz)
                                    ├── Firestore (caller lookup, session/message persistence, memories)
                                    ├── LLM provider (Claude API or Nemotron via Nebius Token Factory)
                                    └── ElevenLabs TTS (text → μ-law 8kHz audio)
```

Each inbound call gets its own Tokio task with isolated conversation state.

## Conversation Loop (media_stream handler)

1. On `Start` event: extract caller phone from TwiML custom parameters, look up caller in Firestore, load memories, build dynamic system prompt
2. Receive Twilio `media` events → decode base64 μ-law → forward to GCP Speech-to-Text
3. On final transcript → send to the LLM provider with dynamic prompt + full history
4. For unknown callers: extract name from first transcript via the LLM, save to Firestore
5. LLM response → ElevenLabs TTS → μ-law 8kHz → chunked back to Twilio
6. Each message pair saved to Firestore (fire-and-forget)
7. On call end: generate summary + extract memories via the LLM, save to Firestore

## Firestore Data Model

- **`callers/{phone}`** — caller profiles (name, first_seen, last_seen, call_count)
- **`call_sessions/{call_sid}`** — session records (caller_phone, caller_name, started_at, ended_at, summary)
- **`call_sessions/{call_sid}/messages/{id}`** — individual messages (role, content, timestamp)
- **`memories/{auto_id}`** — distilled facts from past conversations (caller_phone, call_sid, content, active)

## Caller Identification Flow

- New callers: system prompt instructs Steve to ask for name with varied phrases. Name extracted via Claude after first transcript, saved to `callers` collection.
- Returning callers: system prompt includes name and past memories, instructs Steve to confirm identity with natural varied greetings.

## Audio Format

All audio is **μ-law (PCMU), 8000 Hz, mono**, transported as base64-encoded chunks in WebSocket JSON messages. GCP Speech-to-Text accepts MULAW encoding directly — no re-encoding needed.

## Tech Stack

- **Rust (stable)** with **Tokio** async runtime
- **Axum 0.7** web framework (built-in WebSocket for Twilio)
- **tonic** + **googleapis-tonic-google-cloud-speech-v1** for GCP Speech-to-Text gRPC streaming
- **firestore** crate for Firestore persistence (uses `GOOGLE_APPLICATION_CREDENTIALS`)
- **gcp_auth** for GCP service account authentication
- **reqwest** for HTTP API calls (LLM providers, ElevenLabs)
- **async-trait** for the object-safe `LlmProvider` trait
- **serde/serde_json** for JSON serialization
- **tracing** for logging
- **dotenvy** for .env loading

## Key Design Decisions

- All Firestore writes are fire-and-forget (`tokio::spawn`) — DB failures never block the call
- ElevenLabs `eleven_turbo_v2` model for lowest TTS latency
- Dynamic system prompt composed from: base persona + caller identity layer + memory layer
- End-of-call processing spawns background tasks for summary generation and memory extraction
- Caller phone passed from `/incoming-call` to `/media-stream` via TwiML `<Parameter>` (stateless, no shared HashMap needed)
- Model per agent profile (`agent_profiles.{id}.model`, currently `claude-sonnet-4-6`) with max_tokens=300 for concise spoken responses

## LLM Provider Layer (`src/services/llm/`)

- `llm/mod.rs` — provider-neutral `Message`, `Tool`, `ToolCall`, `ToolResult`, `Completion`, the `LlmProvider` trait, and `from_env()` which picks the provider from `LLM_PROVIDER`
- `llm/claude.rs` — Anthropic Messages API (`ANTHROPIC_API_KEY`)
- `llm/openai_compat.rs` — OpenAI-compatible chat completions; used for NVIDIA Nemotron on Nebius Token Factory (`NEBIUS_API_KEY`, optional `LLM_BASE_URL`). Sends `chat_template_kwargs.enable_thinking=false` by default (`LLM_THINKING=on` to re-enable) because Nemotron 3 otherwise spends the whole `max_tokens` budget on `reasoning_content` and returns empty text; also strips `<think>` blocks so reasoning never reaches TTS
- `services/assistant.rs` — prompt composition and the LLM-backed tasks (respond, extract_name, generate_summary, classify_bot_call, extract_memories). Handlers call this, never a provider directly
- Conversation turns use the profile's `model`; utility tasks use the provider default (`LLM_MODEL` or built-in). When switching providers, update each profile's `model` in Firestore to an id the new provider serves
- History is stored as `llm::Message` variants (`User`, `Assistant`, `ToolCalls`, `ToolResults`); each provider translates to its own wire format, so the tool loop in `media_stream.rs` has no provider-specific code
