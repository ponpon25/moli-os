//! Sonos tells when something changes (UPnP events, GENA): the player calls
//! this listener back, and the driver reads that player at once instead of
//! waiting for the next round. The event's content is never read, only who
//! sent it: what changed is read from the player itself.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;

/// The longest request head and body read from a player.
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;
const MAX_OPEN: usize = 16;
const PATH: &str = "/n/";

/// The listener: the players that called, and its address for them.
#[derive(Debug)]
pub(crate) struct Listener {
    pub(crate) kicks: mpsc::Receiver<String>,
    pub(crate) port: u16,
    task: JoinHandle<()>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Opens the listener on `port`.
pub(crate) async fn listen(port: u16) -> std::io::Result<Listener> {
    let socket = TcpListener::bind(("0.0.0.0", port)).await?;
    let port = socket.local_addr()?.port();
    let (sender, kicks) = mpsc::channel(64);
    let open = Arc::new(Semaphore::new(MAX_OPEN));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, from)) = socket.accept().await else {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            };
            // Players of the home network only, and never a crowd.
            let Ok(permit) = Arc::clone(&open).try_acquire_owned() else {
                continue;
            };
            if !private(from) {
                continue;
            }
            let sender = sender.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Ok(Some(who)) =
                    tokio::time::timeout(Duration::from_secs(5), serve(stream)).await
                {
                    // Full: the next round reads it anyway.
                    let _ = sender.try_send(who);
                }
            });
        }
    });
    Ok(Listener { kicks, port, task })
}

fn private(from: SocketAddr) -> bool {
    match from.ip() {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback(),
        IpAddr::V6(_) => false,
    }
}

/// Reads one notification and answers it; who sent it.
async fn serve(mut stream: TcpStream) -> Option<String> {
    let mut data = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    let head_end = loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        if data.len() > MAX_HEAD {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let who = notified(&head)?.to_owned();
    // The body is not read for its content, only taken off the connection
    // so the player sees its notification delivered.
    let wanted = content_length(&head).min(MAX_BODY);
    while data.len() < head_end + wanted {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .ok()?;
    Some(who)
}

/// Who a `NOTIFY /n/<player>/<service>` request is from.
fn notified(head: &str) -> Option<&str> {
    let mut line = head.lines().next()?.split(' ');
    if line.next()? != "NOTIFY" {
        return None;
    }
    let path = line.next()?.strip_prefix(PATH)?;
    let who = path.split('/').next()?;
    (!who.is_empty()).then_some(who)
}

fn content_length(head: &str) -> usize {
    head.lines()
        .skip(1)
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0)
}

/// Where a player calls back: this machine's address towards it.
pub(crate) fn callback(player: &str, port: u16, who: &str, service: &str) -> Option<String> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect((player, crate::PORT)).ok()?;
    let ip = probe.local_addr().ok()?.ip();
    Some(format!("http://{ip}:{port}{PATH}{who}/{service}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_who_is_calling() {
        let head = "NOTIFY /n/RINCON_AAA01400/av HTTP/1.1\r\nHOST: 192.168.1.2:3400\r\nContent-Length: 12\r\n\r\n";
        assert_eq!(notified(head), Some("RINCON_AAA01400"));
        assert_eq!(content_length(head), 12);
        assert_eq!(notified("GET /n/RINCON_AAA01400/av HTTP/1.1\r\n\r\n"), None);
        assert_eq!(notified("NOTIFY /other HTTP/1.1\r\n\r\n"), None);
        assert_eq!(notified("NOTIFY /n/ HTTP/1.1\r\n\r\n"), None);
        assert_eq!(content_length("NOTIFY / HTTP/1.1\r\n\r\n"), 0);
    }

    #[tokio::test]
    async fn a_notification_reaches_the_driver() {
        let mut listener = listen(0).await.unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", listener.port))
            .await
            .unwrap();
        stream
            .write_all(b"NOTIFY /n/RINCON_AAA01400/rc HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello")
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"));
        let who = tokio::time::timeout(Duration::from_secs(2), listener.kicks.recv())
            .await
            .unwrap();
        assert_eq!(who.as_deref(), Some("RINCON_AAA01400"));
    }
}
