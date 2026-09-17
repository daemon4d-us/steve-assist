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
