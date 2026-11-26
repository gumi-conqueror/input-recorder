use anyhow::{Result, ensure};
use serde_json::{Value, json};
use windows::Win32::UI::Input::{RIM_TYPEHID, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE};

const MOUSE: u32 = RIM_TYPEMOUSE.0;
const KEYBOARD: u32 = RIM_TYPEKEYBOARD.0;
const HID: u32 = RIM_TYPEHID.0;
/// Payload sizes of the Win32 RAWMOUSE and RAWKEYBOARD structures, and of the RAWHID header.
const RAWMOUSE_BYTES: usize = 24;
const RAWKEYBOARD_BYTES: usize = 16;
const RAWHID_HEADER_BYTES: usize = 8;

/// Device and event kind for a Raw Input type.
pub(crate) fn kind_name(kind: u32) -> Option<&'static str> {
    match kind {
        MOUSE => Some("mouse"),
        KEYBOARD => Some("keyboard"),
        HID => Some("hid"),
        _ => None,
    }
}

pub(crate) struct Payload {
    pub kind: &'static str,
    pub fields: Value,
    pub raw: Vec<u8>,
}

fn word(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn dword(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated packet length"),
    )
}

pub(crate) fn parse(kind: u32, bytes: &[u8]) -> Result<Vec<Payload>> {
    match kind {
        MOUSE => {
            ensure!(bytes.len() >= RAWMOUSE_BYTES, "truncated RAWMOUSE");
            Ok(vec![Payload {
                kind: "mouse",
                raw: bytes[..RAWMOUSE_BYTES].to_vec(),
                fields: json!({
                    "flags": word(bytes, 0), "button_flags": word(bytes, 4),
                    "button_data": word(bytes, 6), "wheel_delta": word(bytes, 6) as i16,
                    "raw_buttons": dword(bytes, 8), "x": dword(bytes, 12) as i32,
                    "y": dword(bytes, 16) as i32, "extra_information": dword(bytes, 20),
                    "raw_format": "win32_rawmouse_le",
                }),
            }])
        }
        KEYBOARD => {
            ensure!(bytes.len() >= RAWKEYBOARD_BYTES, "truncated RAWKEYBOARD");
            Ok(vec![Payload {
                kind: "keyboard",
                raw: bytes[..RAWKEYBOARD_BYTES].to_vec(),
                fields: json!({
                    "scan_code": word(bytes, 0), "flags": word(bytes, 2),
                    "released": word(bytes, 2) & 1 != 0, "virtual_key": word(bytes, 6),
                    "message": dword(bytes, 8), "extra_information": dword(bytes, 12),
                    "raw_format": "win32_rawkeyboard_le",
                }),
            }])
        }
        HID => {
            ensure!(
                bytes.len() >= RAWHID_HEADER_BYTES,
                "truncated RAWHID header"
            );
            let size = dword(bytes, 0) as usize;
            let count = dword(bytes, 4) as usize;
            ensure!(size > 0 && count > 0, "empty HID report batch");
            let length = size
                .checked_mul(count)
                .ok_or_else(|| anyhow::anyhow!("HID batch overflow"))?;
            ensure!(
                length <= bytes.len() - RAWHID_HEADER_BYTES,
                "truncated HID report batch"
            );
            Ok(bytes[RAWHID_HEADER_BYTES..RAWHID_HEADER_BYTES + length]
                .chunks_exact(size)
                .enumerate()
                .map(|(index, report)| Payload {
                    kind: "hid",
                    raw: report.to_vec(),
                    fields: json!({"raw_format": "hid_input_report", "report_size": size,
                    "batch_index": index, "batch_count": count}),
                })
                .collect())
        }
        _ => anyhow::bail!("unknown Raw Input type {kind}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_every_report_in_a_batch() {
        let bytes = [3, 0, 0, 0, 2, 0, 0, 0, 1, 7, 9, 2, 0, 255];
        let reports = parse(HID, &bytes).expect("valid batch");
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].raw, [1, 7, 9]);
        assert_eq!(reports[1].raw, [2, 0, 255]);
        assert_eq!(reports[1].fields["batch_index"], 1);
    }

    #[test]
    fn rejects_truncation_and_invalid_batch_lengths() {
        for length in 0..14 {
            let bytes = [3, 0, 0, 0, 2, 0, 0, 0, 1, 7, 9, 2, 0, 255];
            assert!(parse(HID, &bytes[..length]).is_err(), "length {length}");
        }
        assert!(parse(HID, &[255; 8]).is_err());
        assert!(parse(HID, &[0; 8]).is_err());
    }

    #[test]
    fn preserves_signed_mouse_motion_and_keyboard_release() {
        let mut mouse = [0; 24];
        mouse[12..16].copy_from_slice(&(-123_i32).to_le_bytes());
        assert_eq!(parse(MOUSE, &mouse).expect("mouse")[0].fields["x"], -123);
        let mut keyboard = [0; 16];
        keyboard[2] = 1;
        assert_eq!(
            parse(KEYBOARD, &keyboard).expect("keyboard")[0].fields["released"],
            true
        );
    }
}
