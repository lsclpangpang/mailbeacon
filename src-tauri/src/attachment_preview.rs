//! Bounded image previews and cancellable native PDF preview leases.
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tauri::Emitter;
use tokio::sync::watch;

const MAX_IMAGE: usize = 10 * 1024 * 1024;
const MAX_PDF: usize = 20 * 1024 * 1024;
const MAX_PIXELS: u64 = 24_000_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub request_id: String,
    pub kind: String,
    pub filename: String,
    pub mime: String,
    pub data_url: Option<String>,
}
#[derive(Default)]
struct Registry {
    jobs: HashMap<String, (String, watch::Sender<bool>)>,
    cancelled: VecDeque<String>,
}
fn registry() -> &'static Mutex<Registry> {
    static STORE: OnceLock<Mutex<Registry>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Registry::default()))
}

/// Register before any asynchronous read so closing the UI also cancels downloads.
pub fn register(id: &str, account_id: &str) -> Result<watch::Receiver<bool>> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        bail!("预览请求标识无效");
    }
    let mut jobs = registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("预览状态不可用"))?;
    if jobs.cancelled.iter().any(|x| x == id) {
        bail!("预览已取消");
    }
    if jobs.jobs.contains_key(id) {
        bail!("预览请求已存在");
    }
    if jobs.jobs.len() >= 8 {
        bail!("预览任务过多，请先关闭已有预览");
    }
    let (tx, rx) = watch::channel(false);
    jobs.jobs.insert(id.into(), (account_id.into(), tx));
    Ok(rx)
}

/// Cancel both pending reads and native windows; retain a bounded pre-start tombstone.
pub fn cancel(id: &str) -> Result<()> {
    let mut jobs = registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("预览状态不可用"))?;
    if let Some((_, job)) = jobs.jobs.get(id) {
        let _ = job.send(true);
    }
    if !jobs.cancelled.iter().any(|x| x == id) {
        jobs.cancelled.push_back(id.into());
    }
    while jobs.cancelled.len() > 1024 {
        jobs.cancelled.pop_front();
    }
    Ok(())
}
/// Release bookkeeping after the worker has dropped its temporary-file lease.
pub fn finish(id: &str) {
    if let Ok(mut jobs) = registry().lock() {
        jobs.jobs.remove(id);
    }
}

/// Cancellation discards the result after draining an in-flight shared protocol response.
pub async fn drain_on_cancel<T>(
    cancel: &mut watch::Receiver<bool>,
    read: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    if *cancel.borrow() {
        bail!("预览已取消");
    }
    tokio::pin!(read);
    let result = tokio::select! {
        result=&mut read=>result,
        _=cancel.changed()=>{let _=read.await;bail!("预览已取消")},
    };
    if *cancel.borrow() {
        bail!("预览已取消");
    }
    result
}

/// Close native previews and pending reads when the account identity is removed.
pub fn cancel_account(account_id: &str) {
    if let Ok(jobs) = registry().lock() {
        for (account, job) in jobs.jobs.values() {
            if account == account_id {
                let _ = job.send(true);
            }
        }
    }
}

fn dimensions(data: &[u8], mime: &str) -> Option<(u32, u32)> {
    match mime {
        "image/png" if data.len() >= 24 && &data[12..16] == b"IHDR" => Some((
            u32::from_be_bytes(data[16..20].try_into().ok()?),
            u32::from_be_bytes(data[20..24].try_into().ok()?),
        )),
        "image/gif" if data.len() >= 10 => Some((
            u16::from_le_bytes(data[6..8].try_into().ok()?) as u32,
            u16::from_le_bytes(data[8..10].try_into().ok()?) as u32,
        )),
        "image/webp" if data.len() >= 30 && &data[12..16] == b"VP8X" => {
            let le24 = |p: usize| {
                u32::from(data[p]) | (u32::from(data[p + 1]) << 8) | (u32::from(data[p + 2]) << 16)
            };
            Some((le24(24) + 1, le24(27) + 1))
        }
        "image/webp" if data.len() >= 25 && &data[12..16] == b"VP8L" && data[20] == 0x2f => {
            let bits = u32::from_le_bytes(data[21..25].try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        "image/webp"
            if data.len() >= 30 && &data[12..16] == b"VP8 " && &data[23..26] == b"\x9d\x01\x2a" =>
        {
            Some((
                (u16::from_le_bytes(data[26..28].try_into().ok()?) & 0x3fff) as u32,
                (u16::from_le_bytes(data[28..30].try_into().ok()?) & 0x3fff) as u32,
            ))
        }
        "image/jpeg" => {
            let mut n = 2;
            while n + 4 <= data.len() {
                if data[n] != 0xff {
                    return None;
                }
                while n < data.len() && data[n] == 0xff {
                    n += 1;
                }
                let marker = *data.get(n)?;
                n += 1;
                if marker == 0xd9 || marker == 0xda {
                    return None;
                }
                if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                    continue;
                }
                let len = u16::from_be_bytes(data.get(n..n + 2)?.try_into().ok()?) as usize;
                if len < 2 || n.checked_add(len)? > data.len() {
                    return None;
                }
                if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) && len >= 7 {
                    return Some((
                        u16::from_be_bytes(data[n + 5..n + 7].try_into().ok()?) as u32,
                        u16::from_be_bytes(data[n + 3..n + 5].try_into().ok()?) as u32,
                    ));
                }
                n += len;
            }
            None
        }
        _ => None,
    }
}

/// Detect actual bytes, never trust a filename or a declared MIME type.
pub fn content_kind(data: &[u8]) -> Result<&'static str> {
    let mime = if data.starts_with(b"%PDF-") {
        "application/pdf"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        "image/gif"
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        bail!("此文件类型不支持安全快速预览，请使用打开或下载");
    };
    if mime == "application/pdf" {
        if data.len() > MAX_PDF {
            bail!("PDF超过20MB预览预算，请使用打开或下载");
        }
    } else {
        if data.len() > MAX_IMAGE {
            bail!("图片超过10MB预览预算，请使用打开或下载");
        }
        let (w, h) = dimensions(data, mime).context("图片结构损坏，无法安全预览")?;
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
            bail!("图片像素超过预览预算，请使用打开或下载");
        }
    }
    Ok(mime)
}

/// Produce a non-executable image URL after format and decompression-size checks.
pub fn image(id: &str, name: String, data: &[u8]) -> Result<Preview> {
    let mime = content_kind(data)?;
    if mime == "application/pdf" {
        bail!("PDF需要原生预览");
    }
    Ok(Preview {
        request_id: id.into(),
        kind: "image".into(),
        filename: name,
        mime: mime.into(),
        data_url: Some(format!("data:{mime};base64,{}", STANDARD.encode(data))),
    })
}

/// Own the PDF file until the native preview exits or cancellation/timeout kills the child.
pub async fn native_pdf(
    app: tauri::AppHandle,
    id: String,
    name: String,
    data: Vec<u8>,
    mut cancel: watch::Receiver<bool>,
) -> Result<Preview> {
    if content_kind(&data)? != "application/pdf" {
        bail!("不是有效的PDF预览类型");
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id, name, data, cancel);
        bail!("此平台暂不支持PDF快速预览，请使用默认应用打开");
    }
    #[cfg(target_os = "macos")]
    {
        if *cancel.borrow() {
            bail!("预览已取消");
        }
        let dir = tempfile::Builder::new()
            .prefix("mailbeacon-preview-")
            .tempdir()?;
        let path = dir.path().join("document.pdf");
        tokio::fs::write(&path, &data).await?;
        // Bound the initial native decoding pass before opening a long-lived viewer.
        // Quick Look renders outside the application's WebView and never receives HTML.
        let mut check = tokio::process::Command::new("/usr/bin/qlmanage")
            .args(["-t", "-s", "256", "-o"])
            .arg(dir.path())
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("系统PDF解析器无法启动")?;
        let decoded = tokio::select! {
            status=check.wait()=>status.context("系统PDF解析器失败")?,
            _=cancel.changed()=>{let _=check.kill().await;bail!("预览已取消")},
            _=tokio::time::sleep(Duration::from_secs(12))=>{let _=check.kill().await;bail!("PDF解析超时，请使用打开或下载")},
        };
        if !decoded.success() {
            bail!("系统无法预览此PDF，请使用打开或下载");
        }
        let thumbnail = dir.path().join("document.pdf.png");
        let meta = tokio::fs::metadata(&thumbnail)
            .await
            .context("系统没有生成PDF预览，请使用打开或下载")?;
        if !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
            bail!("PDF预览超过解析预算，请使用打开或下载");
        }
        tokio::fs::remove_file(thumbnail).await?;
        if *cancel.borrow() {
            bail!("预览已取消");
        }
        let mut child = tokio::process::Command::new("/usr/bin/qlmanage")
            .arg("-p")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("系统PDF预览无法启动")?;
        if *cancel.borrow() {
            let _ = child.kill().await;
            bail!("预览已取消");
        }
        let response = Preview {
            request_id: id.clone(),
            kind: "nativePdf".into(),
            filename: name,
            mime: "application/pdf".into(),
            data_url: None,
        };
        tauri::async_runtime::spawn(async move {
            let error = tokio::select! {
                status=child.wait()=>match status{Ok(s) if s.success()=>None,Ok(s)=>Some(format!("系统预览退出：{s}")),Err(e)=>Some(e.to_string())},
                _=cancel.changed()=>{let _=child.kill().await;None},
                _=tokio::time::sleep(Duration::from_secs(30*60))=>{let _=child.kill().await;Some("预览已超时关闭".into())},
            };
            drop(dir);
            finish(&id);
            let _ = app.emit(
                "attachment-preview-closed",
                serde_json::json!({"requestId":id,"error":error}),
            );
        });
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_svg_and_spoofed_extensions_never_produce_preview_urls() {
        assert!(content_kind(b"<svg onload=alert(1)>").is_err());
        assert!(content_kind(b"<html>not really a PDF</html>").is_err());
        assert!(content_kind(b"PK macro workbook").is_err());
    }
    #[test]
    fn oversized_dimensions_are_rejected_before_browser_decode() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&100_000u32.to_be_bytes());
        png.extend_from_slice(&100_000u32.to_be_bytes());
        assert!(content_kind(&png).is_err());
    }
    #[test]
    fn cancel_before_start_and_duplicate_jobs_are_rejected() {
        cancel("test-prestart").unwrap();
        assert!(register("test-prestart", "preview-test-a").is_err());
        let rx = register("test-live", "preview-test-a").unwrap();
        assert!(register("test-live", "preview-test-a").is_err());
        cancel("test-live").unwrap();
        assert!(*rx.borrow());
        finish("test-live");
    }
    #[test]
    fn account_cancel_only_closes_its_own_jobs() {
        let a = register("test-account-a", "preview-scope-a").unwrap();
        let b = register("test-account-b", "preview-scope-b").unwrap();
        cancel_account("preview-scope-a");
        assert!(*a.borrow());
        assert!(!*b.borrow());
        finish("test-account-a");
        finish("test-account-b");
    }
    #[tokio::test]
    async fn cancelling_an_inflight_read_drains_it_without_publishing_bytes() {
        let (tx, mut rx) = watch::channel(false);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            drain_on_cancel(&mut rx, async move {
                started_tx.send(()).unwrap();
                done_rx.await.unwrap();
                Ok(b"must not publish".to_vec())
            })
            .await
        });
        started_rx.await.unwrap();
        tx.send(true).unwrap();
        tokio::task::yield_now().await;
        assert!(
            !task.is_finished(),
            "keep the protocol lane until the current response is drained"
        );
        done_tx.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
    }
}
