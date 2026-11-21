use anyhow::{Result, ensure};
use serde_json::{Value, json};

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
        0 => {
            ensure!(bytes.len() >= 24, "truncated RAWMOUSE");
            Ok(vec![Payload {
                kind: "mouse",
                raw: bytes[..24].to_vec(),
                fields: json!({
                    "flags": word(bytes, 0), "button_flags": word(bytes, 4),
                    "button_data": word(bytes, 6), "wheel_delta": word(bytes, 6) as i16,
                    "raw_buttons": dword(bytes, 8), "x": dword(bytes, 12) as i32,
                    "y": dword(bytes, 16) as i32, "extra_information": dword(bytes, 20),
                    "raw_format": "win32_rawmouse_le",
                }),
            }])
        }
        1 => {
            ensure!(bytes.len() >= 16, "truncated RAWKEYBOARD");
            Ok(vec![Payload {
                kind: "keyboard",
                raw: bytes[..16].to_vec(),
                fields: json!({
                    "scan_code": word(bytes, 0), "flags": word(bytes, 2),
                    "released": word(bytes, 2) & 1 != 0, "virtual_key": word(bytes, 6),
                    "message": dword(bytes, 8), "extra_information": dword(bytes, 12),
                    "raw_format": "win32_rawkeyboard_le",
                }),
            }])
        }
        2 => {
            ensure!(bytes.len() >= 8, "truncated RAWHID header");
            let size = dword(bytes, 0) as usize;
            let count = dword(bytes, 4) as usize;
            ensure!(size > 0 && count > 0, "empty HID report batch");
            let length = size
                .checked_mul(count)
                .ok_or_else(|| anyhow::anyhow!("HID batch overflow"))?;
            ensure!(length <= bytes.len() - 8, "truncated HID report batch");
            Ok(bytes[8..8 + length]
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
        let reports = parse(2, &bytes).expect("valid batch");
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].raw, [1, 7, 9]);
        assert_eq!(reports[1].raw, [2, 0, 255]);
        assert_eq!(reports[1].fields["batch_index"], 1);
    }

    #[test]
    fn rejects_truncation_and_invalid_batch_lengths() {
        for length in 0..14 {
            let bytes = [3, 0, 0, 0, 2, 0, 0, 0, 1, 7, 9, 2, 0, 255];
            assert!(parse(2, &bytes[..length]).is_err(), "length {length}");
        }
        assert!(parse(2, &[255; 8]).is_err());
        assert!(parse(2, &[0; 8]).is_err());
    }

    #[test]
    fn preserves_signed_mouse_motion_and_keyboard_release() {
        let mut mouse = [0; 24];
        mouse[12..16].copy_from_slice(&(-123_i32).to_le_bytes());
        assert_eq!(parse(0, &mouse).expect("mouse")[0].fields["x"], -123);
        let mut keyboard = [0; 16];
        keyboard[2] = 1;
        assert_eq!(
            parse(1, &keyboard).expect("keyboard")[0].fields["released"],
            true
        );
    }
}
