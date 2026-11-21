# Initial verification

These results describe the development machine, not universal device coverage.

- Formatting and Clippy passed with warnings denied.
- Seven automated tests passed. They cover HID batch preservation, malformed packet rejection, signed mouse motion and keyboard release, selective export and byte offsets, bundle checksums, interrupted recording and failed export behavior, Windows argument quoting, and process permission queries.
- The optimized Windows x64 build started through Direct3D 12 on an NVIDIA GeForce RTX 4080 SUPER.
- Enumeration returned 28 Raw Input interfaces: 16 HID, 7 keyboard, and 5 mouse. HID capability queries succeeded for all 16 HID interfaces. Individual metadata access errors were retained.
- A GUI recording saved 6,509 HID reports, 1,472 mouse events, 10 keyboard events, and 28 connection events. It stopped normally with zero known capture loss, and SQLite integrity checking passed. This does not measure loss before Windows delivered input.
- A separate GUI check cleared device selection, selected one HID interface, started recording, entered and saved a marker, stopped, and exported through the native save dialog. The resulting ZIP contained only that interface, preserved the marker, and passed every member checksum. That second check recorded only a connection event because the selected device was idle.
- Synthetic export tests verify byte-for-byte report preservation and exclusion of another device's events.

Actual UAC consent, restricted hardware access after elevation, rapid physical reconnects, high-rate saturation, other GPUs, and a clean Windows installation still need hardware validation. Automated checks do not operate the UAC secure desktop.

Recordings and exports used for these checks remain in ignored local directories and are not part of the repository or application package.
