//! One-shot localhost callback server for the OAuth redirect.
//!
//! Per RFC 8252 we bind to `127.0.0.1:0` so the OS picks a free
//! ephemeral port; the caller reads the assigned port back via
//! `local_addr()` and uses it to construct the `redirect_uri` for
//! the authorize URL.
//!
//! Tiny hand-parsed HTTP/1.1 — we only ever need to read the request
//! line, so a full HTTP server crate would be overkill.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::x::Error;

pub struct Callback {
    pub code:  Option<String>,
    pub error: Option<String>,
    pub state: Option<String>,
}

/// Bind a one-shot listener on `127.0.0.1:0`. Returns the OS-assigned
/// port and a future that resolves with the captured callback (or a
/// timeout error). The caller uses the port to build the authorize
/// URL before launching chromium, then awaits the future.
pub async fn bind_and_await(
    timeout: Duration,
) -> Result<(u16, impl std::future::Future<Output = Result<Callback, Error>>), Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await
        .map_err(|e| Error::Other(format!("oauth: bind 127.0.0.1:0 failed: {e}")))?;
    let port = listener.local_addr()
        .map_err(|e| Error::Other(format!("oauth: local_addr failed: {e}")))?
        .port();

    let fut = async move {
        let accept = async {
            // Single accept — we only handle one redirect, then drop.
            let (mut socket, _peer) = listener.accept().await
                .map_err(|e| Error::Other(format!("oauth: accept failed: {e}")))?;

            // Read until end of headers (`\r\n\r\n`). A redirect GET is
            // tiny — bound the read to 8 KiB to avoid pathological inputs.
            let mut buf = Vec::with_capacity(2048);
            let mut chunk = [0u8; 1024];
            loop {
                let n = socket.read(&mut chunk).await
                    .map_err(|e| Error::Other(format!("oauth: read failed: {e}")))?;
                if n == 0 { break; }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") { break; }
                if buf.len() > 8192 {
                    return Err(Error::Other("oauth: request too large".into()));
                }
            }

            let header = std::str::from_utf8(&buf).map_err(|e| {
                Error::Other(format!("oauth: non-UTF-8 request: {e}"))
            })?;
            let request_line = header.lines().next().unwrap_or("");
            // Format: "GET /callback?code=…&state=… HTTP/1.1"
            let mut it = request_line.split_whitespace();
            let _method = it.next();
            let target = it.next().unwrap_or("");
            let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
            let params: HashMap<String, String> = query
                .split('&')
                .filter(|p| !p.is_empty())
                .filter_map(|p| {
                    let (k, v) = p.split_once('=')?;
                    let k = urlencoding::decode(k).ok()?.into_owned();
                    let v = urlencoding::decode(v).ok()?.into_owned();
                    Some((k, v))
                })
                .collect();

            // Reply with a small success/failure page so the operator
            // sees something other than "Connection refused".
            let body = if params.contains_key("code") {
                "<!doctype html><meta charset=utf-8>\
                 <title>Authorization complete</title>\
                 <body style='font:16px system-ui;padding:2em'>\
                 <h2>Authorization complete</h2>\
                 <p>You can close this tab and return to the terminal.</p>"
            } else {
                "<!doctype html><meta charset=utf-8>\
                 <title>Authorization failed</title>\
                 <body style='font:16px system-ui;padding:2em'>\
                 <h2>Authorization failed</h2>\
                 <p>Check the terminal for details. You can close this tab.</p>"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/html; charset=utf-8\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
                body.len(), body,
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;

            Ok(Callback {
                code:  params.get("code").cloned(),
                error: params.get("error").cloned(),
                state: params.get("state").cloned(),
            })
        };

        match tokio::time::timeout(timeout, accept).await {
            Ok(result) => result,
            Err(_) => Err(Error::Other(format!(
                "oauth: timed out after {} seconds waiting for callback",
                timeout.as_secs(),
            ))),
        }
    };

    Ok((port, fut))
}
