# Steve on Nebius: Hackathon Summary

*2026-10-07 · Dmitrii Sidorenko*

Steve is a personal AI phone assistant (Rust/Tokio, Twilio, Deepgram, ElevenLabs, Firestore) rebuilt for the NVIDIA + Nebius hackathon to run its conversation on NVIDIA Nemotron 3 Super 120B-A12B served by Nebius Token Factory, from a Nebius VM in the same us-central1 region. Callers hear a reply about 1.2 s after they stop speaking; the model itself answers in about 0.5 s.

## Model choice

Every spoken turn, the greeting and the post-call tasks (summary, memory extraction, caller-name extraction) run on `nvidia/nemotron-3-super-120b-a12b` through Nebius Token Factory at `api.tokenfactory.us-central1.nebius.com`. The weights are used as served; nothing was fine-tuned.

| Decision | Why |
| --- | --- |
| NVIDIA Nemotron 3 family | Hackathon requirement: an NVIDIA model served on Nebius. |
| Super 120B-A12B variant | Mixture-of-experts: ~120B total parameters, ~12B active per token. Large-model instruction following and tool use at roughly 12B inference cost, which is what a live phone call can afford. Measured: 0.35 s for a short turn, 0.5 s median including tool-calling turns. |
| Same region as Token Factory | The Steve VM runs in the Nebius us-central1 project, so the service-to-model round trip stays inside the region. |
| One model for everything | Simplicity under hackathon time: one key, one endpoint, one failure mode. Post-call tasks are latency-insensitive and could move to a smaller model via `LLM_MODEL`. |

Claude Sonnet 4.6 remains implemented as a second provider (`LLM_PROVIDER=claude`) and was what the project ran on before the Nebius move; the provider layer has no Nemotron-specific code outside its adapter. The Nemotron variants were not benchmarked against each other for this workload; with the profile now read per call, that comparison is a one-field change in the dashboard.

## Latency on Nebius

A caller hears Steve's reply a median 1.2 s after they finish speaking (12 scripted turns across 3 calls, 0.54 s best, 2.4 s worst); the opening greeting arrives 1.8 s after pickup for a new caller. Measured on 2026-10-06 with the project's `steve-e2e` harness: scripted calls over WebSocket from a laptop to the us-central1 VM, so the Twilio PSTN leg is not included.

| Stage of a turn (server side, 13 turns) | Median | Range |
| --- | --- | --- |
| Nemotron via Token Factory | 505 ms | 346 ms – 2.4 s |
| ElevenLabs TTS (`eleven_turbo_v2`) | 441 ms | 297 ms – 1.15 s |
| Send to stream | 1 ms | 0 – 7 ms |
| Server total | 994 ms | 664 ms – 3.5 s |

Plain conversational turns (no tool call, ~2.2k input tokens, 8–45 output tokens) take 0.65–1.1 s server side. Turns that call a calendar tool take 1.9–3.5 s: the model runs twice, the context grows to 4–7k tokens and the reply is longer (150–280 tokens, so TTS takes 0.5–1.2 s). The 200–300 ms between server total and what the caller perceives is speech end-pointing, the final transcript and network transit.

**Greeting optimisation.** The greeting was the slowest moment at 2.1–3.4 s while the model needed only ~270 ms of it: four Firestore reads (agent profile, calendar tokens, caller record, memories) ran one after another before the prompt could be built. Running them concurrently under one `tokio::join!` cut that setup to 92–400 ms.

| Greeting (new caller) | Before | After |
| --- | --- | --- |
| Firestore setup | ~1–1.5 s (inferred) | 92 ms (measured) |
| Nemotron | 270 ms | 260 ms |
| ElevenLabs TTS | 450 ms | 395 ms |
| Server total, pickup to audio ready | ~2 s | 0.75 s |
| Caller-perceived | 2.1–3.4 s | 1.8 s |

The remaining ~1.1 s between the server's 0.75 s and the caller's 1.8 s is the WebSocket/TLS handshake and the `start` round trip between the test client and us-central1; on a real call that part belongs to Twilio.

## How Nemotron performed

Overall 6/10 for this use case: 8/10 on speech quality and tool-call mechanics, 4/10 on following prose rules and reasoning about dates. Every weakness proved fixable deterministically on the server, which is why the score is a 6 and not a 5; the fixes were necessary rather than optional, which is why it is not a 7. Based on about a dozen end-to-end runs on 2026-10-05/06, their transcripts and the VM's tool-call logs.

| Dimension | Score | Evidence |
| --- | --- | --- |
| Speed | 9 | 0.35 s short turn, 0.5 s median with tool calls, 0.26 s greeting, from a 120B model. |
| Conversational text | 8 | Natural greetings, used the caller's name, picked up a persona-name change on the next call, 39 output tokens median. |
| Tool-call mechanics | 8 | ~40 tool invocations, zero malformed; correct ISO timestamps and offsets; adopted new arguments (`start_weekday`, `caller_confirmed`) on first use; re-issued after tool errors. |
| Post-call tasks | 7 | Accurate summaries, memories and name extraction; one summary attributed the caller's line to the assistant. |
| Following prose rules | 4 | Booked without the required read-back, used markdown bullets despite "never use markdown", demanded an email the rules called optional, narrated its plan aloud. |
| Date and state reasoning | 4 | Asked for Saturday Oct 10 as "Friday" twice despite an explicit dated weekday list; read "no Saturday slots" as "nothing free Friday"; read its own `booked` result as still pending. |
| Run-to-run consistency | 5 | Four clean booking passes, then two distinct new failure modes with no relevant code change. |

**What it did great.** Speed is the headline: it is what makes the 1.2 s turn latency possible. Tool calling is reliable at the wire level, and the model is steerable through the API surface: every rule moved from prose into a tool contract was followed on the next run.

**Where it is not so great.** Rules stated in the system prompt are followed loosely, date arithmetic is unreliable even with the dates spelled out, reasoning sometimes leaks into the spoken reply (a stray `</think>` with the plan in front of it, with thinking disabled), and tool results are occasionally misread. The variance is the part no guard removes; it is what would argue for a stronger model if the product went to real customers rather than a demo.

## Comparison with Claude

Nemotron answers about 4× faster at the model stage (0.5 s vs 2.1 s) and roughly 1 s faster per turn as the caller hears it; Claude Opus 5.5 and Sonnet 5.5 made zero rule violations out of the box where Nemotron needed server-side guards. Measured on 2026-10-07 by switching the profile's model (the provider is set by `LLM_PROVIDER`; the Anthropic API sits outside Nebius) and running the same three scenarios on the same VM, prompts and guards within one hour.

| Caller-perceived (12 turns, 3 greetings each) | Nemotron 3 Super 120B-A12B | Claude Sonnet 5.5 | Claude Opus 5.5 |
| --- | --- | --- | --- |
| Turn, median | 1.17 s | 2.04 s | 2.15 s |
| Turn, p95 | 1.39 s | 3.12 s | 2.90 s |
| Greeting, new caller | 1.83 s | 2.4–2.8 s | 2.9–3.0 s |
| Scenarios passed | 3/3 | 3/3 (one rerun) | 3/3 |

| Server side, medians | Nemotron | Sonnet 5.5 | Opus 5.5 |
| --- | --- | --- | --- |
| Model time per turn | 505 ms | 2142 ms | 2057 ms |
| Model time, greeting | 260 ms | 0.8–1.9 s | 1.1–1.3 s |
| Output tokens per turn | 39 | 76 | 107 |
| Input tokens per turn | 2383 | 3860 | 3852 |
| ElevenLabs TTS | 441 ms | 423 ms | 413 ms |

**Quality.** Both Claude models got the date right on the first call (`2026-10-09`, `start_weekday: Friday`), called `suggest_meeting_slots` once, took the `needs_confirmation` result into a read-back, booked on the caller's yes and ended the call with `end_call`. No markdown, no narration, no reasoning leak, no email demand, no slot-by-slot probing: every guard built for Nemotron sat idle. Opus was the most careful; it flagged an ambiguity in the script that Nemotron never noticed ("I handle Dmitrii's calendar; did you mean a meeting with Dmitrii, or is Jeremy joining?"), summarised the free slots as a range instead of a list, and added an event description unprompted, at the cost of 107 output tokens per turn, about 2.7× Nemotron, which is most of its extra second per turn. Sonnet sat between the two: precise, brief, correct on the first attempt; its first run of the returning-caller scenario ended with the stream closed before the script's last line, consistent with calling `end_call` one turn early, and passed on rerun.

**Reading.** Claude 5.5 is more reliable by a wide margin (about 9/10 against Nemotron's 6) but not faster, and it is not served on Nebius. With the server-side contracts in place, Nemotron's outcomes match Claude's on what matters (right day, consent before booking, one event), so the trade behind this build holds up: accept lower raw reliability, enforce it in code, keep the 0.5-second model inside the region.

## Nebius platform capabilities

Token Factory gave us a 120B NVIDIA model at 0.5 s per turn through an OpenAI-compatible regional endpoint; a single CPU VM in the same region, provisioned with cloud-init through the Nebius API, hosts the service behind auto-TLS, so the entire real-time path (caller audio in, Nemotron, speech out) stays inside Nebius us-central1. Ranked by how much each mattered:

| Capability | How it was used | Why it mattered |
| --- | --- | --- |
| Token Factory | Serves `nvidia/nemotron-3-super-120b-a12b` behind a plain OpenAI-compatible endpoint at `api.tokenfactory.us-central1.nebius.com`; honours `chat_template_kwargs.enable_thinking=false`. | The product's brain. The existing OpenAI-compatible provider worked with a base URL and key change; the regional endpoint keeps the model stage at ~0.5 s median and the greeting's model call at ~260 ms; the thinking toggle is what makes Nemotron 3 usable for voice at all. Steady across ~15 e2e runs and ~40 tool calls, no rate-limit or availability hiccups. |
| Compute VM | One `cpu-d3` 4 vCPU / 16 GB instance (`steve-assist-nb`, us-central1) runs the Rust server, the dashboard and Caddy via Docker Compose; public IP with Caddy auto-TLS for Twilio's HTTPS/WSS webhook. | Hosts everything else, and the only deployment since Cloud Run was retired. No GPU needed because inference is Token Factory's job, which keeps the VM cheap and simple. |
| Nebius API / CLI | The VM was provisioned with cloud-init user data through the API (installs Docker, clones the repo, creates the user); the CLI and its MCP integration were used to inspect the tenant, nine regional projects, the instance spec and networking. | Reproducible environment from day one; operations questions ("what is running and where") answered in minutes without the console. The application deploy itself is git + SSH + Docker Compose (`deploy/nebius/redeploy.sh`), not an API call; scripting `nebius compute instance create` from the repo is the next step. |

Not used: Object Storage (the two buckets belong to other projects), managed Kubernetes, managed Postgres. Firestore on GCP remains the datastore for callers, sessions and memories, and GCP Secret Manager holds a few secrets, so the split is Nebius for compute and inference, GCP for data.

## Approach

Nemotron is used out of the box: no fine-tuning, no custom weights. Reliability came from three layers, most important first.

**1. Server-side contracts and audio post-processing (where reliability came from).**

- Weekday guard: every calendar tool takes `start_weekday`; `book_meeting` refuses a date on the wrong day, and the read-only tools move the window to the asked day and say so.
- Two-phase booking: the first `book_meeting` call returns `needs_confirmation`; the booking proceeds only when the conversation history shows that proposal followed by something the caller said. The `caller_confirmed` flag alone is never trusted. A repeat call for a slot already booked in the call answers `already_booked` instead of creating a second event.
- Out of tool rounds: a turn that exhausts its tool budget ends with a text-only completion over the results gathered, never silence.
- Speech sanitising before TTS: strip `<think>` blocks and a stray `</think>`, markdown, bullets (`-`, `•`, `1.`), time ranges spoken as "to", verbal hang-up markers.
- Hang-up fallbacks, because the model sometimes ends a call in words instead of calling `end_call`.

**2. Prompt engineering (good for tone and context, weak for rules).** A system prompt composed per call: base persona with a configurable agent name, the current date and time in the profile's time zone, a 14-day weekday-to-date list so the model reads dates instead of computing them, a caller-identity layer (new or returning, with memories from past calls), calendar context (working hours, rules), `end_call` instructions and a "spoken only, never narrate" block. Tool results are written to be read aloud (`"when": "Friday, October 9, 2026, 9:30 AM"` beside the ISO timestamp) and each carries a `next_step` sentence saying what to tell the caller. This layer shaped how Steve talks; it did not reliably control what Steve must not do.

**3. API configuration (small but essential).** `chat_template_kwargs.enable_thinking: false`, because Nemotron 3 otherwise spends the whole token budget on `reasoning_content` and returns empty text to TTS. `max_tokens: 300` per profile as a hard cap on spoken length. The standard OpenAI-compatible wire format, so the tool loop has no Nemotron-specific code; only the provider adapter knows about the thinking flag and `<think>` tags.

**The feedback loop.** `steve-e2e` places scripted calls over WebSocket against the live VM, measures greeting and per-turn latency, and asserts outcomes in Firestore (profile created, booking recorded, greeting content). Each guard above was added in response to a specific failure found in a transcript or the VM's tool-call log, not anticipated up front.

**Why not fine-tune.** The failures were occasional lapses in rule-following and date arithmetic, not gaps in knowledge or style. A fine-tune would reduce them; a guard in code removes them for every future call, costs no training data, and under hackathon time was the only option with a deterministic result.

## What we learned

Treat the model as a fast, fluent, slightly unreliable component inside a system of server-side guarantees, not as the system. Every behaviour that matters (right day, caller's consent, no double booking, no reasoning or markdown read aloud, always say something when a turn ends) now lives in code where it is deterministic, and the model is free to be fast inside those rails. Nemotron 3 Super gave 8/10 speech and tool mechanics with 4/10 adherence to prose rules; moving the hard guarantees into tool contracts turned a 6/10 model into a reliable agent without giving up its 0.5-second responses.

Three specific lessons:

- Rules belong in the tool contract, not the prompt. A refusal or a two-step protocol was followed every time; the same rule in prose was skipped in at least one run out of five.
- Measure repeatedly. One successful demo call proves little with this much run-to-run variance; the automated end-to-end suite, run after every change, is what found each gap.
- Instrument the pipeline. Per-stage timings in the server log (`setup_ms`, `llm_ms`, `tts_ms`) showed the greeting was slow because of sequential database reads, not the model, and turned a guess into a one-line fix.

**Next.** Trim tool-turn context (4–7k tokens today) and reply length to bring tool-calling turns under 2 s; compare Nemotron variants on the same scenarios now that the model is a per-call profile setting; run the suite over a real Twilio leg to measure the PSTN share of the latency.
