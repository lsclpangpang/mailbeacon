# MailPulse MIME nesting safety patch

Source: mail-parser 0.11.9 from crates.io, upstream https://github.com/stalwartlabs/mail-parser.

Local Cargo package/lib name is `mailbeacon-parser` / `mailbeacon_parser` so build logs show MailBeacon branding; the vendor directory path stays `vendor/mail-parser`.
Unused optional features (`rkyv`, `serde`, `base64_slice`) were also dropped from the vendored manifest; only `full_encoding` remains, matching what MailBeacon enables.
Original crate SHA-256: 4ec00bda90c6e645a54506c630c2820cd6b1890cfd2b0a169b50f74b2b8c7c86.
The original Apache-2.0 and MIT license texts are preserved in LICENSES/.
Only compilation sources, their README include, licenses, and the manifest are vendored. Example/test targets and dev dependencies were removed from the manifest because upstream fixtures are not vendored.

The upstream parser bounds encoded-message recursion but does not bound unencoded `message/rfc822` ancestry. A small deeply nested message can overflow the thread stack when dropping the parsed message; inside an encoded message it can overflow during `into_owned`, before returning from parsing.

`src/parsers/message.rs` limits combined multipart/message ancestry to 64, carries the remaining budget across encoded boundaries, and propagates limit failure before recursive ownership conversion. Existing encoded recursion limit of 3 stays in effect. A shared 128 MiB budget also limits retained decoded buffers plus the descendant raw buffers copied by `into_owned`; this prevents depth or wide sibling parts from multiplying one encoded payload into unbounded memory. It is an ownership amplification budget, not a limit on the entire process or incoming raw mail. Input sizes exceeding the parser's u32 offset representation are rejected. Normal MIME handling is otherwise unchanged.

Application regression tests are in `src-tauri/src/mime.rs`, executed in child processes on 2 MiB thread stacks. Keep this patch until an upstream release provides equivalent limits, and rerun these tests when upgrading.
