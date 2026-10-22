//! Bounded HTTP image downloads without an external platform-specific executable.

use std::{io::Read, time::Duration};

fn allowed(url: &reqwest::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            !host
                .trim_end_matches('.')
                .eq_ignore_ascii_case("tauri.localhost")
        })
}

/// Download only HTTP(S), bounding both advertised and streamed bytes and redirects.
pub fn fetch(url: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let url = reqwest::Url::parse(url).map_err(|_| "不支持打开这种图片地址")?;
    if !allowed(&url) {
        return Err("不支持打开这种图片地址".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if !allowed(attempt.url()) || attempt.previous().len() >= 5 {
                attempt.error("不支持的图片跳转地址")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| format!("下载图片失败：{e}"))?;
    let response = client
        .get(url)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|e| format!("下载图片失败：{e}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err("图片太大，无法打开".into());
    }
    let mut bytes = Vec::new();
    response
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("下载图片失败：{e}"))?;
    if bytes.len() > max_bytes {
        return Err("图片太大，无法打开".into());
    }
    if bytes.is_empty() {
        return Err("下载图片失败：内容为空".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn response(bytes: Vec<u8>) -> String {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/image", server.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&bytes);
        });
        url
    }

    #[test]
    fn downloads_bytes_and_rejects_empty_and_error_responses() {
        let url = response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\npng".to_vec());
        assert_eq!(fetch(&url, 8).unwrap(), b"png");
        let url = response(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec());
        assert!(fetch(&url, 8).is_err());
        let url = response(b"HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\n\r\nerr".to_vec());
        assert!(fetch(&url, 8).is_err());
    }

    #[test]
    fn caps_response_even_when_content_length_is_missing() {
        for header in ["Content-Length: 9\r\n", "Connection: close\r\n"] {
            let url = response(format!("HTTP/1.1 200 OK\r\n{header}\r\n123456789").into_bytes());
            assert!(fetch(&url, 8).is_err());
        }
    }

    #[test]
    fn rejects_unsafe_urls_including_redirect_destinations() {
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/a",
            "http://tauri.localhost/x",
            "http://u:p@example.com/a",
        ] {
            assert!(fetch(url, 8).is_err());
            let redirect = response(
                format!("HTTP/1.1 302 Found\r\nLocation: {url}\r\nContent-Length: 0\r\n\r\n")
                    .into_bytes(),
            );
            assert!(fetch(&redirect, 8).is_err());
        }
    }
}
