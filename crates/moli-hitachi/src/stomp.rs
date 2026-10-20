//! The little STOMP 1.2 the app's notification service speaks over its
//! WebSocket: one frame per text message, ended by a NUL byte. A message
//! holding only line feeds is a heart-beat.

use serde_json::Value as Json;

/// A frame received from the service.
#[derive(Debug, PartialEq)]
pub enum Frame {
    /// A bare line feed: the service is alive.
    HeartBeat,
    Frame {
        command: String,
        headers: Vec<(String, String)>,
        body: String,
    },
}

/// Reads one text message.
#[must_use]
pub fn parse(text: &str) -> Frame {
    let text = text.trim_start_matches(['\n', '\r']);
    let text = text.strip_suffix('\0').unwrap_or(text);
    if text.is_empty() {
        return Frame::HeartBeat;
    }
    let (head, body) = text.split_once("\n\n").unwrap_or((text, ""));
    let mut lines = head.lines();
    let command = lines.next().unwrap_or_default().trim().to_owned();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Frame::Frame {
        command,
        headers,
        body: body.to_owned(),
    }
}

/// The `CONNECT` frame: the access token is the only credential.
#[must_use]
pub fn connect(token: &str) -> String {
    format!(
        "CONNECT\naccept-version:1.1,1.2\nAuthorization:Bearer {token}\nheart-beat:0,30000\n\n\0"
    )
}

#[must_use]
pub fn subscribe(destination: &str, token: &str) -> String {
    format!(
        "SUBSCRIBE\nid:{destination}\ndestination:{destination}\nack:auto\nAuthorization:Bearer {token}\nheart-beat:0,25000\n\n\0"
    )
}

#[must_use]
pub fn send(destination: &str, body: &Json, token: &str) -> String {
    let body = body.to_string();
    format!(
        "SEND\ndestination:{destination}\ncontent-type:application/json;charset=UTF-8\nAuthorization:Bearer {token}\ncontent-length:{}\n\n{body}\0",
        body.len()
    )
}

/// Where the service publishes the household's units.
#[must_use]
pub fn notifications(user: i64, family: i64) -> String {
    format!("/notification/{user}/{family}")
}

/// Where the app asks for a fresh picture of all units.
#[must_use]
pub fn requests(user: i64, family: i64) -> String {
    format!("/app/racs/{user}/{family}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_read() {
        assert_eq!(parse("\n"), Frame::HeartBeat);
        assert_eq!(parse(""), Frame::HeartBeat);
        let Frame::Frame {
            command,
            headers,
            body,
        } = parse(
            "MESSAGE\ndestination:/notification/1/2\ncontent-type:application/json\n\n{\"a\":1}\0",
        )
        else {
            panic!("a frame")
        };
        assert_eq!(command, "MESSAGE");
        assert_eq!(
            headers[0],
            ("destination".into(), "/notification/1/2".into())
        );
        assert_eq!(body, "{\"a\":1}");
        let Frame::Frame { command, .. } = parse("CONNECTED\nversion:1.2\n\n\0") else {
            panic!("a frame")
        };
        assert_eq!(command, "CONNECTED");
    }

    #[test]
    fn frames_are_written() {
        let frame = send("/app/racs/1/2", &serde_json::json!({"racId": 1}), "tok");
        assert!(frame.starts_with("SEND\ndestination:/app/racs/1/2\n"));
        assert!(frame.contains("content-length:11\n\n{\"racId\":1}\0"));
        assert!(frame.ends_with('\0'));
        assert!(connect("tok").contains("Authorization:Bearer tok"));
    }
}
