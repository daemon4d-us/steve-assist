# Bluetooth headset bridge — plan

Goal: the Linux laptop pairs with the phone as a Bluetooth hands-free unit
(HFP HF role). When a call is active on the phone, its audio is routed to the
laptop, which runs the call through Steve (STT → LLM → ElevenLabs) and speaks
the reply back into the call.

## Architecture

```
Phone (HFP Audio Gateway)
   │  Bluetooth SCO link, 8 kHz CVSD or 16 kHz mSBC
   ▼
BlueZ + oFono (HFP HF role: RING, +CLIP caller id, ATA answer, hangup)
   │  org.ofono.VoiceCallManager on D-Bus        PipeWire (oFono backend) carries SCO audio
   │                                              bluez source (far-end)  ▲ bluez sink (to caller)
   ▼                                              ▼                       │
steve-bridge  (new binary, src/bin/steve_bridge.rs; zbus for oFono, pw-record/pw-play for audio)
   │  speaks the Twilio Media Streams protocol over WebSocket
   ▼
Steve server  /media-stream   (unchanged conversation loop)
   ├── Deepgram (mulaw 8 kHz)  ├── LLM (Claude locally, Nemotron on Nebius)
   ├── ElevenLabs (ulaw 8 kHz) └── Firestore sessions / memories / summary
```

Key decision: the bridge impersonates a Twilio media stream instead of adding a
second audio path to the server. The whole loop (caller lookup, greeting, tool
calls, barge-in `clear`, end-of-call summary and memory extraction) is reused
with no server changes. `src/bin/steve_e2e/caller.rs::DirectWsCaller` already
implements the client side of the protocol and is the starting point.

## Work items

### 1. Laptop HFP validation with oFono (no code)
Phone is a Pixel 10 (stock Android): no headset auto-answer exists, and the
PipeWire 1.0.5 native backend (Ubuntu 24.04) swallows RING/+CLIP and offers no
answer API (that arrived in PipeWire 1.4). oFono provides both, so it is the
v1 path, not an upgrade.
- `sudo apt install ofono`; enable `ofono.service`.
- D-Bus policy: `/etc/dbus-1/system.d/ofono.conf` allows only root by default;
  add a `<policy user="dsidorenko">` block (or group) so the bridge and
  PipeWire can talk to `org.ofono`.
- WirePlumber: `~/.config/wireplumber/wireplumber.conf.d/50-bt.conf` with
  `bluez5.hfphsp-backend = "ofono"`, `bluez5.roles = [ hfp_hf a2dp_sink ]`,
  `bluez5.enable-msbc = true`; restart pipewire/wireplumber.
- Pair the Pixel; confirm `busctl tree org.ofono` shows a modem for it and the
  card profile `headset-head-unit` is selectable.
- Place a test call to the Pixel: watch `busctl monitor org.ofono` for
  `CallAdded` with `LineIdentification`, answer with
  `busctl call org.ofono /hfp/... org.ofono.VoiceCall Answer`, and verify
  audio both ways with `pw-record` / `pw-play` on the bluez nodes.
- If oFono's HFP HF plugin misbehaves with the Pixel (known to be finicky with
  some phones), fallback is upgrading PipeWire to 1.4+ for its native
  telephony D-Bus API, which is modeled after oFono so the bridge code stays
  the same shape.

### 2. `steve-bridge` binary
- Move `DirectWsCaller` into the library (`src/twilio/client.rs`) and share it
  between the e2e harness and the bridge.
- Call control via `zbus`: subscribe to `org.ofono.VoiceCallManager.CallAdded`
  / `CallRemoved`; on an incoming call read `LineIdentification` (caller
  number → real Firestore caller lookup, memories, name confirmation) and call
  `Answer()` after a configurable ring delay; `Hangup()` when the server ends
  the conversation. Wait for the bluez SCO source node to become `running`
  before streaming.
- Capture: spawn `pw-record` on the bluez source at 8 kHz s16 mono, encode to
  G.711 μ-law (new ~20-line encoder in `src/audio/`, unit-tested against the
  existing decoder), send 20 ms / 160-byte `media` frames.
- Playback: `media` events → μ-law decode → `pw-play` stdin on the bluez sink.
  `clear` → kill and respawn `pw-play` so queued TTS is dropped (barge-in).
- Lifecycle: send `start` with `callSid = bt-<uuid>`, `streamSid`, and
  `customParameters { callerPhone: <LineIdentification>, source: "bluetooth" }`;
  send `stop` on `CallRemoved` so the server runs summary + memory extraction
  as usual.
- Policy hooks (this is where "programmable" lives): allow/deny lists, quiet
  hours, "answer only if I don't pick up within N rings", let the human take
  over by picking up on the phone (bridge sees the audio route change and sends
  `stop`).
- Loopback dev mode: `--source default --sink default` uses the laptop mic and
  speakers so the bridge can be developed without the phone.
- Config: `STEVE_WS_URL` (ws://localhost:8080 or wss://steve-nb...), caller
  phone override for v1, node name patterns.

### 3. Server tweaks (small)
- Optional `MEDIA_STREAM_TOKEN`: when set, a `start` whose customParameters
  carry `source=bluetooth` must include a matching `token`, otherwise close.
  Needed only when the bridge talks to the public Nebius endpoint.
- Log `source` so bridge calls are distinguishable in traces and Firestore
  (`call_sessions.direction` could become `"bluetooth"`).

### 4. Run modes
- Dev: `cargo run` locally with `LLM_PROVIDER=claude`, bridge on ws://localhost.
- Daily use: bridge → wss://steve-nb.creativecaptains.com with the token.

### 5. Complementary path without the laptop: carrier call forwarding
The Twilio path already delivers the full Steve feature set for any caller.
Setting conditional forwarding on the Pixel (busy / no answer / unreachable →
the Twilio 855 number) makes Steve answer the personal number anywhere, with
no laptop nearby. Twilio passes the original caller in `From` and the personal
number in `ForwardedFrom`. The Bluetooth bridge covers the "laptop is next to
the phone" case and gives Steve the ability to answer immediately and to let
the human take over mid-call; forwarding covers everything else. Both use the
same server, sessions, memories, and calendar tools.

## Risks and unknowns
- Phone HFP quirks: some phones tear down SCO when the HF is silent; keep the
  playback stream open (PipeWire feeds silence) for the whole call.
- Adapter SCO support: some Intel/USB adapters have broken SCO routing; check
  `dmesg` and `pw-top` during item 1 before writing code.
- Wi-Fi/Bluetooth coexistence on the laptop may add dropouts; mSBC is more
  robust than CVSD.
- Audio quality is telephone-grade either way (8 kHz), same as the Twilio path.
- Latency: Bluetooth adds ~50–100 ms per direction on top of the existing loop;
  remote Nebius adds RTT. Acceptable for conversation.
- The bridge makes the laptop the "headset"; the phone's own mic and speaker
  are muted for the call, so nobody in the room hears it unless monitored.

## Milestones
1. oFono + PipeWire oFono backend validated manually: caller id seen, call
   answered from busctl, audio both ways with pw-record / pw-play (half day;
   the riskiest step, do it first).
2. Bridge in loopback mode against a local server (half day).
3. Bridge with oFono call control over Bluetooth, end-to-end call with real
   caller id (half day).
4. Token auth + run against Nebius (hour).
5. Policy hooks and human takeover (day).

## Milestone 1 results (2026-09-16)

Verified end to end with Steve calling the Pixel 10 Pro XL from the Nebius
deployment (Twilio 855 number) and the laptop acting as the headset:

- oFono sees the incoming call with `LineIdentification` = the Twilio number.
- `org.ofono.VoiceCall.Answer` answers it (call state changes ~1 s later).
- Far-end audio recorded from PipeWire at 8 kHz s16 transcribes at 0.99
  confidence ("Hi, Dimitri. It's Susan calling from the Nevius deployment...").
- Audio played into the call reaches Steve: its Firestore transcript shows user
  turns that are the laptop's playback, degraded but recognizable after the
  double 8 kHz hop.

Setup facts learned (all encoded in `deploy/laptop/`):
- Start order matters: PipeWire's native backend owns the HFP profile until
  WirePlumber is restarted with the oFono backend; start/restart oFono after.
- **CVSD only.** With mSBC offered, the Pixel picks it and the Intel AX211
  (no `wideband-speech` in `bluetoothctl show` settings) cannot bring up the
  transparent eSCO link; PipeWire then tears the nodes down at once. Codec
  lists are exchanged at SLC time, so reconnect the phone after changing it.
- PipeWire exposes the phone's SCO audio as two *stream* nodes that exist only
  while SCO is up: `bluez_input.<addr>.0` (far-end voice, Stream/Output/Audio,
  auto-linked to the default sink) and `bluez_output.<addr>.1`
  (Stream/Input/Audio, into the call). `pactl list sources/sinks` does not
  show them; use `pw-dump` (decode concatenated JSON documents) and target
  them by name with `pw-record`/`pw-play --target`. The card's only profile is
  `audio-gateway`, which is correct for a phone.
- Android sends in-band ringing over SCO, so the nodes can appear while the
  call is still `incoming`.
- `getsockopt(SCO_OPTIONS) ... Transport endpoint is not connected` at SCO
  start and `error on SCO socket: Resource temporarily unavailable` at hangup
  are benign; audio flows between them.
- `busctl monitor` needs root; `dbus-monitor` with signal match rules works as
  the user (use `/usr/bin/dbus-monitor`, the conda one points at a wrong
  socket).
- Pairing: the Pixel initiates; the laptop-side agent must confirm the passkey
  promptly or the phone cancels. `bluetoothctl --agent NoInputNoOutput` in the
  background makes it Just Works.
- `answer-test.sh` is the reference for the bridge's call loop: poll
  `GetCalls`, read caller id, `Answer`, wait for the two nodes, stream.

Observed on Steve's side, unrelated to the bridge: Nemotron sometimes writes
the user's next line into its own reply ("Dmitrii: Hi Susan, ..."), which TTS
then speaks. Worth a prompt/stop-sequence fix in `assistant.rs`.

## Milestone 2 results (2026-09-16): `steve-bridge` binary

`src/bin/steve_bridge/` (main.rs, ofono.rs, audio.rs, session.rs). Build with
`cargo build --bin steve-bridge`; config in `deploy/laptop/bridge.env.example`.

- **Phone mode** (`steve-bridge`): watches `org.ofono.VoiceCallManager` on the
  HFP modem via zbus; on an incoming call waits `BRIDGE_ANSWER_DELAY_MS`
  (human can pick up first), answers, waits for the SCO stream nodes, breaks
  WirePlumber's auto links (laptop mic → call, call → speakers unless
  `BRIDGE_MONITOR=1`), then runs a Twilio-Media-Streams session: `pw-record`
  on `bluez_input.*` produces μ-law 8 kHz frames (pw-cat encodes natively, no
  G.711 code needed), `pw-play` on `bluez_output.*` plays Steve's μ-law; `clear`
  restarts pw-play to drop queued audio (barge-in). `CallRemoved` sends `stop`.
- **Server selection**: `STEVE_LOCAL_WS_URL` (ws://127.0.0.1:8080) is tried
  with a 1.5 s timeout, then `STEVE_REMOTE_WS_URL` (Nebius). Both verified.
- **Loopback mode** (`steve-bridge loopback [phone]`): no phone; default mic and
  speakers, one synthetic call until Ctrl-C. Wear headphones or Steve hears
  itself.
- Verified end to end: scripted Twilio caller → Pixel → bridge → local server
  (Nemotron): perfect transcripts ("could you tell me a short joke please"),
  joke told, summary + memories saved, 1484 frames in 30 s (exact 20 ms rate).
  Loopback → Nebius fallback also verified (1203 frames / 24 s).
- Not done yet: server-side `MEDIA_STREAM_TOKEN` check (plan item 3), policy
  hooks, human takeover mid-call. A local server run needs
  `TWILIO_PHONE_NUMBER` and, while the Firestore profile pins a Nemotron model
  id, `LLM_PROVIDER=nemotron` (+ `NEBIUS_API_KEY`, `LLM_BASE_URL`); with the
  Claude provider the profile model must be changed to a Claude id first. The
  local server also runs the outbound scheduler against the shared Firestore,
  so two servers can race on pending assignments.

## Outgoing calls (2026-09-16)

Steve can now be on calls the phone places, not only ones it receives:

- `steve-bridge dial <number> <objective...>` asks the phone to dial via
  `org.ofono.VoiceCallManager.Dial`, waits for the call to go `active`, then
  runs the usual session with `customParameters { objective, contactPhone,
  direction: "outgoing" }`. The server treats a start event with an
  `objective` like a scheduler assignment (outbound prompt, outbound greeting,
  `direction = "outbound"`), so Steve pursues the objective with whoever
  answers. This places calls from the personal number with no Twilio leg.
- Calls the user dials on the phone are left alone unless
  `BRIDGE_OBJECTIVE_FILE` (default `~/.config/steve-bridge/objective.txt`)
  holds an objective; then Steve joins that call and the file is renamed to
  `.used`, so only that one call is taken over.
- Incoming handling is unchanged (answer after `BRIDGE_ANSWER_DELAY_MS`,
  unless the human picked up first).
