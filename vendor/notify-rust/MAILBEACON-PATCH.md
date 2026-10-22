# MailBeacon notify-rust patch

Source: notify-rust 4.18.2 from crates.io
(upstream https://github.com/hoodie/notify-rust).

## Why

`tauri-plugin-notification` on macOS waits for clicks via
`NotificationHandle::wait_for_response` → `send_mac_notification(..., asynchronous: false)`.
Our mail notifications have no action buttons (`action_type_id` unset).

`mac-notification-sys` only blocks for a user response when
`needs_response()` is true:

```
main_button.is_some() || close_button.is_some() || wait_for_click
```

Upstream `send_mac_notification` set `asynchronous(false)` but never called
`wait_for_click(true)`, so `send()` returned `None` immediately, the plugin
mapped that to `Closed`, and our Rust `on_action` never saw `"tap"`.
macOS still activates the app on click, which looked like “window opens but
does not jump to the mail”.

There is even a commented `// n.wait_for_click(true);` in upstream
`wait_for_action`, which confirms the intended fix.

## Patch

In `src/macos/nsusernotifications.rs` `send_mac_notification`, when
`!asynchronous`, call `n.wait_for_click(true)` before `send()`.

Wired with workspace `[patch.crates-io] notify-rust = { path = "vendor/notify-rust" }`.

Keep until upstream notify-rust applies the same fix (or we switch the plugin
to `preview-macos-un` / UNUserNotificationCenter).
