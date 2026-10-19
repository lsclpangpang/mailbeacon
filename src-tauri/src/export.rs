//! 将完整邮件原文原样存为 .eml，不改变 MIME、附件或邮件头。

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use mail_parser::MessageParser;

fn safe_stem(subject: &str, uid: u32) -> String {
    let clean: String = subject
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
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
    let message = MessageParser::default().parse_headers(raw);
    safe_stem(
        message.as_ref().and_then(|m| m.subject()).unwrap_or(""),
        uid,
    )
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
    for index in 1u64.. {
        let name = if index == 1 {
            format!("{stem}.eml")
        } else {
            format!("{stem} ({index}).eml")
        };
        let path = dir.join(name);
        match temporary.persist_noclobber(&path) {
            Ok(_) => {
                #[cfg(unix)]
                fs::File::open(dir)?
                    .sync_all()
                    .context("确认邮件目录写入失败")?;
                return Ok(path);
            }
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                temporary = error.file;
            }
            Err(error) => return Err(error.error).context("无法保存邮件到下载文件夹"),
        }
    }
    unreachable!("无法耗尽全部下载文件名")
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
