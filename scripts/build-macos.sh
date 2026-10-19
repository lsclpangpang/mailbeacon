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
  # BSD mktemp doesn't accept a suffix after the X's, so use a temp directory.
  tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/mailpulse-tauri-config.XXXXXX")
  trap 'rm -rf "$tmpdir"' EXIT HUP INT TERM
  override="$tmpdir/config.json"
  printf '{"bundle":{"macOS":{"signingIdentity":"%s"}}}\n' "$identity" > "$override"
  npx tauri build --config "$override" "$@"
else
  npx tauri build "$@"
fi
