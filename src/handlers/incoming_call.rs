use std::collections::HashMap;

use axum::Form;
use axum::extract::Host;
use axum::http::header;
use axum::response::IntoResponse;

pub async fn incoming_call(
    Host(host): Host,
    Form(params): Form<HashMap<String, String>>,
) -> impl IntoResponse {
    let from = params.get("From").cloned().unwrap_or_default();
    let call_sid = params.get("CallSid").cloned().unwrap_or_default();

    tracing::info!("Incoming call: call_sid={call_sid}, from={from}, host={host}");

    let twiml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Response>
    <Connect>
        <Stream url="wss://{host}/media-stream">
            <Parameter name="callerPhone" value="{from}"/>
        </Stream>
    </Connect>
</Response>"#
    );

    ([(header::CONTENT_TYPE, "text/xml")], twiml)
}
