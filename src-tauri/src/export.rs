//! 将完整邮件原文原样存为 .eml，不改变 MIME、附件或邮件头。

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use icu_properties::{CodePointMapData, props::GeneralCategory};
use mailbeacon_parser::MessageParser;

fn safe_stem(subject: &str, uid: u32) -> String {
    let categories = CodePointMapData::<GeneralCategory>::new();
    let clean: String = subject
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            // Valid UTF-8 can still be rejected as a macOS filename (EILSEQ).
            // Cn covers unassigned code points such as U+03A2 and noncharacters.
            if categories.get(c) == GeneralCategory::Unassigned
                || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut clean = clean
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string();
    let mut end = clean.len().min(180);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean.truncate(end);
    clean = clean.trim_end_matches([' ', '.']).to_string();
    if clean.is_empty() {
        format!("邮件-{uid}")
    } else {
        let base = clean.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                base.strip_prefix(prefix).is_some_and(|suffix| {
                    suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9')
                })
            })
        {
            clean.insert(0, '_');
            let mut end = clean.len().min(180);
            while !clean.is_char_boundary(end) {
                end -= 1;
            }
            clean.truncate(end);
        }
        clean
    }
}

/// 自动、全量和单封下载共用解码及文件名净化规则。
pub(crate) fn subject_stem(raw: &[u8], uid: u32) -> String {
    let raw = crate::mime::fix_headers(raw);
    let message = MessageParser::default().parse_headers(&raw);
    safe_stem(
        message.as_ref().and_then(|m| m.subject()).unwrap_or(""),
        uid,
    )
}

fn filename_encoding_error(error: &std::io::Error) -> bool {
    // Darwin EILSEQ. Do not reinterpret permissions, disk-full or length errors.
    cfg!(target_os = "macos") && error.raw_os_error() == Some(92)
}

/// Retry name allocation once if macOS rejects characters newer than its Unicode tables.
/// The operation must not change an already-published identity mapping.
pub(crate) fn with_filesystem_stem<T>(
    dir: &Path,
    stem: &str,
    mut allocate: impl FnMut(&str) -> Result<T>,
) -> Result<T> {
    match allocate(stem) {
        Err(error)
            if error.chain().any(|cause| {
                cause
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(filename_encoding_error)
            }) =>
        {
            // Probe only after rejection, in an isolated directory on the destination
            // filesystem. A character accepted on one volume may fail on another.
            let probe = tempfile::Builder::new()
                .prefix(".mailpulse-filename-")
                .tempdir_in(dir)
                .context("无法检查下载文件名兼容性")?;
            let mut tested = std::collections::HashMap::new();
            let mut compatible = String::with_capacity(stem.len());
            for character in stem.chars() {
                let replacement = if character.is_ascii() {
                    character
                } else if let Some(&replacement) = tested.get(&character) {
                    replacement
                } else {
                    let path = probe.path().join(format!("c{character}"));
                    let replacement = match fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                    {
                        Ok(file) => {
                            drop(file);
                            fs::remove_file(&path).context("无法清理文件名检查临时文件")?;
                            character
                        }
                        Err(error) if filename_encoding_error(&error) => '_',
                        Err(error) => return Err(error).context("无法检查下载文件名兼容性"),
                    };
                    tested.insert(character, replacement);
                    replacement
                };
                compatible.push(replacement);
            }
            probe.close().context("无法清理文件名检查临时目录")?;
            if compatible == stem {
                return Err(error);
            }
            // Re-run all collision checks. A second rejection is returned unchanged.
            allocate(&compatible)
        }
        result => result,
    }
}

/// 在目标目录创建唯一的 .eml 文件；即使并发下载也不覆盖已有文件。
pub fn save_eml(dir: &Path, raw: &[u8], uid: u32) -> Result<PathBuf> {
    let stem = subject_stem(raw, uid);
    save_eml_from_reader(dir, &stem, raw)
}

fn save_eml_from_reader(dir: &Path, stem: &str, mut source: impl Read) -> Result<PathBuf> {
    fs::create_dir_all(dir).context("无法创建下载文件夹")?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".mailpulse-")
        .tempfile_in(dir)
        .context("无法创建邮件下载临时文件")?;
    std::io::copy(&mut source, &mut temporary).context("保存邮件失败")?;
    temporary.as_file().sync_all().context("确认邮件写入失败")?;
    let mut temporary = Some(temporary);
    with_filesystem_stem(dir, stem, |stem| {
        for index in 1u64.. {
            let name = if index == 1 {
                format!("{stem}.eml")
            } else {
                format!("{stem} ({index}).eml")
            };
            let path = dir.join(name);
            let file = temporary.take().context("邮件下载临时文件已释放")?;
            match file.persist_noclobber(&path) {
                Ok(_) => {
                    #[cfg(unix)]
                    fs::File::open(dir)?
                        .sync_all()
                        .context("确认邮件目录写入失败")?;
                    return Ok(path);
                }
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    temporary = Some(error.file);
                }
                Err(error) => {
                    temporary = Some(error.file);
                    return Err(error.error).context("无法保存邮件到下载文件夹");
                }
            }
        }
        unreachable!("无法耗尽全部下载文件名")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_original_mime_bytes_without_overwriting() {
        let dir = std::env::temp_dir().join(format!(
            "mailpulse-export-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let raw = b"Subject: =?UTF-8?B?5rWL6K+V?=\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Transfer-Encoding: base64\r\n\r\nAP8=\r\n--b--\r\n";
        let first = save_eml(&dir, raw, 5).unwrap();
        let second = save_eml(&dir, raw, 5).unwrap();
        assert_eq!(first.file_name().unwrap(), "测试.eml");
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), raw);
        assert_eq!(std::fs::read(second).unwrap(), raw);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn filename_is_a_bounded_safe_unicode_component() {
        let clean = safe_stem(&format!("../a\\b:c\r\n\0{}", "中文📨".repeat(200)), 42);
        assert!(!clean.contains(['/', '\\', ':']));
        assert!(!clean.chars().any(char::is_control));
        assert!(!clean.starts_with('.'));
        assert!(clean.len() <= 180);
        assert_eq!(safe_stem(". \n", 42), "邮件-42");
        assert_eq!(safe_stem("CON", 42), "_CON");
        assert_eq!(safe_stem("a*b?c\"d<e>f|g", 42), "a_b_c_d_e_f_g");
        assert!(safe_stem(&format!("CON.{}", "中文".repeat(200)), 42).len() <= 180);
    }

    #[test]
    fn filesystem_rejected_unicode_is_replaced_without_losing_readable_subject() {
        for character in ['\u{03a2}', '\u{0378}', '\u{038b}', '\u{fdd0}', '\u{fdef}'] {
            assert_eq!(safe_stem(&format!("通知{character}📨"), 644), "通知_📨");
        }
        for plane in 0..=16 {
            for offset in [0xfffe, 0xffff] {
                let character = char::from_u32(plane * 0x10000 + offset).unwrap();
                assert_eq!(safe_stem(&format!("邮件{character}"), 644), "邮件_");
            }
        }
        // Assigned symbols, joiners and private-use characters are not unassigned.
        let preserved = "中文é📨\u{fffd}\u{200d}\u{feff}\u{e000}";
        assert_eq!(safe_stem(preserved, 644), preserved);
        assert!(safe_stem(&"通知\u{03a2}📨".repeat(100), 644).len() <= 180);
    }

    #[test]
    fn exporting_rejected_unicode_subjects_preserves_bytes_and_same_name_messages() {
        let dir = tempfile::tempdir().unwrap();
        let raw = "Subject: 通知\u{03a2}\u{ffff}📨\r\n\r\noriginal MIME bytes";
        let first = save_eml(dir.path(), raw.as_bytes(), 644).unwrap();
        let second = save_eml(dir.path(), raw.as_bytes(), 644).unwrap();
        assert_eq!(first.file_name().unwrap(), "通知__📨.eml");
        assert_eq!(second.file_name().unwrap(), "通知__📨 (2).eml");
        assert_eq!(fs::read(first).unwrap(), raw.as_bytes());
        assert_eq!(fs::read(second).unwrap(), raw.as_bytes());
    }

    #[test]
    fn assigned_unicode_subjects_follow_the_destination_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        for character in [
            '\u{1fae9}',
            '\u{1faea}',
            '\u{1faef}',
            '\u{1cc00}',
            '\u{16100}',
            '\u{1e6c0}',
        ] {
            let raw = format!("Subject: 通知{character}📨\r\n\r\noriginal MIME bytes");
            let path = save_eml(dir.path(), raw.as_bytes(), 644).unwrap();
            assert!(
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("通知")
            );
            assert!(path.file_name().unwrap().to_str().unwrap().contains('📨'));
            assert_eq!(fs::read(&path).unwrap(), raw.as_bytes());
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 6);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn filename_retry_is_bounded_and_does_not_hide_other_io_failures() {
        let dir = tempfile::tempdir().unwrap();
        for code in [13, 28, 63] {
            // EACCES, ENOSPC, ENAMETOOLONG on Darwin.
            let mut attempts = 0;
            let result: Result<()> = with_filesystem_stem(dir.path(), "通知\u{03a2}", |_| {
                attempts += 1;
                Err(std::io::Error::from_raw_os_error(code)).context("simulated write failure")
            });
            assert_eq!(attempts, 1);
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(code)
            );
        }
        for (stem, expected_attempts) in [("通知\u{03a2}", 2), ("通知", 1)] {
            let mut attempts = 0;
            let result: Result<()> = with_filesystem_stem(dir.path(), stem, |_| {
                attempts += 1;
                Err(std::io::Error::from_raw_os_error(92)).context("simulated filename rejection")
            });
            assert_eq!(attempts, expected_attempts);
            assert_eq!(
                result
                    .unwrap_err()
                    .downcast::<std::io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(92)
            );
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_downloads_keep_every_original_message() {
        let dir = std::env::temp_dir().join(format!(
            "mailpulse-export-concurrent-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let raw =
                        format!("Subject: same name\r\n\r\noriginal {index}\r\n").into_bytes();
                    let path = save_eml(&dir, &raw, index).unwrap();
                    assert_eq!(std::fs::read(&path).unwrap(), raw);
                    path
                })
            })
            .collect();
        let paths: std::collections::HashSet<_> =
            workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(paths.len(), 8);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 8);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn interrupted_copy_never_exposes_a_partial_final_eml() {
        let dir = tempfile::tempdir().unwrap();
        struct InterruptedSource {
            final_path: PathBuf,
            premature: std::sync::Arc<std::sync::atomic::AtomicBool>,
            supplied: bool,
        }
        impl Read for InterruptedSource {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.final_path.exists() {
                    self.premature
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                if self.supplied {
                    return Err(std::io::Error::other("simulated copy interruption"));
                }
                self.supplied = true;
                out[..7].copy_from_slice(b"partial");
                Ok(7)
            }
        }
        let final_path = dir.path().join("Interrupted.eml");
        let premature = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        assert!(
            save_eml_from_reader(
                dir.path(),
                "Interrupted",
                InterruptedSource {
                    final_path: final_path.clone(),
                    premature: premature.clone(),
                    supplied: false,
                }
            )
            .is_err()
        );
        assert!(
            !premature.load(std::sync::atomic::Ordering::SeqCst),
            "a final EML name was visible during an incomplete copy"
        );
        assert!(!final_path.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
