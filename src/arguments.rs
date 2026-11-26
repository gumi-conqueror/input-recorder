//! Command-line arguments. `permissions::restart` passes the data folder, the
//! Windows session and the device selection to the elevated process.

use std::{collections::BTreeSet, ffi::OsString};

use anyhow::{Result, anyhow};

pub(crate) const DATA_DIR: &str = "--data-dir";
pub(crate) const EXPECTED_SESSION: &str = "--expected-session";
pub(crate) const SELECTION: &str = "--selection";
pub(crate) const RECORD_SECONDS: &str = "--record-seconds";
pub(crate) const ELEVATE: &str = "--elevate";
pub(crate) const LIST_DEVICES: &str = "--list-devices";
pub(crate) const SMOKE_TEST: &str = "--smoke-test";

pub(crate) fn flag(name: &str) -> bool {
    std::env::args_os().any(|argument| argument == name)
}

/// The argument after `name`, or `None` when `name` is absent.
pub(crate) fn value(name: &str) -> Result<Option<OsString>> {
    let mut arguments = std::env::args_os();
    while let Some(argument) = arguments.next() {
        if argument == name {
            return arguments
                .next()
                .map(Some)
                .ok_or_else(|| anyhow!("{name} requires a value"));
        }
    }
    Ok(None)
}

pub(crate) fn text(name: &str) -> Result<Option<String>> {
    value(name)?
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow!("{name} must be valid Unicode"))
        })
        .transpose()
}

/// Device IDs are portable file-name characters, so a comma separates them.
pub(crate) fn selection(ids: &BTreeSet<String>) -> String {
    ids.iter().cloned().collect::<Vec<_>>().join(",")
}

pub(crate) fn parse_selection(text: &str) -> BTreeSet<String> {
    text.split(',')
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_round_trips_and_ignores_empty_entries() {
        let ids = BTreeSet::from(["hid-1".to_owned(), "keyboard_2".to_owned()]);
        assert_eq!(parse_selection(&selection(&ids)), ids);
        assert_eq!(
            parse_selection(",hid-1,,"),
            BTreeSet::from(["hid-1".to_owned()])
        );
    }
}
