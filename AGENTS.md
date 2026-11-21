# Project rules

- Focus on input capture, device identification, and export. Keep the UI plain; do not add themes.
- Keep application data beside the executable or in an explicitly selected folder. Do not create an AppData directory or write startup files. Create recording directories only when recording starts, and show the data location in the UI.
- Write code, UI text, and commit messages in English.
- Commit and push each completed major logic change. Use short, single-line commit messages.
- Use Direct3D 12 on Windows. Do not enable automatic OpenGL or Vulkan fallback.
- Keep capture independent of UI rendering. Preserve unknown reports and report known data loss.
- Never commit real recordings, device serial numbers, or exported capture bundles.
- Run formatting, Clippy, and relevant tests before completing a logic change.
