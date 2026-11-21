# input-recorder

A small Windows utility for recording input from unfamiliar devices and exporting data for analysis on another computer.

## What works

- Enumerate Raw Input keyboards, mice, and generic HID collections, including vendor-defined usages.
- Capture background input on a dedicated thread, with a source interface and connection generation on each event.
- Keep every report in a HID batch, including reports the application cannot interpret.
- Save device paths, available VID/PID and serial information, container IDs, and HID capability snapshots.
- Record to SQLite, add action markers, and export only the selected devices to a ZIP analysis bundle.
- Reopen a saved SQLite recording and export a different device selection.
- Use a plain egui interface rendered through Direct3D 12. OpenGL and Vulkan are disabled.
- Restart with administrator permissions when requested, preserving the data directory and selected interfaces.

The UI is intentionally basic: select devices, start, perform an action, add a marker, stop, and export. No theme settings or dashboards.

## Run

Download the Windows x64 artifact from [a successful build](https://github.com/gumi-conqueror/input-recorder/actions/workflows/build.yml), unpack the ZIP, and run `input-recorder.exe`. A working Direct3D 12 adapter and driver are required. The package includes dependency notices; no installer is needed.

Build on Windows x64 with the MSVC C++ build tools installed. The Rust toolchain is pinned in `rust-toolchain.toml`.

```powershell
cargo run --locked
cargo build --release --locked
./scripts/package.ps1
```

Recordings normally go to `%LOCALAPPDATA%\InputRecorder\recordings`. Use `--data-dir PATH` to choose a different location. The application starts with standard permissions. Use **Restart as administrator**, or launch with `--elevate`, if you want to retry restricted device access. Declining UAC keeps standard mode available. Administrator privileges do not provide UI Access or bypass every driver restriction.

```powershell
# Enumerate device metadata without opening the GUI.
cargo run --locked -- --list-devices --data-dir target/diagnostics

# Explicitly record all discovered interfaces for a bounded interval and export them.
cargo run --locked -- --record-seconds 5 --data-dir target/diagnostics

# Check DX12 initialization and close automatically without recording input.
cargo run --locked -- --smoke-test --data-dir target/smoke
```

CLI diagnostics write their results under the chosen data directory. The release executable uses the Windows GUI subsystem, so command-line status text may not appear in an interactive terminal.

## Capture workflow

1. Connect the target device and click **Refresh devices**.
2. Select its interfaces. Use **Details** to distinguish similar names or composite-device collections.
3. Click **Start recording**. Record a short idle interval, then perform one action at a time.
4. Add markers such as `Press button A`, `Move stick left`, or `Release pedal`.
5. Click **Stop** and wait for the saved-recording message.
6. Select the interfaces to share and click **Export selected**.

New interfaces are discovered during recording, but the recording selection is fixed. Refresh and start another recording to include new devices. **Unattributed input** is a separate selectable group; these events are not assigned to a guessed physical device. Preview updates may be skipped independently of recording. Known capture loss is counted and included in the bundle.

## Export

A ZIP contains `manifest.json`, `devices.json`, per-interface capabilities, JSONL events, binary reports, markers, diagnostics, and SHA-256 checksums. Each event references its original bytes by offset and length. No target device or driver is needed to inspect the files. See [the bundle format](docs/bundle-format.md).

An interrupted recording remains marked `recording`; an orderly stop with known capture loss is marked `incomplete`. Successfully stopping does not prove that a device or driver delivered every physical action. Raw keyboard and mouse bytes are Windows structures, not USB packets. HID capability snapshots are not original HID report descriptors.

## Current limits

This first implementation uses Windows Raw Input. Devices absent from that API, direct shared HID reads, GameInput/XInput-only controllers, MIDI, Pointer-based touch/pen capture, and proprietary serial/BLE protocols need additional backends. RDP and some precision-touchpad input cannot be attributed reliably. Device enumeration and capability reads can succeed even when input reports are unavailable.

Only the current interactive Windows session is targeted. IME text reconstruction, other user sessions, audio/video recording, and the secure desktop are outside the current scope. Device macros and virtual devices cannot always be distinguished from manual input.

Real recordings may contain key activity and device serial numbers. They remain local until you export and share them. Do not commit recordings or export bundles to this public repository.

## Development

```powershell
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked
```

Commit and push each completed major logic change. Code, UI text, and short single-line commit messages are in English. The original [design document](docs/design.md) also describes possible later backends; it is not a statement of implemented coverage.

## License

GNU General Public License v3.0 only (`GPL-3.0-only`). See [LICENSE](LICENSE).
