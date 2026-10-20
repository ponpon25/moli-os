//! A gRPC call to the Olvid daemon, by hand: HTTP/2 without TLS (the daemon
//! is on this machine or its Docker network), one message in, the messages
//! out. Each call opens its own connection: a notification a few times a day
//! does not deserve a pool.

use std::time::Duration;

use anyhow::{Context as _, bail, ensure};
use http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use hyper::body::Bytes;
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpStream;

/// Nothing the daemon answers is larger.
const MAX_ANSWER: usize = 1 << 20;
const TIMEOUT: Duration = Duration::from_secs(15);

/// Where the daemon is and the key that opens it (a client key of one
/// identity, or the administrator's).
#[derive(Clone)]
pub struct Daemon {
    address: String,
    key: String,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key never reaches a log.
        f.debug_struct("Daemon")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Daemon {
    pub fn new(address: &str, key: &str) -> Self {
        Self {
            address: address.to_owned(),
            key: key.to_owned(),
        }
    }

    /// One call: `path` is `/<package>.<Service>/<Method>`. Returns every
    /// message of the answer (one for a plain call, several for a stream).
    pub async fn call(&self, path: &str, message: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
        tokio::time::timeout(TIMEOUT, self.exchange(path, message))
            .await
            .context("the Olvid daemon did not answer in time")?
    }

    async fn exchange(&self, path: &str, message: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
        let tcp = TcpStream::connect(&self.address)
            .await
            .with_context(|| format!("connecting to {}", self.address))?;
        tcp.set_nodelay(true)?;
        let (mut sender, connection) = http2::handshake(TokioExecutor::new(), TokioIo::new(tcp))
            .await
            .context("HTTP/2 handshake")?;
        tokio::spawn(async move {
            // Ends when the sender is dropped, or the daemon hangs up.
            let _ = connection.await;
        });
        let request = Request::post(format!("http://{}{path}", self.address))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .header("user-agent", "moli-os")
            .header("daemon-client-key", &self.key)
            .body(OneShot(Some(Bytes::from(frame(message)))))?;
        let response = sender.send_request(request).await.context("request")?;
        ensure!(
            response.status() == StatusCode::OK,
            "the daemon answered HTTP {}",
            response.status()
        );
        let (head, mut body) = response.into_parts();
        let mut data = Vec::new();
        let mut trailers = head.headers.clone();
        while let Some(piece) = body.frame().await {
            let piece = piece.context("reading the answer")?;
            match piece.into_data() {
                Ok(bytes) => {
                    ensure!(
                        data.len() + bytes.len() <= MAX_ANSWER,
                        "the daemon's answer is too large"
                    );
                    data.extend_from_slice(&bytes);
                }
                Err(piece) => {
                    if let Ok(t) = piece.into_trailers() {
                        trailers.extend(t);
                    }
                }
            }
        }
        check_status(
            trailers.get("grpc-status").and_then(|v| v.to_str().ok()),
            trailers.get("grpc-message").and_then(|v| v.to_str().ok()),
        )?;
        unframe(&data)
    }
}

/// A request body of one piece whose length is not announced: gRPC servers
/// are not fond of a `content-length` on a call.
struct OneShot(Option<Bytes>);

impl hyper::body::Body for OneShot {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        std::task::Poll::Ready(self.0.take().map(|data| Ok(hyper::body::Frame::data(data))))
    }
}

/// The 5 bytes gRPC puts before a message: not compressed, then its length.
pub fn frame(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(message.len() + 5);
    out.push(0);
    out.extend_from_slice(
        &u32::try_from(message.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    out.extend_from_slice(message);
    out
}

/// The messages of an answer body.
pub fn unframe(mut data: &[u8]) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    while !data.is_empty() {
        ensure!(data.len() >= 5, "gRPC: truncated message header");
        ensure!(data[0] == 0, "gRPC: compressed messages are not supported");
        let n = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
        ensure!(data.len() >= 5 + n, "gRPC: truncated message");
        out.push(data[5..5 + n].to_vec());
        data = &data[5 + n..];
    }
    Ok(out)
}

fn check_status(status: Option<&str>, message: Option<&str>) -> anyhow::Result<()> {
    let Some(status) = status else {
        bail!("the daemon sent no gRPC status");
    };
    if status == "0" {
        return Ok(());
    }
    let words = message.map(decode_message).unwrap_or_default();
    bail!(
        "gRPC status {status} ({}){}",
        status_name(status),
        if words.is_empty() {
            String::new()
        } else {
            format!(": {words}")
        }
    )
}

fn status_name(code: &str) -> &'static str {
    match code {
        "1" => "cancelled",
        "2" => "unknown",
        "3" => "invalid argument",
        "4" => "deadline exceeded",
        "5" => "not found",
        "6" => "already exists",
        "7" => "permission denied",
        "8" => "resource exhausted",
        "9" => "failed precondition",
        "12" => "unimplemented",
        "13" => "internal",
        "14" => "unavailable",
        "16" => "unauthenticated",
        _ => "error",
    }
}

/// `grpc-message` is percent-encoded.
fn decode_message(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| char::from(b).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(u8::try_from(h * 16 + l).unwrap_or(b'?'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_framed_and_unframed() {
        let framed = frame(b"abc");
        assert_eq!(framed, [0, 0, 0, 0, 3, b'a', b'b', b'c']);
        let two = [frame(b"ab"), frame(b""), frame(b"xyz")].concat();
        let all = unframe(&two).unwrap();
        assert_eq!(all, vec![b"ab".to_vec(), Vec::new(), b"xyz".to_vec()]);
        assert!(unframe(&[]).unwrap().is_empty());
    }

    #[test]
    fn a_damaged_body_is_refused() {
        assert!(unframe(&[0, 0, 0]).is_err());
        assert!(unframe(&[0, 0, 0, 0, 9, 1]).is_err());
        assert!(unframe(&[1, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn statuses_say_what_went_wrong() {
        assert!(check_status(Some("0"), None).is_ok());
        let e = check_status(Some("16"), Some("bad%20client%20key")).unwrap_err();
        assert_eq!(
            e.to_string(),
            "gRPC status 16 (unauthenticated): bad client key"
        );
        assert!(check_status(None, None).is_err());
    }

    #[test]
    fn the_key_stays_out_of_logs() {
        let d = Daemon::new("127.0.0.1:50051", "secret-key");
        assert!(!format!("{d:?}").contains("secret-key"));
    }
}
