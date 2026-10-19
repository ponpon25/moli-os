//! The `_hap._tcp` announcement (multicast DNS, RFC 6762/6763): just enough
//! of a responder to be found by the Home app. It announces at start and
//! whenever `c#` or the paired flag changes, answers questions about the
//! service, and says goodbye at shutdown.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::Announcement;

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;
const FLUSH: u16 = 0x8000;

/// TTLs: long for what only changes with us, short for the address.
const TTL_LONG: u32 = 4500;
const TTL_SHORT: u32 = 120;

#[derive(Clone, Debug)]
pub struct Service {
    pub name: String,
    pub port: u16,
    pub address: Option<IpAddr>,
}

/// One resource record, before encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub name: String,
    pub kind: u16,
    pub rdata: Rdata,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rdata {
    Ptr(String),
    Srv { port: u16, target: String },
    Txt(Vec<String>),
    A(Ipv4Addr),
}

/// What is announced: the names and the records they answer with.
#[derive(Clone, Debug)]
pub struct Zone {
    instance: String,
    hostname: String,
    records: Vec<Record>,
}

const SERVICE: &str = "_hap._tcp.local";
const ENUMERATION: &str = "_services._dns-sd._udp.local";

fn label_safe(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let cleaned = cleaned.trim_matches('-').to_owned();
    if cleaned.is_empty() {
        "Moli".to_owned()
    } else {
        cleaned
    }
}

impl Zone {
    pub fn new(service: &Service, address: Ipv4Addr, state: &Announcement) -> Self {
        let tail: String = state
            .device_id
            .chars()
            .filter(char::is_ascii_hexdigit)
            .collect::<Vec<_>>()
            .iter()
            .rev()
            .take(4)
            .rev()
            .collect();
        let hostname = format!("{}-{tail}.local", label_safe(&service.name));
        let instance = format!("{}._hap._tcp.local", service.name.replace('.', " "));
        let txt = vec![
            format!("c#={}", state.config_number),
            "ff=0".to_owned(),
            format!("id={}", state.device_id),
            "md=Moli".to_owned(),
            "pv=1.1".to_owned(),
            "s#=1".to_owned(),
            format!("sf={}", u8::from(!state.paired)),
            "ci=2".to_owned(),
            format!("sh={}", state.setup_hash),
        ];
        let records = vec![
            Record {
                name: SERVICE.to_owned(),
                kind: TYPE_PTR,
                rdata: Rdata::Ptr(instance.clone()),
            },
            Record {
                name: ENUMERATION.to_owned(),
                kind: TYPE_PTR,
                rdata: Rdata::Ptr(SERVICE.to_owned()),
            },
            Record {
                name: instance.clone(),
                kind: TYPE_SRV,
                rdata: Rdata::Srv {
                    port: service.port,
                    target: hostname.clone(),
                },
            },
            Record {
                name: instance.clone(),
                kind: TYPE_TXT,
                rdata: Rdata::Txt(txt),
            },
            Record {
                name: hostname.clone(),
                kind: TYPE_A,
                rdata: Rdata::A(address),
            },
        ];
        Self {
            instance,
            hostname,
            records,
        }
    }

    /// Every record (for announcements).
    pub fn all(&self) -> &[Record] {
        &self.records
    }

    /// The records answering a question, plus what goes with them.
    pub fn answer(&self, name: &str, kind: u16) -> Vec<Record> {
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        let mut out: Vec<Record> = self
            .records
            .iter()
            .filter(|r| r.name.to_ascii_lowercase() == name && (kind == TYPE_ANY || kind == r.kind))
            .cloned()
            .collect();
        // A pointer to the service comes with the service's details.
        if name == SERVICE && !out.is_empty() {
            out.extend(
                self.records
                    .iter()
                    .filter(|r| r.name == self.instance || r.name == self.hostname)
                    .cloned(),
            );
        }
        out
    }
}

fn put_name(out: &mut Vec<u8>, name: &str) {
    for label in name.trim_end_matches('.').split('.') {
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    out.push(0);
}

/// An unsolicited or a replying response; `goodbye` sets every TTL to 0.
pub fn encode_response(id: u16, records: &[Record], goodbye: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&0x8400u16.to_be_bytes()); // response, authoritative
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(&(records.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    for record in records {
        put_name(&mut out, &record.name);
        out.extend_from_slice(&record.kind.to_be_bytes());
        let (class, ttl) = match record.kind {
            TYPE_PTR => (CLASS_IN, TTL_LONG),
            TYPE_A | TYPE_SRV => (CLASS_IN | FLUSH, TTL_SHORT),
            _ => (CLASS_IN | FLUSH, TTL_LONG),
        };
        out.extend_from_slice(&class.to_be_bytes());
        out.extend_from_slice(&(if goodbye { 0 } else { ttl }).to_be_bytes());
        let mut rdata = Vec::new();
        match &record.rdata {
            Rdata::Ptr(target) => put_name(&mut rdata, target),
            Rdata::Srv { port, target } => {
                rdata.extend_from_slice(&[0, 0, 0, 0]);
                rdata.extend_from_slice(&port.to_be_bytes());
                put_name(&mut rdata, target);
            }
            Rdata::Txt(strings) => {
                for s in strings {
                    rdata.push(s.len().min(255) as u8);
                    rdata.extend_from_slice(&s.as_bytes()[..s.len().min(255)]);
                }
            }
            Rdata::A(ip) => rdata.extend_from_slice(&ip.octets()),
        }
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(&rdata);
    }
    out
}

/// A question read from a packet.
#[derive(Debug, PartialEq, Eq)]
pub struct Question {
    pub name: String,
    pub kind: u16,
}

fn read_name(packet: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end = None;
    let mut jumps = 0;
    loop {
        let len = *packet.get(at)?;
        if len & 0xC0 == 0xC0 {
            let low = *packet.get(at + 1)?;
            end.get_or_insert(at + 2);
            at = (usize::from(len & 0x3F) << 8) | usize::from(low);
            jumps += 1;
            if jumps > 16 {
                return None;
            }
        } else if len == 0 {
            return Some((labels.join("."), end.unwrap_or(at + 1)));
        } else {
            let start = at + 1;
            let label = packet.get(start..start + usize::from(len))?;
            labels.push(String::from_utf8_lossy(label).into_owned());
            at = start + usize::from(len);
        }
    }
}

/// The id and questions of a query (`None` for responses and junk).
pub fn parse_query(packet: &[u8]) -> Option<(u16, Vec<Question>)> {
    if packet.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([packet[0], packet[1]]);
    let flags = u16::from_be_bytes([packet[2], packet[3]]);
    if flags & 0x8000 != 0 {
        return None;
    }
    let count = usize::from(u16::from_be_bytes([packet[4], packet[5]]));
    let mut at = 12;
    let mut out = Vec::new();
    for _ in 0..count.min(32) {
        let (name, next) = read_name(packet, at)?;
        let kind = u16::from_be_bytes([*packet.get(next)?, *packet.get(next + 1)?]);
        at = next + 4;
        out.push(Question { name, kind });
    }
    Some((id, out))
}

/// Our own address: the one the default route would use.
fn local_address(configured: Option<IpAddr>) -> Option<Ipv4Addr> {
    if let Some(IpAddr::V4(ip)) = configured {
        return Some(ip);
    }
    let probe = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("192.0.2.1:9").ok()?;
    match probe.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

fn open(address: Ipv4Addr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    socket.join_multicast_v4(&GROUP, &address)?;
    socket.set_multicast_if_v4(&address)?;
    socket.set_multicast_loop_v4(false)?;
    socket.set_multicast_ttl_v4(255)?;
    UdpSocket::from_std(socket.into())
}

pub async fn run(
    service: Service,
    mut state: watch::Receiver<Announcement>,
    cancel: CancellationToken,
) {
    // The network may not be up yet: try until it is.
    let (socket, address) = loop {
        if let Some(address) = local_address(service.address) {
            match open(address) {
                Ok(socket) => break (socket, address),
                Err(e) => tracing::warn!(error = %e, "homekit: mDNS socket"),
            }
        } else {
            tracing::warn!("homekit: no network address to announce yet");
        }
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(Duration::from_secs(15)) => {}
        }
    };
    tracing::info!(%address, "homekit: announcing _hap._tcp");
    let group = SocketAddr::V4(SocketAddrV4::new(GROUP, PORT));
    let mut zone = Zone::new(&service, address, &state.borrow().clone());
    let send = |bytes: Vec<u8>, to: SocketAddr| {
        let socket = &socket;
        async move {
            if let Err(e) = socket.send_to(&bytes, to).await {
                tracing::debug!(error = %e, "homekit: mDNS send");
            }
        }
    };
    // Twice, a second apart, as the RFC asks.
    send(encode_response(0, zone.all(), false), group).await;
    let mut again = Some(tokio::time::Instant::now() + Duration::from_secs(1));
    let mut buf = vec![0u8; 2048];
    loop {
        let repeat = async {
            match again {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = cancel.cancelled() => break,
            () = repeat => {
                again = None;
                send(encode_response(0, zone.all(), false), group).await;
            }
            changed = state.changed() => {
                if changed.is_err() { break; }
                let next = Zone::new(&service, address, &state.borrow_and_update().clone());
                // A new identity: the old one is withdrawn first.
                if next.hostname != zone.hostname {
                    send(encode_response(0, zone.all(), true), group).await;
                }
                zone = next;
                send(encode_response(0, zone.all(), false), group).await;
                again = Some(tokio::time::Instant::now() + Duration::from_secs(1));
            }
            received = socket.recv_from(&mut buf) => {
                let Ok((n, from)) = received else { continue };
                let Some((id, questions)) = parse_query(&buf[..n]) else { continue };
                let mut records: Vec<Record> = Vec::new();
                for q in &questions {
                    for r in zone.answer(&q.name, q.kind) {
                        if !records.contains(&r) {
                            records.push(r);
                        }
                    }
                }
                if records.is_empty() { continue; }
                // A resolver that is not a multicast responder (source port
                // other than 5353) wants its answer sent back to it.
                if from.port() == PORT {
                    send(encode_response(0, &records, false), group).await;
                } else {
                    send(encode_response(id, &records, false), from).await;
                }
            }
        }
    }
    // Goodbye: the Home app forgets us at once.
    send(encode_response(0, zone.all(), true), group).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> Zone {
        let service = Service {
            name: "Moli".into(),
            port: 51827,
            address: None,
        };
        Zone::new(
            &service,
            Ipv4Addr::new(192, 168, 1, 10),
            &Announcement {
                config_number: 7,
                paired: false,
                device_id: "AA:BB:CC:DD:EE:FF".into(),
                setup_hash: "rxqrlA==".into(),
            },
        )
    }

    fn query(name: &str, kind: u16) -> Vec<u8> {
        let mut p = vec![0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        put_name(&mut p, name);
        p.extend_from_slice(&kind.to_be_bytes());
        p.extend_from_slice(&[0, 1]);
        p
    }

    /// Reads the records of a response (names may be compressed).
    fn parse_response(p: &[u8]) -> Vec<(String, u16, u32, Vec<u8>)> {
        let count = usize::from(u16::from_be_bytes([p[6], p[7]]));
        let mut at = 12;
        let mut out = Vec::new();
        for _ in 0..count {
            let (name, next) = read_name(p, at).unwrap();
            let kind = u16::from_be_bytes([p[next], p[next + 1]]);
            let ttl = u32::from_be_bytes([p[next + 4], p[next + 5], p[next + 6], p[next + 7]]);
            let len = usize::from(u16::from_be_bytes([p[next + 8], p[next + 9]]));
            out.push((name, kind, ttl, p[next + 10..next + 10 + len].to_vec()));
            at = next + 10 + len;
        }
        out
    }

    #[test]
    fn questions_are_read() {
        let (id, questions) = parse_query(&query("_hap._tcp.local", TYPE_PTR)).unwrap();
        assert_eq!(id, 0x1234);
        assert_eq!(
            questions,
            [Question {
                name: "_hap._tcp.local".into(),
                kind: TYPE_PTR
            }]
        );
        // A response is not a query.
        let mut response = query("x.local", 1);
        response[2] = 0x84;
        assert!(parse_query(&response).is_none());
        assert!(parse_query(&[1, 2, 3]).is_none());
    }

    #[test]
    fn compression_pointers_are_followed_but_loops_are_not() {
        let mut p = vec![0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0];
        put_name(&mut p, "a.local");
        p.extend_from_slice(&[0, 12, 0, 1]);
        // second question: pointer to offset 12
        p.extend_from_slice(&[0xC0, 12, 0, 12, 0, 1]);
        let (_, qs) = parse_query(&p).unwrap();
        assert_eq!(qs[1].name, "a.local");
        let looping = [0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0, 12, 0, 12, 0, 1];
        assert!(parse_query(&looping).is_none());
    }

    #[test]
    fn the_service_pointer_comes_with_its_details() {
        let answers = zone().answer("_hap._tcp.local", TYPE_PTR);
        let kinds: Vec<u16> = answers.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [TYPE_PTR, TYPE_SRV, TYPE_TXT, TYPE_A]);
        assert!(zone().answer("_other._tcp.local", TYPE_PTR).is_empty());
        assert_eq!(zone().answer("_HAP._tcp.local.", TYPE_PTR).len(), 4);
    }

    #[test]
    fn the_txt_record_says_what_homekit_needs() {
        let zone = zone();
        let txt = zone.answer("Moli._hap._tcp.local", TYPE_TXT);
        let Rdata::Txt(strings) = &txt[0].rdata else {
            panic!()
        };
        assert!(strings.contains(&"c#=7".to_owned()));
        assert!(strings.contains(&"id=AA:BB:CC:DD:EE:FF".to_owned()));
        assert!(strings.contains(&"sf=1".to_owned()));
        assert!(strings.contains(&"ci=2".to_owned()));
        assert!(strings.contains(&"sh=rxqrlA==".to_owned()));
        assert!(strings.contains(&"pv=1.1".to_owned()));
    }

    #[test]
    fn responses_encode_with_ttls_and_goodbyes() {
        let zone = zone();
        let live = parse_response(&encode_response(0, zone.all(), false));
        assert_eq!(live.len(), 5);
        let srv = live.iter().find(|r| r.1 == TYPE_SRV).unwrap();
        assert_eq!(srv.2, TTL_SHORT);
        assert_eq!(&srv.3[4..6], &51827u16.to_be_bytes());
        let a = live.iter().find(|r| r.1 == TYPE_A).unwrap();
        assert_eq!(a.3, [192, 168, 1, 10]);
        let gone = parse_response(&encode_response(0, zone.all(), true));
        assert!(gone.iter().all(|r| r.2 == 0));
        // Authoritative response flags.
        assert_eq!(encode_response(9, &[], false)[2..4], [0x84, 0]);
    }

    #[test]
    fn host_names_are_safe_labels() {
        assert_eq!(label_safe("Mon Moli!"), "Mon-Moli");
        assert_eq!(label_safe("!!!"), "Moli");
        assert!(zone().hostname.ends_with("-EEFF.local"));
    }
}
