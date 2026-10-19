//! Shared MIME entry point. The vendored parser enforces the nesting limit before
//! recursive `into_owned` or destruction can overflow a worker thread's stack.

use anyhow::{Result, anyhow};
use mail_parser::{Message, MessageParser};

pub(crate) fn parse(raw: &[u8]) -> Result<Message<'_>> {
    MessageParser::default()
        .parse(raw)
        .ok_or_else(|| anyhow!("无法解析邮件，邮件格式无效或 MIME 嵌套层级或解码内存超过安全上限"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};

    const CHILD_MODE: &str = "MAILPULSE_MIME_DEPTH_TEST";

    fn raw_nested(depth: usize) -> Vec<u8> {
        let mut raw = "Content-Type: message/rfc822\r\n\r\n".repeat(depth);
        raw.push_str("Subject: nested\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n正常正文");
        raw.into_bytes()
    }

    fn encoded(raw: &[u8]) -> Vec<u8> {
        format!(
            "Content-Type: message/rfc822\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
            STANDARD.encode(raw)
        )
        .into_bytes()
    }

    // Runs inside a subprocess so a future dependency regression reports a failed
    // test instead of aborting the complete test runner or desktop application.
    #[test]
    fn mime_depth_child_process() {
        let Ok(mode) = std::env::var(CHILD_MODE) else {
            return;
        };
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                match mode.as_str() {
                    "raw" => {
                        assert!(parse(&raw_nested(10_000)).is_err());
                    }
                    "encoded" => {
                        assert!(parse(&encoded(&raw_nested(10_000))).is_err());
                    }
                    "shared_ownership" => {
                    let mut raw = raw_nested(31);
                    raw.extend(std::iter::repeat_n(b'x', 2 * 1024 * 1024));
                    let one = encoded(&raw);
                    drop(parse(&one).unwrap());
                    let mut siblings = b"Content-Type: multipart/mixed; boundary=siblings\r\n\r\n".to_vec();
                    for _ in 0..2 {
                        siblings.extend_from_slice(b"--siblings\r\n");
                        siblings.extend_from_slice(&one);
                        siblings.extend_from_slice(b"\r\n");
                    }
                    siblings.extend_from_slice(b"--siblings--\r\n");
                    assert!(parse(&siblings).is_err(), "sibling encoded parts share one ownership budget");
                }
                "ownership" => {
                    // Small encoded input must not multiply into hundreds of MiB.
                    let mut raw = raw_nested(62);
                    raw.extend(std::iter::repeat_n(b'x', 3 * 1024 * 1024));
                    assert!(parse(&encoded(&raw)).is_err());
                    // Width also amplifies full raw buffers, even at shallow depth.
                    let mut wide = b"Content-Type: multipart/mixed; boundary=wide\r\n\r\n".to_vec();
                    for _ in 0..100 {
                        wide.extend_from_slice(b"--wide\r\nContent-Type: message/rfc822\r\n\r\n");
                        wide.extend_from_slice(&raw_nested(0));
                        wide.extend(std::iter::repeat_n(b'x', 16 * 1024));
                        wide.extend_from_slice(b"\r\n");
                    }
                    wide.extend_from_slice(b"--wide--\r\n");
                    assert!(parse(&encoded(&wide)).is_err());
                }
                "other_forms" => {
                    let global = String::from_utf8(raw_nested(10_000)).unwrap().replace("message/rfc822", "message/global");
                    assert!(parse(global.as_bytes()).is_err());
                    let truncated = "Content-Type: message/rfc822\r\n\r\n".repeat(10_000);
                    assert!(parse(truncated.as_bytes()).is_err());
                    let raw = raw_nested(10_000);
                    let qp = raw.iter().map(|b| format!("={b:02X}")).collect::<String>();
                    let qp = format!("Content-Type: message/rfc822\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n{qp}");
                    assert!(parse(qp.as_bytes()).is_err());
                    let mut digest = raw_nested(0);
                    for i in 0..100 {
                        let mut outer = format!("Content-Type: multipart/digest; boundary=d{i}\r\n\r\n--d{i}\r\nContent-Description: embedded\r\n\r\n").into_bytes();
                        outer.extend_from_slice(&digest);
                        outer.extend_from_slice(format!("\r\n--d{i}--\r\n").as_bytes());
                        digest = outer;
                    }
                    assert!(parse(&digest).is_err());
                    let mut wide = b"Content-Type: multipart/mixed; boundary=wide\r\n\r\n".to_vec();
                    for _ in 0..80 {
                        wide.extend_from_slice(b"--wide\r\nContent-Type: message/rfc822\r\n\r\n");
                        wide.extend_from_slice(&raw_nested(0));
                        wide.extend_from_slice(b"\r\n");
                    }
                    wide.extend_from_slice(b"--wide--\r\n");
                    let parsed = parse(&wide).unwrap();
                    assert_eq!(parsed.attachments().count(), 80);
                    drop(parsed);
                }
                "boundary" => {
                        // The 64-level budget includes the leaf message itself.
                        let valid = raw_nested(63);
                        drop(parse(&valid).unwrap());
                        assert!(parse(&raw_nested(64)).is_err());
                        let valid_encoded = encoded(&raw_nested(62));
                        drop(parse(&valid_encoded).unwrap());
                        assert!(parse(&encoded(&raw_nested(63))).is_err());
                        // Encoding a deeper tree again must not reset the budget.
                        let valid_twice = encoded(&encoded(&raw_nested(61)));
                        drop(parse(&valid_twice).unwrap());
                        assert!(parse(&encoded(&encoded(&raw_nested(62)))).is_err());
                    }
                    "ordinary" => {
                        let raw = raw_nested(2);
                        let parsed = parse(&raw).unwrap();
                        let child = parsed.attachments().next().unwrap().message().unwrap();
                        let leaf = child.attachments().next().unwrap().message().unwrap();
                        assert_eq!(leaf.subject(), Some("nested"));
                        assert_eq!(leaf.body_text(0).unwrap(), "正常正文");
                        drop(parsed);
                        let raw = encoded(&raw);
                        let parsed = parse(&raw).unwrap();
                        assert!(parsed.attachments().next().unwrap().message().is_some());
                        drop(parsed);
                    }
                    _ => panic!("unexpected child test mode"),
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn nested_mime_is_bounded_before_parse_conversion_and_drop() {
        for mode in [
            "raw",
            "encoded",
            "other_forms",
            "ownership",
            "shared_ownership",
            "boundary",
            "ordinary",
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "mime::tests::mime_depth_child_process",
                    "--nocapture",
                ])
                .env(CHILD_MODE, mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "MIME {mode} child failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
