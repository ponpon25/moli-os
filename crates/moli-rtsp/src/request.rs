//! What comes in on an RTSP connection: requests (a head and maybe a small
//! body), and the client's own RTP/RTCP packets interleaved between them.
//! Everything is bounded: the connection is hostile until it proves the
//! password.

use std::collections::HashMap;

use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, BufReader};

/// Longest head (request line and headers).
const MAX_HEAD: usize = 8 * 1024;
/// Longest body (a client sends none that Moli needs: it is read and dropped).
const MAX_BODY: usize = 4 * 1024;

/// One request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub uri: String,
    /// Header names in lower case.
    pub headers: HashMap<String, String>,
}

impl Request {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    #[must_use]
    pub fn cseq(&self) -> &str {
        self.header("cseq").unwrap_or("0")
    }
}

/// What the client sent next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Request(Request),
    /// An interleaved packet of the client (a receiver report): dropped.
    Interleaved,
}

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("connection closed")]
    Closed,
    #[error("not RTSP: {0}")]
    Malformed(&'static str),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reads the next request or interleaved packet.
pub async fn read<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<Incoming, ReadError> {
    let first = {
        let buf = reader.fill_buf().await?;
        if buf.is_empty() {
            return Err(ReadError::Closed);
        }
        buf[0]
    };
    if first == b'$' {
        let mut head = [0u8; 4];
        reader.read_exact(&mut head).await?;
        let len = u16::from_be_bytes([head[2], head[3]]);
        let mut skip = (&mut *reader).take(u64::from(len));
        tokio::io::copy(&mut skip, &mut tokio::io::sink()).await?;
        return Ok(Incoming::Interleaved);
    }
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        let n = (&mut *reader)
            .take((MAX_HEAD - total + 1) as u64)
            .read_until(b'\n', &mut line)
            .await?;
        if n == 0 {
            return Err(ReadError::Closed);
        }
        total += n;
        if total > MAX_HEAD {
            return Err(ReadError::Malformed("head too long"));
        }
        let line = String::from_utf8(line).map_err(|_| ReadError::Malformed("not text"))?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if lines.is_empty() {
                continue; // a stray empty line between requests
            }
            break;
        }
        lines.push(line.to_owned());
    }
    let request = parse(&lines)?;
    if let Some(len) = request.header("content-length") {
        let len: usize = len
            .trim()
            .parse()
            .map_err(|_| ReadError::Malformed("content-length"))?;
        if len > MAX_BODY {
            return Err(ReadError::Malformed("body too long"));
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).await?;
    }
    Ok(Incoming::Request(request))
}

fn parse(lines: &[String]) -> Result<Request, ReadError> {
    let (first, rest) = lines.split_first().ok_or(ReadError::Malformed("empty"))?;
    let mut parts = first.split(' ');
    let (Some(method), Some(uri), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(ReadError::Malformed("request line"));
    };
    if !version.starts_with("RTSP/1.") {
        return Err(ReadError::Malformed("version"));
    }
    if method.is_empty()
        || method.len() > 32
        || !method.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
    {
        return Err(ReadError::Malformed("method"));
    }
    let mut headers = HashMap::new();
    for line in rest {
        let (name, value) = line.split_once(':').ok_or(ReadError::Malformed("header"))?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    Ok(Request {
        method: method.to_owned(),
        uri: uri.to_owned(),
        headers,
    })
}

/// The camera's path in a request URI: the first segment after the host
/// (`rtsp://host:8554/door/track1` → `door`).
#[must_use]
pub fn camera_path(uri: &str) -> Option<&str> {
    let rest = uri
        .strip_prefix("rtsp://")
        .or_else(|| uri.strip_prefix("RTSP://"))?;
    let (_, path) = rest.split_once('/')?;
    let path = path.split(['?', '#']).next().unwrap_or("");
    path.split('/').next().filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(bytes: &[u8]) -> Vec<Result<Incoming, String>> {
        let mut reader = BufReader::new(bytes);
        let mut out = Vec::new();
        loop {
            match read(&mut reader).await {
                Err(ReadError::Closed) => return out,
                Err(e) => {
                    out.push(Err(e.to_string()));
                    return out;
                }
                Ok(i) => out.push(Ok(i)),
            }
        }
    }

    #[tokio::test]
    async fn requests_and_interleaved_packets_are_told_apart() {
        let bytes = b"OPTIONS rtsp://h/door RTSP/1.0\r\nCSeq: 1\r\n\r\n\
            $\x01\x00\x03abc\
            ANNOUNCE rtsp://h/door RTSP/1.0\r\nCSeq: 2\r\nContent-Length: 4\r\n\r\nbody\
            DESCRIBE rtsp://h/door RTSP/1.0\r\ncseq: 3\r\nAccept: application/sdp\r\n\r\n";
        let got = read_all(bytes).await;
        assert_eq!(got.len(), 4);
        let Ok(Incoming::Request(first)) = &got[0] else {
            panic!("{got:?}")
        };
        assert_eq!(first.method, "OPTIONS");
        assert_eq!(first.cseq(), "1");
        assert_eq!(got[1], Ok(Incoming::Interleaved));
        let Ok(Incoming::Request(last)) = &got[3] else {
            panic!("{got:?}")
        };
        assert_eq!(last.method, "DESCRIBE");
        assert_eq!(last.cseq(), "3");
        assert_eq!(last.header("accept"), Some("application/sdp"));
    }

    #[tokio::test]
    async fn oversized_or_strange_input_is_refused() {
        let mut long = b"OPTIONS rtsp://h/ RTSP/1.0\r\n".to_vec();
        long.extend(std::iter::repeat_n(b'a', MAX_HEAD));
        long.extend_from_slice(b"\r\n\r\n");
        assert!(matches!(read_all(&long).await[..], [Err(_)]));
        assert!(matches!(
            read_all(b"GET / HTTP/1.1\r\n\r\n").await[..],
            [Err(_)]
        ));
        assert!(matches!(
            read_all(b"OPTIONS rtsp://h/ RTSP/1.0\r\nContent-Length: 999999\r\n\r\n").await[..],
            [Err(_)]
        ));
    }

    #[test]
    fn the_camera_path_is_the_first_segment() {
        assert_eq!(camera_path("rtsp://192.168.1.2:8554/door"), Some("door"));
        assert_eq!(
            camera_path("rtsp://192.168.1.2:8554/door/track1"),
            Some("door")
        );
        assert_eq!(camera_path("rtsp://192.168.1.2:8554/door/"), Some("door"));
        assert_eq!(camera_path("rtsp://192.168.1.2:8554/"), None);
        assert_eq!(camera_path("*"), None);
    }
}
