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
