use base64::{Engine as _, engine::general_purpose::STANDARD};

pub fn decode_payload(base64_str: &str) -> Vec<u8> {
    STANDARD.decode(base64_str).unwrap_or_default()
}

pub fn encode_payload(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Split raw mu-law audio into chunks suitable for Twilio media events.
/// Each chunk is ~20ms of audio at 8kHz mono (160 bytes).
pub fn chunk_audio(audio: &[u8]) -> Vec<&[u8]> {
    const CHUNK_SIZE: usize = 160;
    audio.chunks(CHUNK_SIZE).collect()
}
