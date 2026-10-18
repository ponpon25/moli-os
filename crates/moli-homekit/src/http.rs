//! The little HTTP/1.1 HAP speaks: requests with a `Content-Length`, no
//! chunking, plus `EVENT/1.0` messages the accessory sends on its own.

use serde_json::Value as Json;

const MAX_HEAD: usize = 8 * 1024;
const MAX_BODY: usize = 64 * 1024;

pub const TLV: &str = "application/pairing+tlv8";
pub const JSON: &str = "application/hap+json";

#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// More bytes are needed.
    Incomplete,
    /// Not something to answer: the connection is dropped.
    Bad,
    Done(Request, usize),
}

pub fn parse(buf: &[u8]) -> Parsed {
    let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if buf.len() > MAX_HEAD {
            Parsed::Bad
        } else {
            Parsed::Incomplete
        };
    };
    if end > MAX_HEAD {
        return Parsed::Bad;
    }
    let Ok(head) = std::str::from_utf8(&buf[..end]) else {
        return Parsed::Bad;
    };
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (Some(method), Some(target), Some(version)) = (first.next(), first.next(), first.next())
    else {
        return Parsed::Bad;
    };
    if !version.starts_with("HTTP/1.") {
        return Parsed::Bad;
    }
    let mut length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if name.eq_ignore_ascii_case("content-length") {
                match value.trim().parse() {
                    Ok(n) => length = n,
                    Err(_) => return Parsed::Bad,
                }
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                return Parsed::Bad;
            }
        }
    }
    if length > MAX_BODY {
        return Parsed::Bad;
    }
    let body_start = end + 4;
    if buf.len() < body_start + length {
        return Parsed::Incomplete;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Parsed::Done(
        Request {
            method: method.to_owned(),
            path: path.to_owned(),
            query: query.to_owned(),
            body: buf[body_start..body_start + length].to_vec(),
        },
        body_start + length,
    )
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn tlv(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            content_type: TLV,
            body,
        }
    }

    pub fn json(status: u16, body: &Json) -> Self {
        Self {
            status,
            content_type: JSON,
            body: serde_json::to_vec(body).expect("json serializes"),
        }
    }

    pub fn empty(status: u16) -> Self {
        Self {
            status,
            content_type: JSON,
            body: Vec::new(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            204 => "No Content",
            207 => "Multi-Status",
            400 => "Bad Request",
            404 => "Not Found",
            405 => "Method Not Allowed",
            470 => "Connection Authorization Required",
            _ => "Internal Server Error",
        };
        let mut out = format!("HTTP/1.1 {} {reason}\r\n", self.status).into_bytes();
        if !self.body.is_empty() {
            out.extend_from_slice(
                format!(
                    "Content-Type: {}\r\nContent-Length: {}\r\n",
                    self.content_type,
                    self.body.len()
                )
                .as_bytes(),
            );
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
        out
    }
}

/// An unsolicited notification (`EVENT/1.0 200 OK`).
pub fn event(body: &Json) -> Vec<u8> {
    let body = serde_json::to_vec(body).expect("json serializes");
    let mut out = format!(
        "EVENT/1.0 200 OK\r\nContent-Type: {JSON}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(&body);
    out
}

/// `id=1.10,1.11` → `[(1, 10), (1, 11)]`; `None` for anything else.
pub fn parse_ids(query: &str) -> Option<Vec<(u64, u64)>> {
    let list = query.split('&').find_map(|kv| kv.strip_prefix("id="))?;
    list.split(',')
        .map(|pair| {
            let (aid, iid) = pair.split_once('.')?;
            Some((aid.parse().ok()?, iid.parse().ok()?))
        })
        .collect()
}

pub fn flag(query: &str, name: &str) -> bool {
    query
        .split('&')
        .any(|kv| kv.strip_prefix(name).is_some_and(|v| v == "=1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_with_a_body() {
        let raw = b"POST /pair-setup HTTP/1.1\r\nHost: moli.local\r\nContent-Type: application/pairing+tlv8\r\nContent-Length: 3\r\n\r\nabcGET /next";
        let Parsed::Done(req, used) = parse(raw) else {
            panic!("complete")
        };
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/pair-setup");
        assert_eq!(req.body, b"abc");
        assert_eq!(&raw[used..], b"GET /next");
    }

    #[test]
    fn a_partial_request_waits() {
        assert_eq!(
            parse(b"GET /accessories HTTP/1.1\r\nHo"),
            Parsed::Incomplete
        );
        let half = b"PUT /characteristics HTTP/1.1\r\nContent-Length: 10\r\n\r\n12345";
        assert_eq!(parse(half), Parsed::Incomplete);
    }

    #[test]
    fn the_query_is_split_off() {
        let Parsed::Done(req, _) =
            parse(b"GET /characteristics?id=1.10,2.11&ev=1 HTTP/1.1\r\n\r\n")
        else {
            panic!()
        };
        assert_eq!(req.path, "/characteristics");
        assert_eq!(parse_ids(&req.query), Some(vec![(1, 10), (2, 11)]));
        assert!(flag(&req.query, "ev"));
        assert!(!flag(&req.query, "meta"));
    }

    #[test]
    fn garbage_and_oversized_input_is_refused() {
        assert_eq!(parse(b"nonsense\r\n\r\n"), Parsed::Bad);
        assert_eq!(parse(b"GET / SPDY/3\r\n\r\n"), Parsed::Bad);
        assert_eq!(
            parse(b"GET / HTTP/1.1\r\nContent-Length: x\r\n\r\n"),
            Parsed::Bad
        );
        assert_eq!(
            parse(b"POST / HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n"),
            Parsed::Bad
        );
        assert_eq!(
            parse(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Parsed::Bad
        );
        assert_eq!(parse(&vec![b'a'; 9000]), Parsed::Bad);
        assert_eq!(parse_ids("id=a.b"), None);
        assert_eq!(parse_ids("other=1.2"), None);
    }

    #[test]
    fn responses_and_events_are_framed() {
        let r = Response::json(200, &serde_json::json!({"a": 1})).encode();
        let text = String::from_utf8(r).unwrap();
        assert!(text.starts_with(
            "HTTP/1.1 200 OK\r\nContent-Type: application/hap+json\r\nContent-Length: 7\r\n\r\n"
        ));
        assert!(text.ends_with("{\"a\":1}"));
        assert_eq!(
            Response::empty(204).encode(),
            b"HTTP/1.1 204 No Content\r\n\r\n"
        );
        let e = String::from_utf8(event(&serde_json::json!({}))).unwrap();
        assert!(e.starts_with("EVENT/1.0 200 OK\r\n"));
        assert!(e.ends_with("\r\n\r\n{}"));
    }
}
