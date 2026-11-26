use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Device {
    pub id: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub usage_page: u16,
    pub usage: u16,
    pub properties: Value,
    pub capabilities: Value,
    pub errors: Vec<String>,
}

/// Device ID for input whose Raw Input handle has no known device.
pub(crate) const UNATTRIBUTED_ID: &str = "unattributed";

impl Device {
    pub(crate) fn unattributed() -> Self {
        Self {
            id: UNATTRIBUTED_ID.into(),
            path: String::new(),
            name: "Unattributed input".into(),
            kind: "unknown".into(),
            usage_page: 0,
            usage: 0,
            properties: serde_json::json!({"note": "Events without a known Raw Input device; handles remain in event fields"}),
            capabilities: Value::Null,
            errors: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Event {
    pub sequence: u64,
    pub elapsed_us: u64,
    pub device_id: String,
    pub connection: u64,
    pub kind: String,
    pub fields: Value,
    pub raw: Vec<u8>,
}

#[derive(Debug)]
pub(crate) enum Update {
    Devices(Vec<Device>),
    Diagnostic(String),
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}
