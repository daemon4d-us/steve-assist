//! Minimal oFono D-Bus bindings: modem discovery and voice-call control for
//! the HFP hands-free modem that represents the paired phone.

use std::collections::HashMap;

use zbus::proxy;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

#[proxy(
    interface = "org.ofono.Manager",
    default_service = "org.ofono",
    default_path = "/"
)]
pub trait Manager {
    fn get_modems(&self) -> zbus::Result<Vec<(OwnedObjectPath, HashMap<String, OwnedValue>)>>;
}

#[proxy(
    interface = "org.ofono.VoiceCallManager",
    default_service = "org.ofono"
)]
pub trait VoiceCallManager {
    fn get_calls(&self) -> zbus::Result<Vec<(OwnedObjectPath, HashMap<String, OwnedValue>)>>;

    /// Ask the phone to place a call. `hide_callerid` is "" (network default),
    /// "enabled" or "disabled". Returns the new call's object path.
    fn dial(&self, number: &str, hide_callerid: &str) -> zbus::Result<OwnedObjectPath>;

    #[zbus(signal)]
    fn call_added(
        &self,
        path: OwnedObjectPath,
        properties: HashMap<String, OwnedValue>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    fn call_removed(&self, path: OwnedObjectPath) -> zbus::Result<()>;
}

#[proxy(interface = "org.ofono.VoiceCall", default_service = "org.ofono")]
pub trait VoiceCall {
    fn answer(&self) -> zbus::Result<()>;
    fn hangup(&self) -> zbus::Result<()>;
    fn get_properties(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

    #[zbus(signal)]
    fn property_changed(&self, name: String, value: OwnedValue) -> zbus::Result<()>;
}

/// The HFP modem object for the connected phone, e.g.
/// `/hfp/org/bluez/hci0/dev_B0_D5_FB_CF_4B_79`, if the phone is connected.
pub async fn find_hfp_modem(conn: &zbus::Connection) -> zbus::Result<Option<OwnedObjectPath>> {
    let mgr = ManagerProxy::new(conn).await?;
    let modems = mgr.get_modems().await?;
    Ok(modems
        .into_iter()
        .map(|(path, _)| path)
        .find(|p| p.as_str().starts_with("/hfp/")))
}

/// String-valued property out of an oFono `a{sv}` dictionary.
pub fn prop_str(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    match props.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

pub fn value_str(v: &OwnedValue) -> Option<String> {
    match &**v {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// Current `State` of a call: incoming, dialing, alerting, active, held,
/// waiting, disconnected.
pub async fn call_state(call: &VoiceCallProxy<'_>) -> Option<String> {
    call.get_properties()
        .await
        .ok()
        .and_then(|p| prop_str(&p, "State"))
}

/// Wait until the call reaches one of `targets` (or goes away). Returns the
/// state reached, or None on timeout.
pub async fn wait_for_state(
    call: &VoiceCallProxy<'_>,
    targets: &[&str],
    timeout: std::time::Duration,
) -> Option<String> {
    use futures_util::StreamExt;
    let mut changes = call.receive_property_changed().await.ok()?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // Check first: the state may already be there (or the call gone).
        match call_state(call).await {
            Some(s) if targets.contains(&s.as_str()) || s == "disconnected" => return Some(s),
            None => return Some("disconnected".into()),
            _ => {}
        }
        let next = tokio::time::timeout_at(deadline, changes.next()).await;
        match next {
            Ok(Some(sig)) => {
                if let Ok(args) = sig.args()
                    && args.name() == "State"
                {
                    let s = value_str(args.value()).unwrap_or_default();
                    if targets.contains(&s.as_str()) || s == "disconnected" {
                        return Some(s);
                    }
                }
            }
            Ok(None) => return Some("disconnected".into()),
            Err(_) => return None,
        }
    }
}
