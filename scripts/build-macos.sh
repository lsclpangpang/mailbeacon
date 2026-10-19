#!/bin/sh
set -eu

# CI and other machines keep using the repository's ad-hoc fallback. On this
# Mac, a one-time local certificate keeps the Keychain app identity stable.
identity="${MAILPULSE_SIGNING_IDENTITY:-}"
if [ -z "$identity" ] && [ "$(uname -s)" = "Darwin" ]; then
  if security find-certificate -c "MailPulse Local Signing" "$HOME/Library/Keychains/login.keychain-db" >/dev/null 2>&1; then
    identity="MailPulse Local Signing"
  fi
fi

if [ -n "$identity" ]; then
  override=$(mktemp "${TMPDIR:-/tmp}/mailpulse-tauri-config.XXXXXX.json")
  trap 'rm -f "$override"' EXIT HUP INT TERM
  printf '{"bundle":{"macOS":{"signingIdentity":"%s"}}}\n' "$identity" > "$override"
  exec npx tauri build --config "$override" "$@"
fi

exec npx tauri build "$@"
