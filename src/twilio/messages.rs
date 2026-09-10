use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
#[allow(dead_code)]
pub enum TwilioInbound {
    Connected {
        protocol: String,
        version: String,
    },
    Start {
        #[serde(rename = "streamSid")]
        stream_sid: String,
        start: StartMeta,
    },
    Media {
        #[serde(rename = "streamSid")]
        stream_sid: String,
        media: MediaPayload,
    },
    Stop {
        #[serde(rename = "streamSid")]
        stream_sid: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct StartMeta {
    pub call_sid: String,
    pub tracks: Vec<String>,
    #[serde(default)]
    pub custom_parameters: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct MediaPayload {
    pub payload: String,
}

#[derive(Debug, Serialize)]
pub struct OutboundMedia {
    pub event: &'static str,
    #[serde(rename = "streamSid")]
    pub stream_sid: String,
    pub media: OutboundMediaPayload,
}

#[derive(Debug, Serialize)]
pub struct OutboundMediaPayload {
    pub payload: String,
}

impl OutboundMedia {
    pub fn new(stream_sid: String, payload: String) -> Self {
        Self {
            event: "media",
            stream_sid,
            media: OutboundMediaPayload { payload },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ClearMessage {
    pub event: &'static str,
    #[serde(rename = "streamSid")]
    pub stream_sid: String,
}

impl ClearMessage {
    pub fn new(stream_sid: String) -> Self {
        Self {
            event: "clear",
            stream_sid,
        }
    }
}
