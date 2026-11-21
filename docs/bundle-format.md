# Capture bundle format

## Files

| File | Contents |
| --- | --- |
| `manifest.json` | Format version, capture metadata, exported device IDs, event count, and completion status |
| `devices.json` | Snapshots for the exported interfaces only |
| `capabilities/<id>.json` | Windows HID parser capabilities, or null when unavailable |
| `events/<id>.jsonl` | One event per line, ordered by capture sequence |
| `reports/<id>.bin` | Concatenated bytes referenced by that interface's events |
| `markers.jsonl` | Session-wide action labels and their elapsed times |
| `diagnostics.jsonl` | Session-wide errors and observed capture-loss changes |
| `checksums.sha256` | SHA-256 for each bundle member except the checksum file itself |

## Events

Events contain `sequence`, `elapsed_us`, `device_id`, `connection`, `kind`, `fields`, `raw_offset`, and `raw_length`. Read `raw_length` bytes at `raw_offset` in the matching binary file. Connection events have empty raw data. Sequence gaps can come from filtering as well as capture loss; consult diagnostics before interpreting them.

`elapsed_us` is a monotonic reception timestamp, not a hardware timestamp. A batch of HID reports shares a reception time and records `batch_index`, `batch_count`, and `report_size`. The manifest provides an adjacent wall-clock anchor in `started_unix_us`.

`connection` changes when an interface is rediscovered after disconnection. Device IDs hash the Windows interface path. They distinguish simultaneously connected collections but do not guarantee identity across ports, driver reinstalls, or machines. Container IDs are grouping evidence, not proof that every wireless receiver represents only one physical device.

## Raw formats

- `hid_input_report`: bytes returned by Windows RAWHID. Report length and report-ID conventions come from the capability snapshot. An initial byte is not unconditionally a report ID.
- `win32_rawkeyboard_le`: the 16-byte Windows RAWKEYBOARD structure. Fields include scan code, virtual key, flags, message, and extra information.
- `win32_rawmouse_le`: the 24-byte Windows RAWMOUSE structure. Fields include movement mode, button flags, button data, raw buttons, X/Y movement, and extra information. Interpret `wheel_delta` only when the wheel flags are present; interpret X/Y according to the movement flags.

Original HID report descriptors are currently unavailable. Capability snapshots preserve input/output/feature report lengths and available button/value usages, ranges, units, report IDs, and collection references. They do not replace a raw descriptor or guarantee arbitrary offline semantic decoding.

## Completeness

`recording.status` is `stopped`, `incomplete`, or `recording`. The last value means the recording was not finalized normally. `known_loss` counts observed queue overflow and capture failures. It covers the capture session and can include unselected interfaces. Driver-side or device-side loss may be undetectable.

Markers and diagnostics apply to the whole recording. Export filters all device snapshots and raw events to the selected interfaces, but retains those session-wide annotations. Failed export attempts leave an existing destination unchanged; the finished ZIP replaces the destination only after writing and syncing succeeds.
