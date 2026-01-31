# Notifications (OS alerts)

## Sources checked (2025-2026)
- notify-rust docs: API (`summary`, `body`, `show`), macOS note that `appname` has no effect and `set_application` must be used for bundle/app id.
- notify-rust crate/lib: cross-platform support (Linux, macOS, Windows) and dependency on `mac-notification-sys` + `tauri-winrt-notification`.
- tauri-winrt-notification crate: recent release 0.7.2 (2025-04-08), 119k downloads, used by ~211 crates.
- mac-notification-sys crate: recent release 0.6.6 (2025-06-15), ~195k downloads, used by ~117 crates.

## Takeaways
- `notify-rust` is a simple cross-platform API for OS notifications; suitable for macOS + Windows.
- macOS requires `notify_rust::set_application` to map notifications to the bundle/app id.
- Windows support is via toasts (`tauri-winrt-notification`); no extra platform API calls needed beyond `show()`.
