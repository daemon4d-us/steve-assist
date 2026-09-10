use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::Mutex;

use crate::services::{assistant, calendar_tools, db, elevenlabs, llm, speech};
use crate::state::AppState;
use crate::twilio::{audio, dtmf, messages as twilio_msg};

pub async fn media_stream(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_connection(socket, state))
}

async fn handle_connection(socket: WebSocket, state: Arc<AppState>) {
    tracing::info!("Twilio WebSocket connected");

    let (twilio_sink, mut twilio_stream) = socket.split();
    let twilio_sink = Arc::new(Mutex::new(twilio_sink));

    let stream_sid: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let call_sid: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let caller_phone: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let caller_name: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let system_prompt: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let conversation: Arc<Mutex<Vec<llm::Message>>> = Arc::new(Mutex::new(Vec::new()));
    // True while a caller-name extraction is running, so unknown-caller turns
    // don't launch overlapping extractions.
    let name_lookup_inflight: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let voice_id: Arc<Mutex<String>> = Arc::new(Mutex::new(state.elevenlabs_voice_id.clone()));
    let is_outbound_call: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    // Create audio channel upfront
    let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    let (transcript_tx, mut transcript_rx) = tokio::sync::mpsc::channel::<String>(32);

    // Spawn Speech session setup in background
    let state_for_speech = state.clone();
    tokio::spawn(async move {
        if let Err(e) = speech::start_session(&state_for_speech, audio_rx, transcript_tx).await {
            tracing::error!("Failed to start Speech session: {e:?}");
        }
    });

    // Task A: Twilio → Google Speech (forward audio)
    let twilio_task = {
        let stream_sid = stream_sid.clone();
        let call_sid = call_sid.clone();
        let caller_phone = caller_phone.clone();
        let caller_name = caller_name.clone();
        let system_prompt = system_prompt.clone();
        let conversation = conversation.clone();
        let twilio_sink = twilio_sink.clone();
        let voice_id = voice_id.clone();
        let is_outbound_call = is_outbound_call.clone();
        let state = state.clone();
        async move {
            while let Some(Ok(msg)) = twilio_stream.next().await {
                let Message::Text(text) = msg else {
                    continue;
                };

                let Ok(event) = serde_json::from_str::<twilio_msg::TwilioInbound>(&text) else {
                    tracing::warn!("Failed to parse Twilio message");
                    continue;
                };

                match event {
                    twilio_msg::TwilioInbound::Connected { .. } => {
                        tracing::info!("Twilio media stream connected");
                    }
                    twilio_msg::TwilioInbound::Start {
                        stream_sid: sid,
                        start,
                    } => {
                        tracing::info!("Stream started: call_sid={}", start.call_sid);
                        *stream_sid.lock().await = Some(sid);
                        *call_sid.lock().await = Some(start.call_sid.clone());

                        // Check if this is an outbound call (assignment)
                        let is_outbound = start.custom_parameters.contains_key("assignmentId");
                        *is_outbound_call.lock().await = is_outbound;

                        if is_outbound {
                            let objective = start
                                .custom_parameters
                                .get("objective")
                                .cloned()
                                .unwrap_or_default();
                            let contact_name_val = start
                                .custom_parameters
                                .get("contactName")
                                .cloned()
                                .unwrap_or_default();
                            let contact_phone = start
                                .custom_parameters
                                .get("contactPhone")
                                .cloned()
                                .unwrap_or_default();
                            let profile_id = start
                                .custom_parameters
                                .get("profileId")
                                .cloned()
                                .unwrap_or_else(|| "default".to_string());
                            let assignment_id = start
                                .custom_parameters
                                .get("assignmentId")
                                .cloned()
                                .unwrap_or_default();
                            let contact_index: usize = start
                                .custom_parameters
                                .get("contactIndex")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);

                            tracing::info!(
                                "Outbound call: assignment={assignment_id} contact={contact_phone} objective={objective}"
                            );

                            *caller_phone.lock().await = Some(contact_phone.clone());
                            if !contact_name_val.is_empty() {
                                *caller_name.lock().await = Some(contact_name_val.clone());
                            }

                            // Load the assignment's agent profile
                            let profile = db::get_agent_profile(&state.firestore_db, &profile_id)
                                .await
                                .ok()
                                .flatten()
                                .unwrap_or_else(|| state.agent_profile.clone());

                            if let Some(v) = profile.voice_id.as_deref().filter(|s| !s.is_empty()) {
                                *voice_id.lock().await = v.to_string();
                            }

                            let cn = if contact_name_val.is_empty() {
                                None
                            } else {
                                Some(contact_name_val.as_str())
                            };
                            let prompt = assistant::build_outbound_prompt(
                                &profile,
                                &objective,
                                cn,
                                &contact_phone,
                            );
                            *system_prompt.lock().await = prompt;

                            // Create session record
                            let db = state.firestore_db.clone();
                            let csid = start.call_sid.clone();
                            let ph = contact_phone.clone();
                            tokio::spawn(async move {
                                if let Err(e) =
                                    db::create_session(&db, &csid, &ph, "outbound").await
                                {
                                    tracing::error!("Failed to create session: {e}");
                                }
                            });

                            // Generate and send outbound greeting
                            let greeting_started = std::time::Instant::now();
                            let greeting_prompt = system_prompt.lock().await.clone();
                            let greeting_messages = vec![llm::Message::user_text(format!(
                                "[You just called {contact_phone}. They picked up. Introduce yourself and work toward your objective: {objective}]"
                            ))];
                            match assistant::respond(
                                state.llm.as_ref(),
                                &greeting_prompt,
                                &greeting_messages,
                                &profile,
                            )
                            .await
                            {
                                Ok(greeting) => {
                                    let greeting_llm_ms = greeting_started.elapsed().as_millis();
                                    tracing::info!("Outbound greeting: {greeting}");
                                    {
                                        let db = state.firestore_db.clone();
                                        let csid = start.call_sid.clone();
                                        let g = greeting.clone();
                                        tokio::spawn(async move {
                                            if let Err(e) =
                                                db::save_message(&db, &csid, "assistant", &g).await
                                            {
                                                tracing::error!(
                                                    "Failed to save greeting message: {e}"
                                                );
                                            }
                                        });
                                    }
                                    conversation
                                        .lock()
                                        .await
                                        .push(llm::Message::assistant_text(greeting.clone()));

                                    let active_voice = voice_id.lock().await.clone();
                                    match elevenlabs::text_to_speech(
                                        &state.elevenlabs_api_key,
                                        &active_voice,
                                        &greeting,
                                    )
                                    .await
                                    {
                                        Ok(tts_audio) => {
                                            tracing::info!(
                                                target: "steve_assist::metrics",
                                                "greeting call_sid={} llm_ms={} tts_ms={} audio_ms={}",
                                                start.call_sid,
                                                greeting_llm_ms,
                                                greeting_started.elapsed().as_millis() - greeting_llm_ms,
                                                tts_audio.len() / 8
                                            );
                                            if let Some(ref sid_val) = *stream_sid.lock().await {
                                                let mut sink = twilio_sink.lock().await;
                                                for chunk in audio::chunk_audio(&tts_audio) {
                                                    let payload = audio::encode_payload(chunk);
                                                    let media = twilio_msg::OutboundMedia::new(
                                                        sid_val.clone(),
                                                        payload,
                                                    );
                                                    let json =
                                                        serde_json::to_string(&media).unwrap();
                                                    if let Err(e) =
                                                        sink.send(Message::Text(json.into())).await
                                                    {
                                                        tracing::error!(
                                                            "Failed to send outbound greeting audio: {e}"
                                                        );
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::error!("Outbound greeting TTS error: {e}")
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Outbound greeting generation error: {e}")
                                }
                            }
                        }

                        // Extract caller phone from custom parameters (inbound calls)
                        let phone = start
                            .custom_parameters
                            .get("callerPhone")
                            .cloned()
                            .unwrap_or_default();

                        if !is_outbound && !phone.is_empty() {
                            tracing::info!("Caller phone: {phone}");
                            *caller_phone.lock().await = Some(phone.clone());

                            // Refresh voice_id from the latest agent profile in Firestore
                            // (lets the dashboard change the voice without a backend restart)
                            if let Ok(Some(latest_profile)) =
                                db::get_agent_profile(&state.firestore_db, &state.agent_profile_id)
                                    .await
                            {
                                if let Some(v) =
                                    latest_profile.voice_id.as_deref().filter(|s| !s.is_empty())
                                {
                                    *voice_id.lock().await = v.to_string();
                                }
                            }

                            // Check if calendar tools are available (OAuth configured + tokens stored)
                            let calendar_tools_available =
                                !calendar_tools::available_tools(&state).await.is_empty();
                            let calendar_ctx = calendar_tools::build_calendar_context(
                                &state.agent_profile,
                                calendar_tools_available,
                            );

                            // Look up caller in Firestore
                            match db::get_caller(&state.firestore_db, &phone).await {
                                Ok(Some(profile)) => {
                                    tracing::info!("Returning caller: {}", profile.name);
                                    *caller_name.lock().await = Some(profile.name.clone());

                                    // Load memories
                                    let memories = db::get_memories(
                                        &state.firestore_db,
                                        &phone,
                                        state.memory_depth,
                                    )
                                    .await
                                    .unwrap_or_default();

                                    let mut prompt = assistant::build_system_prompt(
                                        &state.agent_profile,
                                        &phone,
                                        Some(&profile.name),
                                        &memories,
                                    );
                                    prompt.push_str(&calendar_ctx);
                                    *system_prompt.lock().await = prompt;
                                }
                                Ok(None) => {
                                    tracing::info!("New caller from {phone}");
                                    let mut prompt = assistant::build_system_prompt(
                                        &state.agent_profile,
                                        &phone,
                                        None,
                                        &[],
                                    );
                                    prompt.push_str(&calendar_ctx);
                                    *system_prompt.lock().await = prompt;
                                }
                                Err(e) => {
                                    tracing::error!("Firestore caller lookup failed: {e}");
                                    let mut prompt = assistant::build_system_prompt(
                                        &state.agent_profile,
                                        &phone,
                                        None,
                                        &[],
                                    );
                                    prompt.push_str(&calendar_ctx);
                                    *system_prompt.lock().await = prompt;
                                }
                            }

                            // Create session record (fire-and-forget)
                            let db = state.firestore_db.clone();
                            let csid = start.call_sid.clone();
                            let ph = phone.clone();
                            tokio::spawn(async move {
                                if let Err(e) = db::create_session(&db, &csid, &ph, "inbound").await
                                {
                                    tracing::error!("Failed to create session: {e}");
                                }
                            });

                            // Generate and send opening greeting
                            let greeting_started = std::time::Instant::now();
                            let greeting_prompt = system_prompt.lock().await.clone();
                            let greeting_messages = vec![llm::Message::user_text(
                                "[The phone is ringing and the caller just picked up. Greet them.]",
                            )];
                            match assistant::respond(
                                state.llm.as_ref(),
                                &greeting_prompt,
                                &greeting_messages,
                                &state.agent_profile,
                            )
                            .await
                            {
                                Ok(greeting) => {
                                    let greeting_llm_ms = greeting_started.elapsed().as_millis();
                                    tracing::info!("Opening greeting: {greeting}");

                                    // Add to conversation history
                                    conversation
                                        .lock()
                                        .await
                                        .push(llm::Message::assistant_text(greeting.clone()));

                                    // Persist the greeting as the first assistant
                                    // turn so it appears in the transcript.
                                    {
                                        let db = state.firestore_db.clone();
                                        let csid = start.call_sid.clone();
                                        let g = greeting.clone();
                                        tokio::spawn(async move {
                                            if let Err(e) =
                                                db::save_message(&db, &csid, "assistant", &g).await
                                            {
                                                tracing::error!(
                                                    "Failed to save greeting message: {e}"
                                                );
                                            }
                                        });
                                    }

                                    // Convert to speech and send
                                    let active_voice = voice_id.lock().await.clone();
                                    match elevenlabs::text_to_speech(
                                        &state.elevenlabs_api_key,
                                        &active_voice,
                                        &greeting,
                                    )
                                    .await
                                    {
                                        Ok(tts_audio) => {
                                            tracing::info!(
                                                target: "steve_assist::metrics",
                                                "greeting call_sid={} llm_ms={} tts_ms={} audio_ms={}",
                                                start.call_sid,
                                                greeting_llm_ms,
                                                greeting_started.elapsed().as_millis() - greeting_llm_ms,
                                                tts_audio.len() / 8
                                            );
                                            if let Some(ref sid_val) = *stream_sid.lock().await {
                                                let mut sink = twilio_sink.lock().await;
                                                for chunk in audio::chunk_audio(&tts_audio) {
                                                    let payload = audio::encode_payload(chunk);
                                                    let media = twilio_msg::OutboundMedia::new(
                                                        sid_val.clone(),
                                                        payload,
                                                    );
                                                    let json =
                                                        serde_json::to_string(&media).unwrap();
                                                    if let Err(e) =
                                                        sink.send(Message::Text(json.into())).await
                                                    {
                                                        tracing::error!(
                                                            "Failed to send greeting audio: {e}"
                                                        );
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => tracing::error!("Greeting TTS error: {e}"),
                                    }
                                }
                                Err(e) => tracing::error!("Greeting generation error: {e}"),
                            }
                        }
                    }
                    twilio_msg::TwilioInbound::Media { media, .. } => {
                        let raw = audio::decode_payload(&media.payload);
                        if audio_tx.send(raw).await.is_err() {
                            tracing::error!("Speech audio channel closed");
                            break;
                        }
                    }
                    twilio_msg::TwilioInbound::Stop { .. } => {
                        tracing::info!("Twilio stream stopped");
                        break;
                    }
                    twilio_msg::TwilioInbound::Unknown => {}
                }
            }
        }
    };

    // Task B: Speech transcripts → LLM → ElevenLabs → Twilio
    let speech_task = {
        let twilio_sink = twilio_sink.clone();
        let stream_sid = stream_sid.clone();
        let call_sid = call_sid.clone();
        let caller_phone = caller_phone.clone();
        let caller_name = caller_name.clone();
        let system_prompt = system_prompt.clone();
        let conversation = conversation.clone();
        let name_lookup_inflight = name_lookup_inflight.clone();
        let voice_id = voice_id.clone();
        let state = state.clone();
        async move {
            let mut turn_index: u32 = 0;
            while let Some(transcript) = transcript_rx.recv().await {
                let turn_started = std::time::Instant::now();
                turn_index += 1;
                let current_prompt = system_prompt.lock().await.clone();
                let csid = call_sid.lock().await.clone().unwrap_or_default();

                // Add user message to conversation
                {
                    let mut conv = conversation.lock().await;
                    conv.push(llm::Message::user_text(transcript.clone()));
                }

                // Save user message (fire-and-forget)
                {
                    let db = state.firestore_db.clone();
                    let csid = csid.clone();
                    let t = transcript.clone();
                    tokio::spawn(async move {
                        if let Err(e) = db::save_message(&db, &csid, "user", &t).await {
                            tracing::error!("Failed to save user message: {e}");
                        }
                    });
                }

                // For an unknown caller, try to extract a name from each turn until
                // one is found. Callers often open with "hello" and give their name
                // a turn or two later, so this can't be limited to the first exchange.
                let unknown_caller = caller_name.lock().await.is_none();
                if unknown_caller {
                    let mut inflight = name_lookup_inflight.lock().await;
                    if !*inflight {
                        *inflight = true;
                        drop(inflight);
                        let llm = state.llm.clone();
                        let t = transcript.clone();
                        let cn = caller_name.clone();
                        let cp = caller_phone.clone();
                        let sp = system_prompt.clone();
                        let db = state.firestore_db.clone();
                        let md = state.memory_depth;
                        let profile = state.agent_profile.clone();
                        let inflight_flag = name_lookup_inflight.clone();
                        tokio::spawn(async move {
                            match assistant::extract_name(llm.as_ref(), &t).await {
                                Ok(Some(name)) => {
                                    tracing::info!("Extracted caller name: {name}");
                                    *cn.lock().await = Some(name.clone());

                                    let phone = cp.lock().await.clone().unwrap_or_default();
                                    if !phone.is_empty() {
                                        if let Err(e) = db::upsert_caller(&db, &phone, &name).await
                                        {
                                            tracing::error!("Failed to save caller: {e}");
                                        }

                                        let memories = db::get_memories(&db, &phone, md)
                                            .await
                                            .unwrap_or_default();
                                        let new_prompt = assistant::build_system_prompt(
                                            &profile,
                                            &phone,
                                            Some(&name),
                                            &memories,
                                        );
                                        *sp.lock().await = new_prompt;
                                    }
                                    // Leave inflight = true: name found, no more attempts needed.
                                }
                                _ => {
                                    // No name this turn; allow the next turn to retry.
                                    *inflight_flag.lock().await = false;
                                }
                            }
                        });
                    }
                }

                // Tool-use loop: ask the LLM, execute any tool calls, loop until only text
                let tools = calendar_tools::available_tools(&state).await;
                let phone_for_tools = caller_phone.lock().await.clone().unwrap_or_default();

                let turn = match run_tool_loop(
                    &state,
                    &current_prompt,
                    &conversation,
                    &tools,
                    &csid,
                    &phone_for_tools,
                )
                .await
                {
                    Ok(t) => t,
                    Err(e) => {
                        tracing::error!("LLM tool loop error: {e}");
                        continue;
                    }
                };
                let llm_ms = turn_started.elapsed().as_millis();
                let ai_response = turn.text;

                if ai_response.is_empty() {
                    tracing::warn!("LLM returned empty final response");
                    continue;
                }

                tracing::info!("Assistant: {ai_response}");

                // Extract DTMF commands from response
                let (spoken_text, dtmf_commands) = dtmf::extract_dtmf_commands(&ai_response);

                if !dtmf_commands.is_empty() {
                    tracing::info!("DTMF commands detected: {:?}", dtmf_commands);
                }

                // Strip markdown before TTS — ElevenLabs reads asterisks, underscores,
                // hash marks, and list bullets literally.
                let spoken_text = strip_markdown(&spoken_text);

                // Save assistant message (fire-and-forget)
                {
                    let db = state.firestore_db.clone();
                    let csid = csid.clone();
                    let r = ai_response.clone();
                    tokio::spawn(async move {
                        if let Err(e) = db::save_message(&db, &csid, "assistant", &r).await {
                            tracing::error!("Failed to save assistant message: {e}");
                        }
                    });
                }

                let sid = stream_sid.lock().await.clone();
                let Some(sid) = sid else {
                    tracing::warn!("No stream_sid available, cannot send audio");
                    continue;
                };

                let mut sink = twilio_sink.lock().await;

                // Clear any buffered audio first
                let clear = twilio_msg::ClearMessage::new(sid.clone());
                let clear_json = serde_json::to_string(&clear).unwrap();
                if let Err(e) = sink.send(Message::Text(clear_json.into())).await {
                    tracing::error!("Failed to send clear: {e}");
                    break;
                }

                // Send spoken text as TTS if there's any
                let mut tts_ms: u128 = 0;
                let mut send_ms: u128 = 0;
                let mut audio_bytes: usize = 0;
                if !spoken_text.is_empty() {
                    let active_voice = voice_id.lock().await.clone();
                    let tts_started = std::time::Instant::now();
                    match elevenlabs::text_to_speech(
                        &state.elevenlabs_api_key,
                        &active_voice,
                        &spoken_text,
                    )
                    .await
                    {
                        Ok(tts_audio) => {
                            tts_ms = tts_started.elapsed().as_millis();
                            audio_bytes = tts_audio.len();
                            let send_started = std::time::Instant::now();
                            for chunk in audio::chunk_audio(&tts_audio) {
                                let payload = audio::encode_payload(chunk);
                                let media = twilio_msg::OutboundMedia::new(sid.clone(), payload);
                                let json = serde_json::to_string(&media).unwrap();
                                if let Err(e) = sink.send(Message::Text(json.into())).await {
                                    tracing::error!("Failed to send audio chunk: {e}");
                                    break;
                                }
                            }
                            send_ms = send_started.elapsed().as_millis();
                        }
                        Err(e) => {
                            tracing::error!("ElevenLabs TTS error: {e}");
                        }
                    }
                }

                tracing::info!(
                    target: "steve_assist::metrics",
                    "turn call_sid={csid} turn={turn_index} llm_ms={llm_ms} tts_ms={tts_ms} send_ms={send_ms} total_ms={} audio_ms={} in_tokens={} out_tokens={} tool_rounds={}",
                    turn_started.elapsed().as_millis(),
                    audio_bytes / 8,
                    turn.input_tokens,
                    turn.output_tokens,
                    turn.tool_rounds
                );

                // Send DTMF tones
                for keys in &dtmf_commands {
                    tracing::info!("Sending DTMF: {keys}");
                    let tone_audio = dtmf::generate_dtmf_sequence(keys);
                    for chunk in audio::chunk_audio(&tone_audio) {
                        let payload = audio::encode_payload(chunk);
                        let media = twilio_msg::OutboundMedia::new(sid.clone(), payload);
                        let json = serde_json::to_string(&media).unwrap();
                        if let Err(e) = sink.send(Message::Text(json.into())).await {
                            tracing::error!("Failed to send DTMF chunk: {e}");
                            break;
                        }
                    }
                }
            }
        }
    };

    tokio::select! {
        _ = twilio_task => tracing::info!("Twilio task ended"),
        _ = speech_task => tracing::info!("Speech task ended"),
    }

    // End-of-call processing
    let csid = call_sid.lock().await.clone();
    let phone = caller_phone.lock().await.clone();
    let name = caller_name.lock().await.clone();
    let conv = conversation.lock().await.clone();
    let was_outbound = *is_outbound_call.lock().await;

    if let (Some(csid), Some(phone)) = (csid, phone) {
        let state = state.clone();
        tokio::spawn(async move {
            tracing::info!("Starting end-of-call processing for {csid}");

            if conv.len() < 2 {
                // Too short to summarize
                if let Err(e) =
                    db::end_session(&state.firestore_db, &csid, name.as_deref(), None).await
                {
                    tracing::error!("Failed to end session: {e}");
                }
                return;
            }

            // Generate summary
            let summary = match assistant::generate_summary(state.llm.as_ref(), &conv).await {
                Ok(s) => {
                    tracing::info!("Call summary: {s}");
                    Some(s)
                }
                Err(e) => {
                    tracing::error!("Failed to generate summary: {e}");
                    None
                }
            };

            // End session with summary
            if let Err(e) = db::end_session(
                &state.firestore_db,
                &csid,
                name.as_deref(),
                summary.as_deref(),
            )
            .await
            {
                tracing::error!("Failed to end session: {e}");
            }

            // Extract and save memories
            match assistant::extract_memories(state.llm.as_ref(), &conv).await {
                Ok(facts) => {
                    for fact in facts {
                        if let Err(e) =
                            db::save_memory(&state.firestore_db, &phone, &csid, &fact).await
                        {
                            tracing::error!("Failed to save memory: {e}");
                        }
                    }
                    tracing::info!("Memories saved for {csid}");
                }
                Err(e) => {
                    tracing::error!("Failed to extract memories: {e}");
                }
            }

            // Update caller's last_seen
            if let Err(e) = db::update_caller_last_seen(&state.firestore_db, &phone).await {
                tracing::error!("Failed to update caller last_seen: {e}");
            }

            // Bot-call classification: only for inbound calls. Looks at the
            // conversation, decides if the caller was a recognizable bot, and
            // either creates a new bot group or appends to an existing one.
            if !was_outbound {
                classify_and_record_bot_call(&state, &csid, &phone, &conv, summary.as_deref())
                    .await;
            }
        });
    }

    tracing::info!("Call session ended");
}

/// Run the bot classifier against a finished call. If it identifies a known bot,
/// either append the caller phone to the matching group or create a new group.
/// Logs and swallows all errors — never propagates.
async fn classify_and_record_bot_call(
    state: &Arc<AppState>,
    call_sid: &str,
    phone: &str,
    conversation: &[llm::Message],
    summary: Option<&str>,
) {
    let existing = match db::list_bot_groups(&state.firestore_db).await {
        Ok(groups) => groups,
        Err(e) => {
            tracing::error!("Bot classify: failed to list existing groups: {e}");
            Vec::new()
        }
    };
    let id_name_pairs: Vec<(String, String)> = existing
        .iter()
        .map(|g| (g.id.clone(), g.name.clone()))
        .collect();

    let classification = match assistant::classify_bot_call(
        state.llm.as_ref(),
        conversation,
        &id_name_pairs,
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Bot classify: LLM call failed: {e}");
            return;
        }
    };

    let now = chrono::Utc::now();
    match classification {
        assistant::BotClassification::Skip => {
            tracing::info!("Bot classify: call {call_sid} skipped (human or unidentifiable bot)");
        }
        assistant::BotClassification::MatchExisting(group_id) => {
            let Some(mut group) = existing.into_iter().find(|g| g.id == group_id) else {
                tracing::warn!(
                    "Bot classify: classifier returned group_id={group_id} that no longer exists"
                );
                return;
            };
            if !group.phone_numbers.iter().any(|p| p == phone) {
                group.phone_numbers.push(phone.to_string());
            }
            if let Some(s) = summary {
                group.latest_summary = Some(s.to_string());
            }
            group.latest_call_sid = Some(call_sid.to_string());
            group.latest_call_at = now;
            group.call_count = group.call_count.saturating_add(1);
            let id = group.id.clone();
            if let Err(e) = db::update_bot_group(&state.firestore_db, &id, &group).await {
                tracing::error!("Bot classify: failed to update group {id}: {e}");
            } else {
                tracing::info!(
                    "Bot classify: appended {phone} to group '{}' ({id})",
                    group.name
                );
            }
        }
        assistant::BotClassification::NewGroup(name) => {
            let group = db::BotCallGroup {
                id: String::new(),
                name: name.clone(),
                phone_numbers: vec![phone.to_string()],
                latest_summary: summary.map(|s| s.to_string()),
                latest_call_sid: Some(call_sid.to_string()),
                latest_call_at: now,
                call_count: 1,
                created_at: now,
            };
            match db::create_bot_group(&state.firestore_db, &group).await {
                Ok(id) => {
                    tracing::info!("Bot classify: created new group '{name}' ({id}) with {phone}")
                }
                Err(e) => tracing::error!("Bot classify: failed to create group '{name}': {e}"),
            }
        }
    }
}

/// Strip markdown formatting that would be spoken literally by TTS.
/// Handles: **bold**, *italic*, _underscore_, `code`, # headings, - / * bullet lines.
fn strip_markdown(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let mut trimmed = line.trim_start();
        // Strip leading heading hashes
        while trimmed.starts_with('#') {
            trimmed = &trimmed[1..];
        }
        // Strip leading bullet markers
        if let Some(rest) = trimmed.strip_prefix("- ") {
            trimmed = rest;
        } else if let Some(rest) = trimmed.strip_prefix("* ") {
            trimmed = rest;
        } else if let Some(rest) = trimmed.strip_prefix("+ ") {
            trimmed = rest;
        }
        let trimmed = trimmed.trim_start();

        // Remove inline markers. We just drop *, _, `, and ~ characters —
        // acceptable for spoken output and much simpler than a real parser.
        for ch in trimmed.chars() {
            match ch {
                '*' | '_' | '`' | '~' => {}
                _ => out.push(ch),
            }
        }
        out.push('\n');
    }
    // Collapse trailing whitespace/newlines
    out.trim_end().to_string()
}

/// Result of one assistant turn: the spoken text plus accounting for metrics.
struct TurnOutput {
    text: String,
    input_tokens: u32,
    output_tokens: u32,
    tool_rounds: u32,
}

/// Run one assistant turn with tool-use support. Appends the final assistant
/// text to the shared conversation and returns it. Handles up to 4 tool-execution rounds.
async fn run_tool_loop(
    state: &Arc<AppState>,
    system_prompt: &str,
    conversation: &Arc<Mutex<Vec<llm::Message>>>,
    tools: &[llm::Tool],
    call_sid: &str,
    caller_phone: &str,
) -> Result<TurnOutput, Box<dyn std::error::Error + Send + Sync>> {
    const MAX_ROUNDS: usize = 4;
    let mut input_tokens = 0u32;
    let mut output_tokens = 0u32;

    let ctx = calendar_tools::ToolContext {
        state,
        profile: &state.agent_profile,
        call_sid,
        caller_phone,
    };

    for round in 0..MAX_ROUNDS {
        let conv_snapshot = conversation.lock().await.clone();

        let completion = assistant::respond_with_tools(
            state.llm.as_ref(),
            system_prompt,
            &conv_snapshot,
            &state.agent_profile,
            tools,
        )
        .await?;
        if let Some(u) = completion.usage {
            input_tokens += u.input_tokens;
            output_tokens += u.output_tokens;
        }

        if completion.tool_calls.is_empty() {
            // Terminal text response
            conversation
                .lock()
                .await
                .push(llm::Message::assistant_text(completion.text.clone()));
            return Ok(TurnOutput {
                text: completion.text,
                input_tokens,
                output_tokens,
                tool_rounds: round as u32,
            });
        }

        // Execute each tool, then record the request and its results in history
        let mut results = Vec::with_capacity(completion.tool_calls.len());
        for call in &completion.tool_calls {
            tracing::info!("Executing tool: {} input={}", call.name, call.input);
            let output = calendar_tools::dispatch(&ctx, &call.name, &call.input).await;
            results.push(llm::ToolResult {
                call_id: call.id.clone(),
                name: call.name.clone(),
                content: output.to_string(),
            });
        }

        let mut conv = conversation.lock().await;
        conv.push(llm::Message::ToolCalls {
            text: completion.text,
            calls: completion.tool_calls,
        });
        conv.push(llm::Message::ToolResults(results));
    }

    Err("Exceeded maximum tool-use rounds".into())
}
