//! WS-Discovery (ONVIF Core §7): how a recorder finds cameras on the
//! network. A recorder sends a `Probe` to 239.255.255.250:3702; every ONVIF
//! camera served by Moli answers with a `ProbeMatches` (its address, its
//! types, its scopes). Moli also says `Hello` once when it starts.
//!
//! Hostile input, as everything from the network: only a private IPv4
//! address is answered, datagrams are bounded, and at most
//! [`MAX_ANSWERS`] answers go out a second.

use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::onvif::{self, Device, escape};

pub const MULTICAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const PORT: u16 = 3702;
const MAX_DATAGRAM: usize = 8 * 1024;
const MAX_ANSWERS: u32 = 20;

/// The socket that hears probes: port 3702, in the multicast group.
pub fn bind() -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, PORT)).into())?;
    socket.join_multicast_v4(&MULTICAST, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_loop_v4(false)?;
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(_) => false,
    }
}

fn message_id() -> String {
    let mut b = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b)
        .expect("the system's random source");
    let hex: String = b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    });
    format!(
        "urn:uuid:{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

const NAMESPACES: &str = concat!(
    r#"xmlns:s="http://www.w3.org/2003/05/soap-envelope""#,
    r#" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing""#,
    r#" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery""#,
    r#" xmlns:dn="http://www.onvif.org/ver10/network/wsdl""#,
    r#" xmlns:tds="http://www.onvif.org/ver10/device/wsdl""#
);

fn device_part(camera: &Device, ip: IpAddr) -> String {
    let scopes = onvif::scopes(camera).join(" ");
    format!(
        "<a:EndpointReference><a:Address>urn:uuid:{}</a:Address></a:EndpointReference><d:Types>dn:NetworkVideoTransmitter tds:Device</d:Types><d:Scopes>{}</d:Scopes><d:XAddrs>http://{ip}:{}/onvif/device_service</d:XAddrs><d:MetadataVersion>1</d:MetadataVersion>",
        camera.uuid(),
        escape(&scopes),
        camera.port
    )
}

/// The answer of one camera to a probe.
#[must_use]
pub fn probe_matches(camera: &Device, relates_to: &str, ip: IpAddr, number: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope {NAMESPACES}><s:Header><a:MessageID>{}</a:MessageID><a:RelatesTo>{}</a:RelatesTo><a:To>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</a:To><a:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/ProbeMatches</a:Action><d:AppSequence InstanceId="1" MessageNumber="{number}"/></s:Header><s:Body><d:ProbeMatches><d:ProbeMatch>{}</d:ProbeMatch></d:ProbeMatches></s:Body></s:Envelope>"#,
        message_id(),
        escape(relates_to),
        device_part(camera, ip)
    )
}

fn hello(camera: &Device, ip: IpAddr, number: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope {NAMESPACES}><s:Header><a:MessageID>{}</a:MessageID><a:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</a:To><a:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Hello</a:Action><d:AppSequence InstanceId="1" MessageNumber="{number}"/></s:Header><s:Body><d:Hello>{}</d:Hello></s:Body></s:Envelope>"#,
        message_id(),
        device_part(camera, ip)
    )
}

/// Whether a datagram is a probe Moli's cameras answer, and its id.
#[must_use]
pub fn probe(xml: &str) -> Option<&str> {
    if onvif::action(xml)? != "Probe" {
        return None;
    }
    let id = onvif::element(xml, "MessageID")?;
    if id.is_empty() || id.len() > 128 {
        return None;
    }
    // No types: anything. Otherwise, a camera or a device.
    let wanted = onvif::element(xml, "Types").unwrap_or("");
    let fits = wanted.is_empty()
        || wanted.split_whitespace().any(|t| {
            let local = t.rsplit(':').next().unwrap_or(t);
            local == "NetworkVideoTransmitter" || local == "Device"
        });
    fits.then_some(id)
}

/// Answers probes on `socket` until `cancel`; says hello first on `hello_to`.
pub fn serve(
    socket: UdpSocket,
    cameras: Vec<Arc<Device>>,
    hello_to: Option<SocketAddr>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut number: u32 = 0;
        if let Some(to) = hello_to
            && let Some(ip) = onvif::local_for(to)
        {
            for camera in &cameras {
                number += 1;
                let _ = socket
                    .send_to(hello(camera, ip, number).as_bytes(), to)
                    .await;
            }
        }
        let mut buf = vec![0u8; MAX_DATAGRAM];
        let mut window = Instant::now();
        let mut answered = 0u32;
        loop {
            let (n, from) = tokio::select! {
                () = cancel.cancelled() => return,
                r = socket.recv_from(&mut buf) => match r {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::debug!(error = %e, "onvif discovery: receive failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                },
            };
            if !private(from.ip()) || n >= MAX_DATAGRAM {
                continue;
            }
            let Ok(xml) = std::str::from_utf8(&buf[..n]) else {
                continue;
            };
            let Some(id) = probe(xml) else {
                continue;
            };
            if window.elapsed() >= Duration::from_secs(1) {
                window = Instant::now();
                answered = 0;
            }
            if answered >= MAX_ANSWERS {
                continue;
            }
            answered += 1;
            let Some(ip) = onvif::local_for(from) else {
                continue;
            };
            tracing::debug!(%from, "onvif discovery: probe answered");
            for camera in &cameras {
                number = number.wrapping_add(1);
                let reply = probe_matches(camera, id, ip, number);
                let _ = socket.send_to(reply.as_bytes(), from).await;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE: &str = r#"<?xml version="1.0" encoding="utf-8"?><Envelope xmlns:dn="http://www.onvif.org/ver10/network/wsdl" xmlns="http://www.w3.org/2003/05/soap-envelope"><Header><wsa:MessageID xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing">uuid:0a6dc791-2be6-4991-9af1-454778a1917a</wsa:MessageID><wsa:To xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing">urn:schemas-xmlsoap-org:ws:2005:04:discovery</wsa:To><wsa:Action xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing">http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</wsa:Action></Header><Body><Probe xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns="http://schemas.xmlsoap.org/ws/2005/04/discovery"><Types>dn:NetworkVideoTransmitter</Types><Scopes /></Probe></Body></Envelope>"#;

    #[test]
    fn a_camera_probe_is_recognized() {
        assert_eq!(
            probe(PROBE),
            Some("uuid:0a6dc791-2be6-4991-9af1-454778a1917a")
        );
        let any = PROBE.replace("<Types>dn:NetworkVideoTransmitter</Types>", "");
        assert!(probe(&any).is_some());
        let device = PROBE.replace("dn:NetworkVideoTransmitter", "tds:Device");
        assert!(probe(&device).is_some());
        let printer = PROBE.replace("dn:NetworkVideoTransmitter", "wprt:PrintDeviceType");
        assert!(probe(&printer).is_none());
        let resolve = PROBE
            .replace("<Probe ", "<Resolve ")
            .replace("</Probe>", "</Resolve>");
        assert!(probe(&resolve).is_none());
    }

    #[test]
    fn only_private_addresses_are_answered() {
        assert!(private("192.168.1.9".parse().unwrap()));
        assert!(private("10.0.0.9".parse().unwrap()));
        assert!(!private("8.8.8.8".parse().unwrap()));
        assert!(!private("fe80::1".parse().unwrap()));
    }
}
